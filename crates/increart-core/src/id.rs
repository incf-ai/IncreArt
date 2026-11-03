//! IDs: `YYYY.MM.DD-HH.MM-NNNN`, the UTC creation minute plus a counter from
//! 0000, separate for each kind; and the UUIDv7 kept beside each (DESIGN.md,
//! IDs, UUIDs).

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Datelike, NaiveDate, SubsecRound, Timelike, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

pub type Timestamp = DateTime<Utc>;

/// Current time to the second (stored timestamps carry no fraction).
pub fn now() -> Timestamp {
    Utc::now().trunc_subsecs(0)
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IdKind {
    Idea,
    Project,
    Run,
    InputImage,
    OutputImage,
}

impl Id {
    pub fn new(at: Timestamp, seq: u32) -> Id {
        Id(format!("{}-{seq:04}", minute(at)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The `YYYY.MM.DD-HH.MM` part.
    pub fn minute(&self) -> &str {
        &self.0[..16]
    }

    /// The creation minute as a time.
    pub fn time(&self) -> Option<Timestamp> {
        chrono::NaiveDateTime::parse_from_str(self.minute(), "%Y.%m.%d-%H.%M").ok().map(|t| t.and_utc())
    }

    pub fn seq(&self) -> u32 {
        // (validated on construction)
        self.0[17..].parse().unwrap_or_default()
    }
}

fn minute(at: Timestamp) -> String {
    format!("{:04}.{:02}.{:02}-{:02}.{:02}", at.year(), at.month(), at.day(), at.hour(), at.minute())
}

impl FromStr for Id {
    type Err = Error;

    fn from_str(s: &str) -> Result<Id> {
        let bad = || Error::InvalidId(s.to_string());
        let b = s.as_bytes();
        if b.len() < 21 || !s.is_ascii() {
            return Err(bad());
        }
        let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
        let shape = digits(0..4)
            && b[4] == b'.'
            && digits(5..7)
            && b[7] == b'.'
            && digits(8..10)
            && b[10] == b'-'
            && digits(11..13)
            && b[13] == b'.'
            && digits(14..16)
            && b[16] == b'-'
            && digits(17..b.len());
        if !shape {
            return Err(bad());
        }
        // (all digits, checked above; too large fails below)
        let n = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap_or(u32::MAX);
        let valid = NaiveDate::from_ymd_opt(n(0..4) as i32, n(5..7), n(8..10)).is_some()
            && n(11..13) < 24
            && n(14..16) < 60
            && s[17..].parse::<u32>().is_ok();
        if !valid {
            return Err(bad());
        }
        Ok(Id(s.to_string()))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.0)
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Id, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Hands out the next number for a minute, per kind. Seeded by
/// [`observe`](Self::observe) with the IDs found when the library is opened.
#[derive(Default, Debug)]
pub struct IdAllocator {
    next: HashMap<(IdKind, String), u32>,
}

impl IdAllocator {
    pub fn observe(&mut self, kind: IdKind, id: &Id) {
        let next = self.next.entry((kind, id.minute().to_string())).or_insert(0);
        *next = (*next).max(id.seq() + 1);
    }

    pub fn allocate(&mut self, kind: IdKind, at: Timestamp) -> Id {
        let next = self.next.entry((kind, minute(at))).or_insert(0);
        let id = Id::new(at, *next);
        *next += 1;
        id
    }
}

// ------------------------------------------------------------------ UUIDs

pub use uuid::Uuid;

/// A UUIDv7 for `at` (to the millisecond), the rest random.
pub fn uuid_at(at: Timestamp) -> Uuid {
    let mut random = [0u8; 10];
    // (never fails where the app runs; zeros still leave a valid UUID)
    let _ = getrandom::fill(&mut random);
    uuid::Builder::from_unix_timestamp_millis(at.timestamp_millis().max(0) as u64, &random).into_uuid()
}

/// The UUID given to something made before UUIDs, from its ID: the ID's
/// minute plus its number in milliseconds, so they sort as the IDs do.
pub fn uuid_for(id: &Id) -> Uuid {
    let minute = id.time().unwrap_or_default();
    uuid_at(minute + chrono::TimeDelta::milliseconds(id.seq().min(59_999) as i64))
}

/// When a UUIDv7 was made (`None` for other versions).
pub fn uuid_time(u: &Uuid) -> Option<Timestamp> {
    if u.get_version_num() != 7 {
        return None;
    }
    let (s, ns) = u.get_timestamp()?.to_unix();
    DateTime::from_timestamp(s as i64, ns)
}

/// Hands out UUIDv7s in order: one made in the same millisecond as the last
/// (or a clock gone back) takes the millisecond after it.
#[derive(Default, Debug)]
pub struct UuidGen {
    last_ms: i64,
}

impl UuidGen {
    pub fn next(&mut self, at: Timestamp) -> Uuid {
        let ms = at.timestamp_millis().max(self.last_ms + 1);
        self.last_ms = ms;
        uuid_at(DateTime::from_timestamp_millis(ms).unwrap_or(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32, m: u32, s: u32) -> Timestamp {
        Utc.with_ymd_and_hms(2025, 1, 30, h, m, s).unwrap()
    }

    #[test]
    fn uuids_are_v7_in_order() {
        let mut g = UuidGen::default();
        let at = t(23, 45, 12);
        let a = g.next(at);
        let b = g.next(at);
        assert_eq!(a.get_version_num(), 7);
        assert!(a < b && a.to_string() < b.to_string());
        assert_eq!(uuid_time(&a), Some(at));
        assert_eq!(uuid_time(&b), Some(at + chrono::TimeDelta::milliseconds(1)));
        // a clock gone back still gives a later one
        assert!(g.next(t(23, 0, 0)) > b);
        // a backfilled one: the ID's minute, its number in milliseconds
        let old = Id::new(at, 7);
        assert_eq!(uuid_time(&uuid_for(&old)), Some(t(23, 45, 0) + chrono::TimeDelta::milliseconds(7)));
        assert!(uuid_for(&old) < uuid_for(&Id::new(at, 8)));
    }

    #[test]
    fn format_and_parse() {
        let id = Id::new(t(23, 45, 12), 1);
        assert_eq!(id.as_str(), "2025.01.30-23.45-0001");
        assert_eq!(id.minute(), "2025.01.30-23.45");
        assert_eq!(id.seq(), 1);
        assert_eq!(id.time(), Some(t(23, 45, 0)));
        assert_eq!("2025.01.30-23.45-0001".parse::<Id>().unwrap(), id);
        assert_eq!("2025.01.30-23.45-10000".parse::<Id>().unwrap().seq(), 10000);
        for bad in [
            "",
            "2025.01.30-23.45-001",
            "2025.13.30-23.45-0001",
            "2025.01.30-24.00-0000",
            "2025-01-30-23.45-0001",
            "2025.01.30-23.45-00a1",
            "2025.02.30-23.45-0000",
        ] {
            assert!(bad.parse::<Id>().is_err(), "{bad}");
        }
    }

    #[test]
    fn allocation_is_per_kind_and_minute() {
        let mut a = IdAllocator::default();
        assert_eq!(a.allocate(IdKind::Idea, t(9, 0, 0)).as_str(), "2025.01.30-09.00-0000");
        assert_eq!(a.allocate(IdKind::Idea, t(9, 0, 59)).as_str(), "2025.01.30-09.00-0001");
        assert_eq!(a.allocate(IdKind::Run, t(9, 0, 1)).as_str(), "2025.01.30-09.00-0000");
        assert_eq!(a.allocate(IdKind::Idea, t(9, 1, 0)).as_str(), "2025.01.30-09.01-0000");
        a.observe(IdKind::Project, &"2025.01.30-09.00-0007".parse().unwrap());
        a.observe(IdKind::Project, &"2025.01.30-09.00-0003".parse().unwrap());
        assert_eq!(a.allocate(IdKind::Project, t(9, 0, 0)).as_str(), "2025.01.30-09.00-0008");
    }

    #[test]
    fn ids_sort_by_time() {
        let mut ids = [Id::new(t(10, 0, 0), 0), Id::new(t(9, 59, 0), 1), Id::new(t(9, 59, 0), 0)];
        ids.sort();
        let s: Vec<_> = ids.iter().map(Id::as_str).collect();
        assert_eq!(s, ["2025.01.30-09.59-0000", "2025.01.30-09.59-0001", "2025.01.30-10.00-0000"]);
    }
}
