//! Standalone audio device testing (mic level meter + speaker test tone).
//!
//! Allows users to verify their microphone and speakers work from the Settings
//! page, without needing an active call.

use rtp_engine::device::{AudioCapture, AudioPlayback};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Manages standalone audio device testing outside of a call.
pub struct AudioTestManager {
    inner: Mutex<Inner>,
}

struct Inner {
    /// Running mic capture for level metering.
    capture: Option<CaptureTest>,
    /// Running speaker test tone.
    tone_playing: Arc<AtomicBool>,
}

struct CaptureTest {
    capture: AudioCapture,
    level: Arc<Mutex<f32>>,
    running: Arc<AtomicBool>,
}

impl AudioTestManager {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                capture: None,
                tone_playing: Arc::new(AtomicBool::new(false)),
            }),
        }
    }

    /// Start capturing from the given input device and computing RMS levels.
    /// If `device_name` is None or "default", uses the system default.
    pub fn start_input_test(&self, device_name: Option<&str>) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;

        // Stop any existing test first
        if let Some(old) = inner.capture.take() {
            old.running.store(false, Ordering::Relaxed);
            old.capture.stop();
        }

        let dev = match device_name {
            Some("default") | None => None,
            Some(name) => Some(name),
        };

        let capture = AudioCapture::start_with_device_name(dev)
            .map_err(|e| format!("Failed to open input device: {}", e))?;

        let level: Arc<Mutex<f32>> = Arc::new(Mutex::new(0.0));
        let running = Arc::new(AtomicBool::new(true));

        // No background thread needed -- get_test_level() is polled every
        // ~80ms from the frontend and drains the capture buffer at that point.

        inner.capture = Some(CaptureTest {
            capture,
            level,
            running,
        });

        log::info!("Audio input test started");
        Ok(())
    }

    /// Stop the input test.
    pub fn stop_input_test(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(test) = inner.capture.take() {
                test.running.store(false, Ordering::Relaxed);
                test.capture.stop();
                log::info!("Audio input test stopped");
            }
        }
    }

    /// Get the current test input level (RMS 0.0-1.0), or None if no test running.
    pub fn get_test_level(&self) -> Option<f32> {
        let inner = self.inner.lock().ok()?;
        let test = inner.capture.as_ref()?;

        // Read samples that have accumulated since last poll.
        // Use the device's native rate to avoid resampling overhead.
        let device_rate = test.capture.device_rate();
        let samples = test.capture.read_samples(device_rate, device_rate as usize / 10);

        if samples.is_empty() {
            // Return last known level
            return test.level.lock().ok().map(|l| *l);
        }

        // Compute RMS
        let sum_sq: f64 = samples.iter().map(|&s| {
            let f = s as f64 / 32768.0;
            f * f
        }).sum();
        let rms = (sum_sq / samples.len() as f64).sqrt() as f32;

        if let Ok(mut level) = test.level.lock() {
            *level = rms;
        }

        Some(rms)
    }

    /// Play a 1-second 440Hz sine wave test tone through the given output device.
    /// If `device_name` is None or "default", uses the system default.
    pub fn play_test_tone(&self, device_name: Option<&str>) -> Result<(), String> {
        let inner = self.inner.lock().map_err(|e| e.to_string())?;

        if inner.tone_playing.load(Ordering::Relaxed) {
            return Ok(()); // already playing
        }

        let dev = match device_name {
            Some("default") | None => None,
            Some(name) => Some(name.to_string()),
        };

        let tone_flag = inner.tone_playing.clone();
        tone_flag.store(true, Ordering::Relaxed);

        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let playback = AudioPlayback::start_with_device_name(dev.as_deref())
                    .map_err(|e| format!("Failed to open output device: {}", e))?;

                let device_rate = playback.device_rate();
                let duration_secs = 1.5;
                let total_samples = (device_rate as f64 * duration_secs) as usize;
                let freq = 440.0f64;
                let amplitude = 0.25f64; // moderate volume

                // Generate sine wave in chunks and write to playback
                let chunk_size = device_rate as usize / 20; // 50ms chunks
                let mut written = 0usize;

                while written < total_samples {
                    let count = chunk_size.min(total_samples - written);
                    let samples: Vec<i16> = (0..count)
                        .map(|i| {
                            let t = (written + i) as f64 / device_rate as f64;
                            // Apply fade in/out envelope
                            let env = if t < 0.05 {
                                t / 0.05
                            } else if t > duration_secs - 0.05 {
                                (duration_secs - t) / 0.05
                            } else {
                                1.0
                            };
                            let sample = amplitude * env * (2.0 * std::f64::consts::PI * freq * t).sin();
                            (sample * 32767.0) as i16
                        })
                        .collect();

                    playback.write_samples(&samples, device_rate);
                    written += count;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }

                // Wait a bit for the buffer to drain
                std::thread::sleep(std::time::Duration::from_millis(200));
                playback.stop();
                Ok(())
            })();

            if let Err(e) = result {
                log::error!("Test tone failed: {}", e);
            }

            tone_flag.store(false, Ordering::Relaxed);
            log::info!("Test tone finished");
        });

        log::info!("Test tone started");
        Ok(())
    }
}
