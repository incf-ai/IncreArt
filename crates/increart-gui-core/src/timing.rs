//! Timing statistics (DESIGN.md, Timing): per image, run and project, from
//! the timings each run records (`run.rsn` `timings`), and the live seconds
//! per step of images being sampled.

use std::collections::BTreeMap;

use increart_core::model::{Run, SeedTiming};

/// What a set of timed images took.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timing {
    pub images: usize,
    /// Generating, summed (GPU time).
    pub gpu_s: f64,
    /// Generating, per image: mean, fastest, slowest.
    pub per_image: f64,
    pub fastest: f64,
    pub slowest: f64,
    /// Seconds per sampling step: the mean measured one, else estimated
    /// from the generating time (which also counts encoding and decoding).
    pub step_s: f64,
    pub step_measured: bool,
    /// Waiting in a server's queue, per image (mean).
    pub queued_s: f64,
    /// First image started → last one stored, as the clock went (runs
    /// sharing the GPUs and pauses included).
    pub wall_s: f64,
    /// Per device: (device, images, mean generating time).
    pub devices: Vec<(String, usize, f64)>,
}

impl Timing {
    /// Statistics of these images, each with its run's steps; `None` when
    /// there are none.
    pub fn of<'a>(timed: impl IntoIterator<Item = (&'a SeedTiming, u32)>) -> Option<Timing> {
        let timed: Vec<(&SeedTiming, u32)> = timed.into_iter().collect();
        if timed.is_empty() {
            return None;
        }
        let n = timed.len() as f64;
        let elapsed = |t: &SeedTiming| t.elapsed_s as f64;
        let gpu_s: f64 = timed.iter().map(|(t, _)| elapsed(t)).sum();
        let measured: Vec<f64> = timed.iter().filter(|(t, _)| t.step_s > 0.0).map(|(t, _)| t.step_s as f64).collect();
        let (step_s, step_measured) = if measured.is_empty() {
            let steps: u32 = timed.iter().map(|(_, s)| *s).sum();
            (gpu_s / steps.max(1) as f64, false)
        } else {
            (measured.iter().sum::<f64>() / measured.len() as f64, true)
        };
        let start = |t: &SeedTiming| t.done.timestamp_millis() as f64 / 1000.0 - elapsed(t) - t.queued_s as f64;
        let first = timed.iter().map(|(t, _)| start(t)).fold(f64::INFINITY, f64::min);
        let last =
            timed.iter().map(|(t, _)| t.done.timestamp_millis() as f64 / 1000.0).fold(f64::NEG_INFINITY, f64::max);
        let mut devices: BTreeMap<&str, (usize, f64)> = BTreeMap::new();
        for (t, _) in &timed {
            let d = devices.entry(t.device.as_str()).or_default();
            d.0 += 1;
            d.1 += elapsed(t);
        }
        Some(Timing {
            images: timed.len(),
            gpu_s,
            per_image: gpu_s / n,
            fastest: timed.iter().map(|(t, _)| elapsed(t)).fold(f64::INFINITY, f64::min),
            slowest: timed.iter().map(|(t, _)| elapsed(t)).fold(0.0, f64::max),
            step_s,
            step_measured,
            queued_s: timed.iter().map(|(t, _)| t.queued_s as f64).sum::<f64>() / n,
            wall_s: (last - first).max(0.0),
            devices: devices.into_iter().map(|(d, (k, s))| (d.to_string(), k, s / k as f64)).collect(),
        })
    }

    /// A run's images.
    pub fn of_run(run: &Run) -> Option<Timing> {
        Timing::of(run.timings.iter().map(|t| (t, run.sample.steps)))
    }

    /// Every image of these runs (a project, everything).
    pub fn of_runs<'a>(runs: impl IntoIterator<Item = &'a Run>) -> Option<Timing> {
        Timing::of(runs.into_iter().flat_map(|r| r.timings.iter().map(|t| (t, r.sample.steps))))
    }

    /// Seconds per step for display: "1.71 s/it", "≈1.9 s/it" when estimated.
    pub fn step_text(&self) -> String {
        format!("{}{:.2} s/it", if self.step_measured { "" } else { "≈" }, self.step_s)
    }
}

