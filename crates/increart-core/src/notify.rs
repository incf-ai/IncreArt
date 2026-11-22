//! Notifications (DESIGN.md, Notifications): the settings, the built-in
//! sounds, made as WAV files, and the configurable command that plays them.

use crate::vfs::PathExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `config.rsn` `notifications`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Notifications", default)]
pub struct Notifications {
    /// A GPU (server) ready with no job: `threshold` minutes.
    pub gpu_idle: AlertRule,
    /// Only when every GPU is idle (else any one).
    pub all_gpus: bool,
    /// Prompt enhance asks count as work: no GPU is idle while they go,
    /// and idle time counts again from the last.
    pub pe_counts_as_work: bool,
    /// At once when a GPU finishes its last job (its queue clears).
    pub queue_cleared: EventRule,
    /// Only when the last busy GPU clears (else each one).
    pub queue_cleared_all: bool,
    /// More than `threshold` images waiting for review.
    pub review_backlog: AlertRule,
    /// 0..=1.
    pub volume: f32,
    /// The command that plays a sound file, with `{file}`, `{volume}`
    /// (0.00–1.00), `{volume_percent}` and `{volume_pa}` (0–65536). Empty:
    /// the usual players of this OS, the first one found ([`auto_players`]).
    pub player: String,
    /// No sound while the app's window has the focus.
    pub only_unfocused: bool,
    /// Ask the window manager for attention (taskbar flash).
    pub flash_taskbar: bool,
    /// Say what it's about in the status bar.
    pub show_message: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Notifications {
            gpu_idle: AlertRule { on: false, threshold: 5, repeat_minutes: 5, sound: Sound::Chime },
            all_gpus: false,
            pe_counts_as_work: true,
            queue_cleared: EventRule { on: false, sound: Sound::DoubleBeep },
            queue_cleared_all: false,
            review_backlog: AlertRule { on: false, threshold: 50, repeat_minutes: 15, sound: Sound::Bell },
            volume: 0.8,
            player: String::new(),
            only_unfocused: false,
            flash_taskbar: true,
            show_message: true,
        }
    }
}

/// One alert: on when its condition passes `threshold`, sounding then and
/// every `repeat_minutes` while it holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "AlertRule")]
pub struct AlertRule {
    pub on: bool,
    pub threshold: u32,
    pub repeat_minutes: u32,
    pub sound: Sound,
}

/// An alert for a moment rather than a condition: it sounds once, when it
/// happens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "EventRule")]
pub struct EventRule {
    pub on: bool,
    pub sound: Sound,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sound {
    Chime,
    DoubleBeep,
    Bell,
    /// A sound file of your own (any format the player takes).
    File(String),
}

impl Sound {
    pub const BUILT_IN: [Sound; 3] = [Sound::Chime, Sound::DoubleBeep, Sound::Bell];

    pub fn name(&self) -> String {
        match self {
            Sound::Chime => "chime".into(),
            Sound::DoubleBeep => "double beep".into(),
            Sound::Bell => "bell".into(),
            Sound::File(f) => Path::new(f).file_name().map_or(f.clone(), |n| n.to_string_lossy().into_owned()),
        }
    }
}

/// Ready-made player commands, for the settings' list.
pub const PLAYERS: &[(&str, &str)] = &[
    ("paplay (Linux: PulseAudio, PipeWire)", "paplay --volume={volume_pa} {file}"),
    ("pw-play (Linux: PipeWire)", "pw-play --volume={volume} {file}"),
    ("aplay (Linux: ALSA)", "aplay -q {file}"),
    ("afplay (macOS)", "afplay -v {volume} {file}"),
    (
        "PowerShell (Windows)",
        "powershell -NoProfile -NonInteractive -Command \"(New-Object Media.SoundPlayer '{file}').PlaySync()\"",
    ),
];

/// The commands an empty `player` tries on this OS, in order.
pub fn auto_players() -> Vec<&'static str> {
    let pick = |names: &[&str]| -> Vec<&'static str> {
        PLAYERS.iter().filter(|(_, c)| names.iter().any(|n| c.starts_with(n))).map(|(_, c)| *c).collect()
    };
    if cfg!(target_os = "macos") {
        pick(&["afplay"])
    } else if cfg!(windows) {
        pick(&["powershell"])
    } else {
        pick(&["paplay", "pw-play", "aplay"])
    }
}

