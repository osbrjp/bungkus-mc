//! The ding: the kitchen-timer bell mc plays when a session finishes its
//! turn, needs the user or fails (DESIGN §9).
//!
//! The sound is synthesised here, so no audio file ships and no audio
//! crate is needed: a short WAV, written once next to `sessions.json`, is
//! handed to the system's own player with a fixed argv. This module never
//! writes to the terminal; when nothing can play, the caller rings the
//! terminal bell instead.

use std::f64::consts::TAU;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use crate::agent::find_on_path;

/// Samples per second.
const RATE: u16 = 44_100;

/// Length of the sound in samples: 0.8 s.
const SAMPLES: u16 = 35_280;

/// The partials of a small struck bell as (frequency in Hz, level, decay
/// time in seconds): the overtones are not harmonic and die first, which
/// is what makes it a "ding" and not a beep.
const PARTIALS: [(f64, f64, f64); 3] = [
    (2100.0, 0.30, 0.30),
    (5796.0, 0.12, 0.12),
    (11340.0, 0.05, 0.05),
];

/// The players tried, in order, with the arguments before the file:
/// macOS, then `PipeWire`, `PulseAudio` and ALSA.
const PLAYERS: [(&str, &[&str]); 4] = [
    ("afplay", &[]),
    ("pw-play", &[]),
    ("paplay", &[]),
    ("aplay", &["-q"]),
];

/// Plays the ding without waiting for it to end.
///
/// Nothing plays over SSH (the sound would come out of the wrong machine)
/// or when no player is on `PATH`.
///
/// # Arguments
///
/// * `file` - Where the WAV is kept; (re)written when it is not the
///   current sound.
///
/// # Returns
///
/// Whether a player was started; on `false` the caller falls back to the
/// terminal bell.
pub(crate) fn play(file: &Path) -> bool {
    if std::env::var_os("SSH_CONNECTION").is_some() {
        return false;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let Some((player, args)) = player(&path) else {
        return false;
    };
    let sound = wav();
    if std::fs::read(file).ok().as_ref() != Some(&sound)
        && crate::store::write_atomic(file, &sound).is_err()
    {
        return false;
    }
    // ponytail: the first player on PATH is trusted to work; try the next
    // one on a failed exit if a pw-play without its daemon turns up.
    let child = Command::new(player)
        .args(args)
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return false };
    thread::spawn(move || {
        // reason: the wait only reaps the player; its exit code changes nothing.
        let _ = child.wait();
    });
    true
}

/// Returns the first of [`PLAYERS`] found in the `PATH` list `path`, with
/// its arguments.
fn player(path: &OsStr) -> Option<(PathBuf, &'static [&'static str])> {
    PLAYERS
        .iter()
        .find_map(|(name, args)| Some((find_on_path(name, path)?, *args)))
}

/// Returns the ding as a 16-bit mono PCM WAV file: the decaying
/// [`PARTIALS`], faded to silence at the end so it does not click.
fn wav() -> Vec<u8> {
    let (rate, data) = (u32::from(RATE), u32::from(SAMPLES) * 2);
    let mut out = Vec::with_capacity(44 + usize::from(SAMPLES) * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    // The 16-byte format block: PCM, one channel, the rates, 2 bytes per
    // frame, 16 bits per sample.
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for i in 0..SAMPLES {
        let t = f64::from(i) / f64::from(RATE);
        let fade = 1.0 - f64::from(i) / f64::from(SAMPLES);
        let level: f64 = PARTIALS
            .iter()
            .map(|(hz, gain, decay)| gain * (-t / decay).exp() * (TAU * hz * t).sin())
            .sum();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the levels sum to under 1, so the product fits an i16"
        )]
        let sample = (level * fade * f64::from(i16::MAX)) as i16;
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns the loudest sample among `frames` of the WAV `bytes`.
    fn peak(bytes: &[u8], frames: std::ops::Range<usize>) -> u16 {
        bytes[44..].chunks_exact(2).collect::<Vec<_>>()[frames]
            .iter()
            .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs())
            .max()
            .unwrap()
    }

    #[test]
    fn the_ding_is_a_wav_that_rings_and_dies_away() {
        let bytes = wav();
        assert_eq!(bytes.len(), 44 + usize::from(SAMPLES) * 2);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
            bytes.len() - 8
        );
        let frames = usize::from(SAMPLES);
        let (strike, tail) = (peak(&bytes, 0..2000), peak(&bytes, frames - 2000..frames));
        assert!(strike > 8000, "audible at the strike: {strike}");
        assert!(strike < 20000, "not at full volume: {strike}");
        assert!(tail < 200, "silent at the end: {tail}");
    }

    #[test]
    fn the_first_player_on_path_is_used_with_its_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("mc-ding-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(player(dir.as_os_str()), None, "no player: the bell rings");
        for name in ["aplay", "paplay"] {
            std::fs::write(dir.join(name), "").unwrap();
            let mode = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(dir.join(name), mode).unwrap();
        }
        assert_eq!(player(dir.as_os_str()), Some((dir.join("paplay"), &[][..])));
        std::fs::remove_file(dir.join("paplay")).unwrap();
        assert_eq!(
            player(dir.as_os_str()),
            Some((dir.join("aplay"), &["-q"][..]))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