/// One image's timing, for its tooltip: "34.2 s · 1.71 s/it · ROCm0".
pub fn image_text(t: &SeedTiming, steps: u32) -> String {
    let step = if t.step_s > 0.0 {
        format!("{:.2} s/it", t.step_s)
    } else {
        format!("≈{:.2} s/it", t.elapsed_s / steps.max(1) as f32)
    };
    let mut parts = vec![format!("generated in {}", duration(t.elapsed_s as f64)), step];
    if t.queued_s >= 1.0 {
        parts.push(format!("waited {}", duration(t.queued_s as f64)));
    }
    if !t.device.is_empty() {
        parts.push(t.device.clone());
    }
    parts.join(" · ")
}

/// A duration for display: "0.84 s", "34.2 s", "4 min 12 s", "2 h 05 min".
pub fn duration(s: f64) -> String {
    let s = s.max(0.0);
    if s < 1.0 {
        format!("{s:.2} s")
    } else if s < 60.0 {
        format!("{s:.1} s")
    } else if s < 3600.0 {
        let s = s.round() as u64;
        format!("{} min {:02} s", s / 60, s % 60)
    } else {
        let m = (s / 60.0).round() as u64;
        format!("{} h {:02} min", m / 60, m % 60)
    }
}

/// How many of a server's latest images its speed is of.
pub const RECENT: usize = 20;

/// One image a server made this session, for its speed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Recent {
    pub elapsed_s: f32,
    /// Measured seconds per step; 0 when not.
    pub step_s: f32,
    pub steps: u32,
}

/// A server's speed over its latest images (the server list).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speed {
    pub images: usize,
    pub per_image: f64,
    pub step_s: f64,
    pub step_measured: bool,
}

impl Speed {
    /// `None` when there are none.
    pub fn of(recent: &[Recent]) -> Option<Speed> {
        if recent.is_empty() {
            return None;
        }
        let per_image = recent.iter().map(|r| r.elapsed_s as f64).sum::<f64>() / recent.len() as f64;
        let measured: Vec<f64> = recent.iter().filter(|r| r.step_s > 0.0).map(|r| r.step_s as f64).collect();
        let (step_s, step_measured) = if measured.is_empty() {
            let steps: u32 = recent.iter().map(|r| r.steps).sum();
            (recent.iter().map(|r| r.elapsed_s as f64).sum::<f64>() / steps.max(1) as f64, false)
        } else {
            (measured.iter().sum::<f64>() / measured.len() as f64, true)
        };
        Some(Speed { images: recent.len(), per_image, step_s, step_measured })
    }

    /// Images an hour, generating one after another.
    pub fn per_hour(&self) -> f64 {
        if self.per_image > 0.0 { 3600.0 / self.per_image } else { 0.0 }
    }

    /// "1.74 s/it · 35.1 s/image · 103/h".
    pub fn text(&self) -> String {
        let step = format!("{}{:.2} s/it", if self.step_measured { "" } else { "≈" }, self.step_s);
        format!("{step} · {}/image · {:.0}/h", duration(self.per_image), self.per_hour())
    }
}

/// Time left for display, coarser the longer it is: "~4 min 12 s", "~38
/// min", "~2 h 05 min"; from 10 minutes on, with when that is by the
/// clock: "~38 min · ≈15:12", "≈Tue 09:40" when it isn't today.
pub fn left_text(s: f64) -> String {
    left_text_at(s, chrono::Local::now().naive_local())
}

pub fn left_text_at(s: f64, now: chrono::NaiveDateTime) -> String {
    let s = s.max(0.0);
    let d = if (600.0..3600.0).contains(&s) { format!("{} min", (s / 60.0).round() as u64) } else { duration(s) };
    if s < 600.0 {
        return format!("~{d} left");
    }
    let at = now + chrono::Duration::seconds(s.round() as i64);
    let clock = if at.date() == now.date() { at.format("%H:%M") } else { at.format("%a %H:%M") };
    format!("~{d} left · ≈{clock}")
}

/// An image being sampled: when its first and latest steps were seen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sampling {
    pub first: (f64, u32),
    pub latest: (f64, u32),
}

impl Sampling {
    pub fn new(now: f64, step: u32) -> Sampling {
        Sampling { first: (now, step), latest: (now, step) }
    }