/// Splits a command line into arguments: spaces separate, `"…"` and `'…'`
/// group (the quotes are removed), `\` escapes the next character outside
/// single quotes.
pub fn split_command(s: &str) -> Result<Vec<String>, String> {
    let mut args = vec![];
    let mut cur = String::new();
    let mut started = false;
    let mut quote = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('\''), c) => cur.push(c),
            (_, '\\') => cur.push(chars.next().ok_or("the command ends with \\")?),
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                }
                started = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if quote.is_some() {
        return Err("a quote isn't closed".into());
    }
    if started || !cur.is_empty() {
        args.push(cur);
    }
    Ok(args)
}

/// The arguments to run: `template` split, then its placeholders filled in
/// (in each argument, so a path with spaces stays one argument).
pub fn command_for(template: &str, file: &Path, volume: f32) -> Result<Vec<String>, String> {
    let v = volume.clamp(0.0, 1.0);
    let args = split_command(template)?;
    if args.is_empty() {
        return Err("the player command is empty".into());
    }
    Ok(args
        .into_iter()
        .map(|a| {
            a.replace("{file}", &file.to_string_lossy())
                .replace("{volume_percent}", &format!("{}", (v * 100.0).round() as u32))
                .replace("{volume_pa}", &format!("{}", (v * 65536.0).round() as u32))
                .replace("{volume}", &format!("{v:.2}"))
        })
        .collect())
}

/// Plays `sound` and waits for the player. Returns the player used. A
/// built-in sound is made at `volume` (the player gets 1.0); a file of
/// your own gets `volume` through the placeholders.
pub fn play(sound: &Sound, volume: f32, player: &str) -> Result<String, String> {
    let (file, player_volume) = match sound {
        Sound::File(f) if f.trim().is_empty() => return Err("no sound file chosen".into()),
        Sound::File(f) => (PathBuf::from(f), volume),
        s => (built_in_file(s, volume).map_err(|e| format!("making the sound: {e}"))?, 1.0),
    };
    if !player.trim().is_empty() {
        return run(player, &file, player_volume);
    }
    let tried = auto_players();
    for p in &tried {
        match run(p, &file, player_volume) {
            Err(e) if e.contains("not found") => continue,
            r => return r,
        }
    }
    let names: Vec<_> = tried.iter().filter_map(|c| c.split_whitespace().next()).collect();
    Err(format!("no sound player found (tried {}); choose one in Settings → Notifications", names.join(", ")))
}

fn run(template: &str, file: &Path, volume: f32) -> Result<String, String> {
    let args = command_for(template, file, volume)?;
    let mut cmd = std::process::Command::new(&args[0]);
    let out = crate::process::background(&mut cmd, false).args(&args[1..]).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{}: not found", args[0])
        } else {
            format!("{}: {e}", args[0])
        }
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{}: {} {}", args[0], out.status, err.trim()));
    }
    Ok(args[0].clone())
}

/// The built-in sound as a WAV file in the temp folder (made once per volume).
pub fn built_in_file(sound: &Sound, volume: f32) -> std::io::Result<PathBuf> {
    let pct = (volume.clamp(0.0, 1.0) * 100.0).round() as u32;
    let dir = std::env::temp_dir().join("increart-sounds");
    let path = dir.join(format!("{}-{pct}.wav", sound.name().replace(' ', "-")));
    if !path.vfs_is_file() {
        crate::vfs::create_dir_all(&dir)?;
        let tmp = path.with_extension("tmp");
        crate::vfs::write(&tmp, wav(&samples(sound, pct as f32 / 100.0)))?;
        crate::vfs::rename(&tmp, &path)?;
    }
    Ok(path)
}

const RATE: u32 = 44_100;

