//! Joining a call recording that a mid-call media restart split in two.
//!
//! rtp-engine records per session, and a re-INVITE that moves media starts a
//! new session. The recording carries on in a file of its own, and the old
//! session saves the part before it. History links one recording per call
//! (the path on the call's "ended" event), so the first part used to be on
//! disk with nothing pointing at it. Now the later part is appended to the
//! earlier one when it is saved, and the call's recording is the whole call.
//!
//! The restart registers the new part's path before the old part is saved
//! ([`expect`]), and names the earlier file once it is ([`resolve`]). Saving
//! the new part ([`join`]) waits for that, so a call that ends while the old
//! part is still being written is joined all the same.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// What a continuing part is waiting for.
enum Earlier {
    /// The old session's part is still being saved.
    Pending,
    /// Saved here.
    Saved(PathBuf),
}

/// Continuing parts, by their own path.
static PARTS: LazyLock<Mutex<HashMap<PathBuf, Earlier>>> = LazyLock::new(Default::default);

/// How long [`join`] waits for the earlier part to be saved. A whole call's
/// audio is written in one go, which takes seconds for a long call.
const SAVE_WAIT: Duration = Duration::from_secs(60);

fn parts() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Earlier>> {
    PARTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A path for the part that continues a recording, next to where `call_id`'s
/// recordings go. It only lives until it is joined, but must not collide
/// with the earlier part's name, which has one-second resolution.
pub(crate) fn continuation_path(call_id: &str, dir: &Path) -> PathBuf {
    let base = rtp_engine::generate_recording_filename(call_id, dir);
    let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("call");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    base.with_file_name(format!("{stem}-continued-{nanos}.wav"))
}

/// `next` continues a recording whose earlier part is about to be saved.
pub(crate) fn expect(next: &Path) {
    parts().insert(next.to_owned(), Earlier::Pending);
}

/// The earlier part of the recording `next` continues was saved at
/// `earlier`, or could not be saved (`None`).
pub(crate) fn resolve(next: &Path, earlier: Option<PathBuf>) {
    let mut parts = parts();
    match earlier {
        Some(earlier) => {
            parts.insert(next.to_owned(), Earlier::Saved(earlier));
        }
        None => {
            parts.remove(next);
        }
    }
}

/// The recording `saved` belongs to, once joined: the earlier part with
/// `saved` appended when `saved` continues one, else `saved` itself.
///
/// Blocking: call it where the recording was written.
pub(crate) fn join(saved: PathBuf) -> PathBuf {
    let deadline = Instant::now() + SAVE_WAIT;
    let earlier = loop {
        {
            let mut parts = parts();
            match parts.get(&saved) {
                None => return saved,
                Some(Earlier::Saved(_)) => match parts.remove(&saved) {
                    Some(Earlier::Saved(earlier)) => break earlier,
                    _ => return saved,
                },
                Some(Earlier::Pending) if Instant::now() >= deadline => {
                    parts.remove(&saved);
                    log::warn!("Gave up waiting for the first part of {}", saved.display());
                    return saved;
                }
                Some(Earlier::Pending) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    join_files(earlier, saved)
}

fn join_files(earlier: PathBuf, later: PathBuf) -> PathBuf {
    // A part with no audio is never written.
    match (earlier.exists(), later.exists()) {
        (true, true) => {}
        (true, false) => return earlier,
        _ => return later,
    }
    match append_wav(&earlier, &later) {
        Ok(true) => {
            log::info!("Joined recording {} onto {}", later.display(), earlier.display());
            earlier
        }
        Ok(false) => {
            // The codec's clock rate changed with the media: the parts
            // cannot share one WAV header.
            log::warn!(
                "Recording parts {} and {} differ in format; keeping them apart",
                earlier.display(),
                later.display()
            );
            later
        }
        Err(e) => {
            log::warn!("Could not join recording parts {} and {}: {}", earlier.display(), later.display(), e);
            later
        }
    }
}

/// The length of the header rtp-engine writes: RIFF, a 16-byte PCM `fmt `
/// chunk, then `data`.
const HEADER: usize = 44;

fn read_header(file: &mut File) -> io::Result<Option<[u8; HEADER]>> {
    let mut h = [0u8; HEADER];
    file.read_exact(&mut h)?;
    let fmt_len = u32::from_le_bytes([h[16], h[17], h[18], h[19]]);
    let ok = &h[0..4] == b"RIFF"
        && &h[8..12] == b"WAVE"
        && &h[12..16] == b"fmt "
        && fmt_len == 16
        && &h[36..40] == b"data";
    Ok(ok.then_some(h))
}

/// Append `later`'s audio to `earlier` and delete `later`. `Ok(false)`, with
/// nothing changed, when either is not a WAV as rtp-engine writes them or
/// their formats differ.
fn append_wav(earlier: &Path, later: &Path) -> io::Result<bool> {
    let mut a = OpenOptions::new().read(true).write(true).open(earlier)?;
    let mut b = File::open(later)?;
    let (Some(ha), Some(hb)) = (read_header(&mut a)?, read_header(&mut b)?) else {
        return Ok(false);
    };
    if ha[20..36] != hb[20..36] {
        return Ok(false);
    }
    let a_len = u32::from_le_bytes([ha[40], ha[41], ha[42], ha[43]]);
    let b_len = u32::from_le_bytes([hb[40], hb[41], hb[42], hb[43]]);
    // RIFF sizes are 32-bit: a joined file past 4 GiB cannot be described.
    if a_len.checked_add(b_len).and_then(|t| t.checked_add(36)).is_none() {
        return Ok(false);
    }
    a.seek(SeekFrom::Start(HEADER as u64 + u64::from(a_len)))?;
    let copied = io::copy(&mut (&mut b).take(u64::from(b_len)), &mut a)?;
    let total = a_len + u32::try_from(copied).unwrap_or(b_len);
    a.set_len(HEADER as u64 + u64::from(total))?;
    a.seek(SeekFrom::Start(4))?;
    a.write_all(&(36 + total).to_le_bytes())?;
    a.seek(SeekFrom::Start(40))?;
    a.write_all(&total.to_le_bytes())?;
    a.flush()?;
    drop(b);
    std::fs::remove_file(later)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV as rtp-engine writes it: stereo 16-bit PCM.
    fn wav(path: &Path, rate: u32, samples: &[i16]) {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        f.extend_from_slice(&16u32.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&2u16.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * 4).to_le_bytes());
        f.extend_from_slice(&4u16.to_le_bytes());
        f.extend_from_slice(&16u16.to_le_bytes());
        f.extend_from_slice(b"data");
        f.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.extend_from_slice(&data);
        std::fs::write(path, f).unwrap();
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aria-recording-parts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_continued_recording_is_joined_onto_its_first_part() {
        let d = dir("join");
        let (first, second) = (d.join("a.wav"), d.join("b.wav"));
        wav(&first, 8000, &[1, 2, 3, 4]);
        wav(&second, 8000, &[5, 6]);
        expect(&second);
        resolve(&second, Some(first.clone()));

        assert_eq!(join(second.clone()), first);
        assert!(!second.exists(), "the joined part is removed");
        let mut want = Vec::new();
        wav(&d.join("want.wav"), 8000, &[1, 2, 3, 4, 5, 6]);
        File::open(d.join("want.wav")).unwrap().read_to_end(&mut want).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), want);
    }

    #[test]
    fn join_waits_for_the_first_part_to_be_saved() {
        let d = dir("wait");
        let (first, second) = (d.join("a.wav"), d.join("b.wav"));
        wav(&second, 8000, &[5, 6]);
        expect(&second);
        let saver = {
            let (first, second) = (first.clone(), second.clone());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                wav(&first, 8000, &[1, 2]);
                resolve(&second, Some(first));
            })
        };
        assert_eq!(join(second), first);
        saver.join().unwrap();
        assert_eq!(std::fs::metadata(&first).unwrap().len(), HEADER as u64 + 8);
    }

    #[test]
    fn parts_in_different_formats_are_kept_apart() {
        let d = dir("formats");
        let (first, second) = (d.join("a.wav"), d.join("b.wav"));
        wav(&first, 8000, &[1, 2]);
        wav(&second, 16000, &[5, 6]);
        expect(&second);
        resolve(&second, Some(first.clone()));
        assert_eq!(join(second.clone()), second);
        assert!(first.exists() && second.exists());
    }

    #[test]
    fn a_part_with_no_audio_leaves_the_other() {
        let d = dir("empty");
        let (first, second) = (d.join("a.wav"), d.join("b.wav"));
        wav(&first, 8000, &[1, 2]);
        expect(&second);
        resolve(&second, Some(first.clone()));
        assert_eq!(join(second), first, "an unwritten second part");

        let third = d.join("c.wav");
        wav(&third, 8000, &[1, 2]);
        expect(&third);
        resolve(&third, Some(d.join("never-written.wav")));
        assert_eq!(join(third.clone()), third, "an unwritten first part");
    }

    #[test]
    fn a_recording_that_continues_nothing_is_its_own() {
        let p = dir("alone").join("x.wav");
        assert_eq!(join(p.clone()), p);
    }
}
