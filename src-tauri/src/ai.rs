//! On-device transcription for the desktop app.
//!
//! The desktop already writes call recordings as WAV, so unlike the mobile core
//! — which taps live RTP because it has no recording to read — this transcribes
//! a finished file. Nothing runs during the call, and the media path is
//! untouched.
//!
//! Everything here has a no-op counterpart compiled when the `ai` feature is
//! off, so one binary can answer [`ai_available`] at runtime rather than the
//! frontend inferring support from a build flavour.

use serde::{Deserialize, Serialize};

/// A model as the frontend sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModelDto {
    pub id: String,
    pub display_name: String,
    /// "stt" or "llm".
    pub kind: String,
    pub size_bytes: u64,
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiDownloadProgressDto {
    pub model_id: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    /// "queued", "downloading", "verifying", "completed", "failed", "cancelled".
    pub state: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSegmentDto {
    /// "local" or "remote".
    pub speaker: String,
    pub start_ms: u32,
    pub end_ms: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiInsightDto {
    pub call_id: String,
    pub created_at: i64,
    pub duration_secs: u32,
    pub segments: Vec<AiSegmentDto>,
    pub language: Option<String>,
    pub summary_headline: Option<String>,
    pub summary_points: Vec<String>,
    pub status: String,
    /// The call was carried at 8 kHz, so accuracy is materially worse and the
    /// UI has to say so.
    pub narrowband: bool,
}

/// A stored insight in list form, without the transcript text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiInsightSummaryDto {
    pub call_id: String,
    pub created_at: i64,
    pub duration_secs: u32,
    pub status: String,
}

#[cfg(feature = "ai")]
pub use enabled::*;

#[cfg(not(feature = "ai"))]
pub use disabled::*;

#[cfg(feature = "ai")]
mod enabled {
    use super::{
        AiDownloadProgressDto, AiInsightDto, AiInsightSummaryDto, AiModelDto, AiSegmentDto,
    };
    use aria_ai_core::engine::{AiConfig, AiEngine};
    use aria_ai_core::insights::InsightStore;
    use aria_ai_core::models::catalog::ModelKind;
    use aria_ai_core::models::DeviceCapability;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    struct Desktop {
        engine: AiEngine,
        /// `None` when the store could not be opened. A failed store costs
        /// history, not transcription itself.
        store: Option<InsightStore>,
    }

    static STATE: OnceLock<Mutex<Option<Desktop>>> = OnceLock::new();

    fn state() -> &'static Mutex<Option<Desktop>> {
        STATE.get_or_init(|| Mutex::new(None))
    }

    /// Where models and the insight database live.
    ///
    /// This used to join a hardcoded `com.5060.aria`, which is not the bundle
    /// identifier, so it sat outside the app data directory Tauri manages.
    /// Correcting it moves the location: any models or insights written by an
    /// earlier build stay at the old path and are not read from here. They can
    /// be deleted, and models will be re-downloaded on demand.
    fn storage_root() -> Result<PathBuf, String> {
        crate::app_data_root()
            .map(|d| d.join("ai"))
            .ok_or_else(|| "no data directory on this platform".to_string())
    }

    /// The key transcripts are encrypted with at rest, created on first use.
    ///
    /// The desktop has no keystore the way Android does, so the key is a file
    /// beside the database with owner-only permissions. That protects it from
    /// other users on the machine, not from someone with this account — which
    /// is the honest limit, and worth stating rather than implying more.
    fn insight_key(root: &Path) -> Result<Vec<u8>, String> {
        let path = root.join("insights.key");
        if let Ok(existing) = std::fs::read(&path) {
            if existing.len() == 32 {
                return Ok(existing);
            }
            log::warn!("insight key was malformed; generating a new one");
        }
        let mut key = vec![0_u8; 32];
        getrandom(&mut key)?;
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        std::fs::write(&path, &key).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(key)
    }

    fn getrandom(buf: &mut [u8]) -> Result<(), String> {
        use rand::RngCore;
        rand::thread_rng().fill_bytes(buf);
        Ok(())
    }

    /// Bring the engine up if it is not already. Safe to call repeatedly.
    fn ensure_init() -> Result<(), String> {
        let mut guard = state().lock().map_err(|_| "AI state poisoned".to_string())?;
        if guard.is_some() {
            return Ok(());
        }
        let root = storage_root()?;
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let config = AiConfig::new(&root, DeviceCapability::desktop_default());
        let engine = AiEngine::new(config).map_err(|e| e.to_string())?;

        let store = match insight_key(&root) {
            Ok(key) => match InsightStore::open(root.join("insights.db"), &key) {
                Ok(s) => Some(s),
                Err(e) => {
                    log::error!("insight store unavailable, transcripts will not persist: {e}");
                    None
                }
            },
            Err(e) => {
                log::error!("no insight key, transcripts will not persist: {e}");
                None
            }
        };
        *guard = Some(Desktop { engine, store });
        Ok(())
    }

    /// Run `f` against the initialised state.
    fn with<T>(f: impl FnOnce(&Desktop) -> Result<T, String>) -> Result<T, String> {
        ensure_init()?;
        let guard = state().lock().map_err(|_| "AI state poisoned".to_string())?;
        let desktop = guard.as_ref().ok_or_else(|| "AI not initialised".to_string())?;
        f(desktop)
    }

    pub fn ai_available() -> bool {
        true
    }

    /// Whether finished recordings should be transcribed automatically.
    ///
    /// Off until asked for. A recording existing is not consent to transcribe
    /// it, and the file is a flag beside the models rather than frontend state
    /// so the backend can check it without a round trip.
    pub fn auto_transcribe() -> bool {
        storage_root().is_ok_and(|r| r.join("auto-transcribe").exists())
    }

    pub fn set_auto_transcribe(on: bool) -> Result<(), String> {
        let root = storage_root()?;
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let flag = root.join("auto-transcribe");
        if on {
            std::fs::write(&flag, b"1").map_err(|e| e.to_string())
        } else {
            match std::fs::remove_file(&flag) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        }
    }

    /// Transcribe a recording in the background, if the user asked for that.
    ///
    /// Deliberately fire-and-forget: this runs off the back of a hangup, and a
    /// transcription problem must not surface as a call-teardown failure.
    pub fn maybe_transcribe_in_background(call_id: &str, wav_path: &Path) {
        if !auto_transcribe() {
            return;
        }
        let call_id = call_id.to_string();
        let path = wav_path.to_path_buf();
        std::thread::spawn(move || match transcribe_recording(&call_id, &path) {
            Ok(i) => log::info!(
                "Transcribed {call_id}: {} segments ({})",
                i.segments.len(),
                i.status
            ),
            Err(e) => log::warn!("Could not transcribe {call_id}: {e}"),
        });
    }

    pub fn models() -> Result<Vec<AiModelDto>, String> {
        with(|d| {
            let installed: Vec<String> =
                d.engine.installed_models().into_iter().map(|m| m.id).collect();
            Ok(d.engine
                .availability()
                .into_iter()
                .map(|a| AiModelDto {
                    id: a.model.id.clone(),
                    display_name: a.model.display_name.clone(),
                    kind: match a.model.kind {
                        ModelKind::Stt => "stt",
                        ModelKind::Llm => "llm",
                        ModelKind::Vad => "vad",
                    }
                    .to_string(),
                    size_bytes: a.model.size_bytes,
                    available: a.available,
                    unavailable_reason: a.reason.clone(),
                    installed: installed.iter().any(|i| i == &a.model.id),
                })
                .collect())
        })
    }

    pub fn start_download(model_id: &str) -> Result<(), String> {
        with(|d| d.engine.start_download(model_id).map_err(|e| e.to_string()))
    }

    pub fn download_progress(model_id: &str) -> Result<Option<AiDownloadProgressDto>, String> {
        with(|d| {
            Ok(d.engine.download_progress(model_id).map(|p| {
                use aria_ai_core::models::DownloadState;
                // The reason travels in `error` rather than inside the state
                // string, so the frontend can match on a stable set of states.
                let (state, error) = match &p.state {
                    DownloadState::Queued => ("queued", None),
                    DownloadState::Running => ("downloading", None),
                    DownloadState::Verifying => ("verifying", None),
                    DownloadState::Completed => ("completed", None),
                    DownloadState::Cancelled => ("cancelled", None),
                    DownloadState::Failed { reason } => ("failed", Some(reason.clone())),
                };
                AiDownloadProgressDto {
                    model_id: model_id.to_string(),
                    downloaded_bytes: p.downloaded_bytes,
                    total_bytes: p.total_bytes,
                    state: state.to_string(),
                    error,
                }
            }))
        })
    }

    pub fn cancel_download(model_id: &str) -> Result<(), String> {
        with(|d| {
            d.engine.cancel_download(model_id);
            Ok(())
        })
    }

    pub fn delete_model(model_id: &str) -> Result<u64, String> {
        with(|d| d.engine.delete_model(model_id).map_err(|e| e.to_string()))
    }

    /// Transcribe a finished recording and store the result under `call_id`.
    ///
    /// Blocking and slow — seconds to minutes — so callers must run it off the
    /// UI thread. Tauri commands marked `async` already do.
    pub fn transcribe_recording(call_id: &str, wav_path: &Path) -> Result<AiInsightDto, String> {
        // Inference takes seconds to minutes. Holding the state lock across it
        // would block every other command — listing models, polling a download
        // — for that whole time, so the lock is only held to hand out the
        // transcriber, and again at the end to store the result.
        let (stt_id, transcriber) = with(|d| {
            let stt_id = d
                .engine
                .recommended(ModelKind::Stt)
                .map(|m| m.id.clone())
                .ok_or_else(|| "no speech-to-text model installed".to_string())?;
            let transcriber = d.engine.transcriber(&stt_id).map_err(|e| e.to_string())?;
            Ok((stt_id, transcriber))
        })?;

        let (samples, rate) =
            aria_ai_core::audio::wav::read_mono_i16(wav_path).map_err(|e| e.to_string())?;
        // A recording is one mixed track, so both sides are already in it.
        // There is no per-leg separation to recover here, which is why every
        // segment comes back attributed to the remote side rather than guessed
        // at.
        let narrowband = rate <= 8_000;
        let pcm = aria_ai_core::audio::SincResampler::resample_all(
            rate,
            aria_ai_core::audio::TARGET_RATE_HZ,
            &samples,
        )
        .map_err(|e| e.to_string())?;

        let duration_secs = if rate == 0 {
            0
        } else {
            u32::try_from(samples.len() / rate as usize).unwrap_or(u32::MAX)
        };

        let req = aria_ai_core::stt::TranscribeRequest {
            samples: &pcm,
            sample_rate: aria_ai_core::audio::TARGET_RATE_HZ,
            speaker: aria_ai_core::types::Speaker::Remote,
            language: None,
        };
        let out = transcriber.transcribe(&req).map_err(|e| e.to_string())?;

        let segments: Vec<aria_ai_core::types::TranscriptSegment> = out
            .utterances
            .iter()
            .map(|u| aria_ai_core::types::TranscriptSegment {
                speaker: aria_ai_core::types::Speaker::Remote,
                start_ms: u.start_ms,
                end_ms: u.end_ms,
                text: u.text.clone(),
                confidence: u.confidence,
            })
            .collect();

        let transcript = aria_ai_core::types::Transcript {
            call_id: call_id.to_string(),
            language: out.language.clone(),
            duration_ms: duration_secs.saturating_mul(1000),
            segments,
            narrowband,
            lost_audio: false,
        };

        let insight = aria_ai_core::types::CallInsight {
            call_id: call_id.to_string(),
            created_at: now_secs(),
            duration_secs,
            transcript: Some(transcript),
            summary: None,
            stt_model_id: Some(stt_id),
            llm_model_id: None,
            // A desktop recording is one mixed track with no summariser wired
            // in yet, so the transcript is the whole result rather than a step
            // on the way to one.
            status: aria_ai_core::types::InsightStatus::TranscriptOnly {
                reason: "summarisation is not enabled on desktop".to_string(),
            },
        };

        // Best effort, for the same reason it is on mobile: the caller already
        // holds the transcript, and losing history is not a reason to report
        // the transcription as failed.
        let _ = with(|d| {
            if let Some(store) = d.store.as_ref() {
                if let Err(e) = store.put(&insight) {
                    log::warn!("could not persist insight for {}: {e}", insight.call_id);
                }
            }
            Ok(())
        });
        Ok(to_dto(&insight))
    }

    pub fn insights(limit: u32, offset: u32) -> Result<Vec<AiInsightSummaryDto>, String> {
        with(|d| {
            let Some(store) = d.store.as_ref() else {
                return Ok(Vec::new());
            };
            Ok(store
                .list(limit, offset)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|r| AiInsightSummaryDto {
                    call_id: r.call_id,
                    created_at: r.created_at,
                    duration_secs: r.duration_secs,
                    status: r.status,
                })
                .collect())
        })
    }

    pub fn insight(call_id: &str) -> Result<Option<AiInsightDto>, String> {
        with(|d| {
            let Some(store) = d.store.as_ref() else {
                return Ok(None);
            };
            Ok(store
                .get(call_id)
                .map_err(|e| e.to_string())?
                .as_ref()
                .map(to_dto))
        })
    }

    pub fn delete_insight(call_id: &str) -> Result<bool, String> {
        with(|d| {
            d.store
                .as_ref()
                .map_or(Ok(false), |s| s.delete(call_id).map_err(|e| e.to_string()))
        })
    }

    pub fn clear_insights() -> Result<u64, String> {
        with(|d| {
            d.store
                .as_ref()
                .map_or(Ok(0), |s| s.clear().map_err(|e| e.to_string()))
        })
    }

    fn now_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
    }

    fn to_dto(i: &aria_ai_core::types::CallInsight) -> AiInsightDto {
        use aria_ai_core::types::{InsightStatus, Speaker};

        let (segments, language, narrowband) = i.transcript.as_ref().map_or_else(
            || (Vec::new(), None, false),
            |t| {
                (
                    t.segments
                        .iter()
                        .map(|s| AiSegmentDto {
                            speaker: match s.speaker {
                                Speaker::Local => "local",
                                Speaker::Remote => "remote",
                            }
                            .to_string(),
                            start_ms: s.start_ms,
                            end_ms: s.end_ms,
                            text: s.text.clone(),
                        })
                        .collect(),
                    Some(t.language.clone()),
                    t.narrowband,
                )
            },
        );
        let (summary_headline, summary_points) = i.summary.as_ref().map_or_else(
            || (None, Vec::new()),
            |s| (Some(s.headline.clone()), s.key_points.clone()),
        );

        AiInsightDto {
            call_id: i.call_id.clone(),
            created_at: i.created_at,
            duration_secs: i.duration_secs,
            segments,
            language,
            summary_headline,
            summary_points,
            status: match &i.status {
                InsightStatus::Pending => "pending",
                InsightStatus::Transcribing => "transcribing",
                InsightStatus::Summarizing => "summarizing",
                InsightStatus::Complete => "complete",
                InsightStatus::TranscriptOnly { .. } => "transcript_only",
                InsightStatus::Failed { .. } => "failed",
            }
            .to_string(),
            narrowband,
        }
    }
}