/// A built-in sound's samples (mono, peak `volume` × 0.8).
pub fn samples(sound: &Sound, volume: f32) -> Vec<f32> {
    use std::f32::consts::TAU;
    // (start s, frequency Hz, amplitude, decay s, length s)
    let notes: Vec<(f32, f32, f32, f32, f32)> = match sound {
        Sound::Chime => vec![(0.0, 880.0, 1.0, 0.25, 0.6), (0.16, 1318.5, 0.9, 0.35, 0.9)],
        Sound::DoubleBeep => vec![(0.0, 1000.0, 1.0, 10.0, 0.12), (0.2, 1000.0, 1.0, 10.0, 0.12)],
        Sound::Bell => [(1.0, 1.0, 1.2), (2.0, 0.6, 0.9), (2.76, 0.4, 0.6), (5.4, 0.25, 0.4), (8.93, 0.15, 0.25)]
            .iter()
            .map(|(ratio, amp, decay)| (0.0, 523.25 * ratio, *amp, *decay, 2.0))
            .collect(),
        Sound::File(_) => vec![],
    };
    let len = notes.iter().map(|n| n.0 + n.4).fold(0.0, f32::max);
    let mut out = vec![0.0f32; (len * RATE as f32) as usize];
    for (start, freq, amp, decay, length) in notes {
        let first = (start * RATE as f32) as usize;
        for i in 0..(length * RATE as f32) as usize {
            let t = i as f32 / RATE as f32;
            // 5 ms fades in and out against clicks
            let fade = (t / 0.005).min(1.0).min((length - t) / 0.005).max(0.0);
            let s = (TAU * freq * t).sin() + 0.25 * (TAU * 2.0 * freq * t).sin();
            if let Some(o) = out.get_mut(first + i) {
                *o += amp * fade * (-t / decay).exp() * s;
            }
        }
    }
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-6);
    let gain = 0.8 * volume.clamp(0.0, 1.0) / peak;
    out.iter_mut().for_each(|s| *s *= gain);
    out
}

/// 16-bit mono PCM WAV.
pub fn wav(samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend(b"RIFF");
    b.extend((36 + data).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes()); // PCM
    b.extend(1u16.to_le_bytes()); // mono
    b.extend(RATE.to_le_bytes());
    b.extend((RATE * 2).to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data.to_le_bytes());
    for s in samples {
        b.extend(((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_split_and_fill_in() {
        assert_eq!(split_command(r#"a "b c" 'd \e' f\ g"#).unwrap(), ["a", "b c", r"d \e", "f g"]);
        assert_eq!(split_command(r#"x """#).unwrap(), ["x", ""]);
        assert!(split_command("a 'b").is_err());
        let file = Path::new("/tmp/my sounds/x.wav");
        assert_eq!(
            command_for("paplay --volume={volume_pa} {file}", file, 0.5).unwrap(),
            ["paplay", "--volume=32768", "/tmp/my sounds/x.wav"]
        );
        assert_eq!(command_for("afplay -v {volume} {file}", file, 2.0).unwrap()[2], "1.00");
        let ps = command_for(PLAYERS[4].1, file, 1.0).unwrap();
        assert_eq!(ps.len(), 5);
        assert_eq!(ps[4], "(New-Object Media.SoundPlayer '/tmp/my sounds/x.wav').PlaySync()");
        assert!(!auto_players().is_empty());
    }

    #[test]
    fn built_in_sounds_are_wav_files_at_the_volume() {
        for s in Sound::BUILT_IN {
            let full = samples(&s, 1.0);
            let peak = full.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!((peak - 0.8).abs() < 1e-3, "{s:?} {peak}");
            let half = samples(&s, 0.5).iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!((half - 0.4).abs() < 1e-3);
            let w = wav(&full);
            assert_eq!(&w[..4], b"RIFF");
            assert_eq!(w.len(), 44 + full.len() * 2);
        }
        let path = built_in_file(&Sound::Chime, 0.3).unwrap();
        assert!(path.ends_with("chime-30.wav") && path.vfs_is_file());
    }
}