    /// Seconds per step so far; `None` until two steps were seen.
    pub fn step_s(&self) -> Option<f64> {
        let steps = self.latest.1.checked_sub(self.first.1).filter(|s| *s > 0)?;
        Some((self.latest.0 - self.first.0) / steps as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(seed: u64, device: &str, done_s: i64, elapsed: f32, step_s: f32) -> SeedTiming {
        SeedTiming {
            rev: 0,
            seed,
            device: device.into(),
            done: chrono::DateTime::from_timestamp(1_800_000_000 + done_s, 0).unwrap(),
            elapsed_s: elapsed,
            queued_s: 1.0,
            step_s,
        }
    }

    #[test]
    fn statistics_of_timed_images() {
        let a = [t(0, "ROCm0", 34, 33.0, 1.5), t(1, "ROCm1", 36, 35.0, 1.7), t(2, "ROCm0", 70, 30.0, 0.0)];
        let s = Timing::of(a.iter().map(|t| (t, 20))).unwrap();
        assert_eq!(s.images, 3);
        assert_eq!(s.gpu_s, 98.0);
        assert!((s.per_image - 98.0 / 3.0).abs() < 1e-9);
        assert_eq!((s.fastest, s.slowest), (30.0, 35.0));
        assert!(s.step_measured && (s.step_s - 1.6).abs() < 1e-6, "the measured ones: {}", s.step_s);
        assert_eq!(s.queued_s, 1.0);
        // seed 0 started at 34 - 33 - 1 = 0, the last was stored at 70
        assert_eq!(s.wall_s, 70.0);
        assert_eq!(s.devices, [("ROCm0".to_string(), 2, 31.5), ("ROCm1".to_string(), 1, 35.0)]);

        // none measured: estimated from the generating time
        let b = [t(0, "", 40, 40.0, 0.0)];
        let s = Timing::of(b.iter().map(|t| (t, 20))).unwrap();
        assert!(!s.step_measured && s.step_s == 2.0);
        assert_eq!(s.step_text(), "≈2.00 s/it");
        assert!(Timing::of(std::iter::empty()).is_none());
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(0.844), "0.84 s");
        assert_eq!(duration(34.24), "34.2 s");
        assert_eq!(duration(252.0), "4 min 12 s");
        assert_eq!(duration(7500.0), "2 h 05 min");
        assert_eq!(
            image_text(&t(0, "ROCm0", 0, 34.2, 1.71), 20),
            "generated in 34.2 s · 1.71 s/it · waited 1.0 s · ROCm0"
        );
    }

    #[test]
    fn a_servers_speed() {
        assert_eq!(Speed::of(&[]), None);
        let r = |e, s| Recent { elapsed_s: e, step_s: s, steps: 20 };
        let s = Speed::of(&[r(36.0, 1.7), r(34.0, 1.5), r(35.0, 0.0)]).unwrap();
        assert_eq!((s.images, s.per_image, s.step_measured), (3, 35.0, true));
        assert!((s.step_s - 1.6).abs() < 1e-6);
        assert_eq!(s.text(), "1.60 s/it · 35.0 s/image · 103/h");
        let s = Speed::of(&[r(40.0, 0.0)]).unwrap();
        assert_eq!(s.text(), "≈2.00 s/it · 40.0 s/image · 90/h");
    }

    #[test]
    fn time_left_with_the_clock_from_ten_minutes() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap().and_hms_opt(14, 34, 0).unwrap();
        assert_eq!(left_text_at(252.0, now), "~4 min 12 s left");
        assert_eq!(left_text_at(38.0 * 60.0 + 20.0, now), "~38 min left · ≈15:12");
        assert_eq!(left_text_at(7500.0, now), "~2 h 05 min left · ≈16:39");
        // tomorrow: the day too (2026-11-03 is a Tuesday)
        assert_eq!(left_text_at(20.0 * 3600.0, now), "~20 h 00 min left · ≈Tue 10:34");
    }

    #[test]
    fn live_seconds_per_step() {
        let mut s = Sampling::new(10.0, 2);
        assert_eq!(s.step_s(), None);
        s.latest = (16.0, 6);
        assert_eq!(s.step_s(), Some(1.5));
    }
}