#[cfg(not(feature = "ai"))]
mod disabled {
    use super::{AiDownloadProgressDto, AiInsightDto, AiInsightSummaryDto, AiModelDto};
    use std::path::Path;

    const OFF: &str = "this build was made without transcription support";

    pub const fn ai_available() -> bool {
        false
    }

    pub const fn auto_transcribe() -> bool {
        false
    }

    pub fn set_auto_transcribe(_on: bool) -> Result<(), String> {
        Err(OFF.to_string())
    }

    pub fn maybe_transcribe_in_background(_call_id: &str, _wav_path: &Path) {}

    pub fn models() -> Result<Vec<AiModelDto>, String> {
        Ok(Vec::new())
    }

    pub fn start_download(_model_id: &str) -> Result<(), String> {
        Err(OFF.to_string())
    }

    pub const fn download_progress(
        _model_id: &str,
    ) -> Result<Option<AiDownloadProgressDto>, String> {
        Ok(None)
    }

    pub fn cancel_download(_model_id: &str) -> Result<(), String> {
        Err(OFF.to_string())
    }

    pub fn delete_model(_model_id: &str) -> Result<u64, String> {
        Err(OFF.to_string())
    }

    pub fn transcribe_recording(_call_id: &str, _wav_path: &Path) -> Result<AiInsightDto, String> {
        Err(OFF.to_string())
    }

    pub fn insights(_limit: u32, _offset: u32) -> Result<Vec<AiInsightSummaryDto>, String> {
        Ok(Vec::new())
    }

    pub const fn insight(_call_id: &str) -> Result<Option<AiInsightDto>, String> {
        Ok(None)
    }

    pub const fn delete_insight(_call_id: &str) -> Result<bool, String> {
        Ok(false)
    }

    pub const fn clear_insights() -> Result<u64, String> {
        Ok(0)
    }
}
