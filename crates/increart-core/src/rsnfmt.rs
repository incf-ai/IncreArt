//! RSN reading and writing in the library's style: 4-space indentation and
//! `Option` values written bare (DESIGN.md, File format: RSN).

use std::borrow::Cow;
use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Serialize, Serializer};

use crate::error::{Error, IoContext, Result};
use crate::fsutil;

/// `value` as RSN (an error only from a type's own `Serialize`).
pub fn to_string<T: Serialize>(value: &T) -> Result<String, String> {
    let mut config = rsn::ser::Config::new();
    config.writer = rsn::writer::Config::Pretty { indentation: "    ".into(), newline: "\n".into() };
    config.serialize(value).map_err(|e| e.to_string())
}

pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, rsn::de::Error> {
    rsn::from_str(&lf(text))
}

pub fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = lf(&fs::read_to_string(path).at(path)?).into_owned();
    rsn::from_str(&text).map_err(|e| Error::format(path, describe(&text, &e)))
}

/// `text` with CRLF line ends turned into LF: rsn rejects a `\r`, and files
/// saved on Windows (or checked out there) have them.
fn lf(text: &str) -> Cow<'_, str> {
    if text.contains('\r') { Cow::Owned(text.replace("\r\n", "\n")) } else { Cow::Borrowed(text) }
}

/// Writes `value` atomically (temporary file + rename).
pub fn write<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = to_string(value).map_err(|e| Error::encode(path.display(), e))?;
    fsutil::write_atomic(path, format!("{text}\n").as_bytes())
}

/// `line:col: error` for an RSN parse error.
pub fn describe(text: &str, e: &rsn::de::Error) -> String {
    let at = e.location.start.min(text.len());
    let line = text[..at].matches('\n').count() + 1;
    let col = at - text[..at].rfind('\n').map_or(0, |i| i + 1) + 1;
    format!("{line}:{col}: {:?}", e.kind)
}

/// `serialize_with` for `Option` fields: `Some(x)` is written as `x`, `None`
/// as `None`. rsn reads a bare value back as `Some`.
pub fn bare<T: Serialize, S: Serializer>(value: &Option<T>, s: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(v) => v.serialize(s),
        None => s.serialize_none(),
    }
}

/// `deserialize_with` for a list of structs read whatever name each is
/// written with, for a type renamed in the files (`RecipePreset` →
/// `ModelPreset`): older files keep loading.
pub fn any_names<'de, T: serde::Deserialize<'de>, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(<Vec<AnyName<T>> as serde::Deserialize>::deserialize(d)?.into_iter().map(|a| a.0).collect())
}

/// A struct read whatever name it's written with (see [`any_names`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnyName<T>(pub T);

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for AnyName<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Any<T>(std::marker::PhantomData<T>);
        impl<'de, T: serde::Deserialize<'de>> serde::de::Visitor<'de> for Any<T> {
            type Value = T;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a struct")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
                T::deserialize(serde::de::value::MapAccessDeserializer::new(map))
            }
        }
        // rsn checks a struct's name, not a map's
        d.deserialize_map(Any(std::marker::PhantomData)).map(AnyName)
    }
}

/// As [`write`], with short lists and tuples of plain values on one line
/// (`gpu: Rocm(0)`, `devices: [1, 2]`), for files people edit by hand.
pub fn write_compact<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = to_string(value).map_err(|e| Error::encode(path.display(), e))?;
    fsutil::write_atomic(path, format!("{}\n", compact(&text)).as_bytes())
}

/// `text` (as [`to_string`] writes it) with each list or tuple of plain
/// values that fits in [`COMPACT_WIDTH`] columns joined onto one line,
/// innermost first.
pub fn compact(text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let opener = |l: &str| l.ends_with('(') || l.ends_with('[') || l.ends_with('{');
    let closer = |l: &str| l.trim_start().starts_with([')', ']', '}']);
    let mut i = 0;
    while i < lines.len() {
        let close = match lines[i].chars().last() {
            Some('(') => ')',
            Some('[') => ']',
            _ => {
                i += 1;
                continue;
            }
        };
        let end = (i + 1..lines.len()).find(|&j| opener(&lines[j]) || closer(&lines[j]));
        let Some(end) = end.filter(|&j| lines[j].trim_start().starts_with(close)) else {
            i += 1;
            continue;
        };
        let items: Vec<&str> = lines[i + 1..end].iter().map(|l| l.trim().trim_end_matches(',')).collect();
        let joined = format!("{}{}{}", lines[i], items.join(", "), lines[end].trim_start());
        if joined.chars().count() > COMPACT_WIDTH {
            i += 1;
            continue;
        }
        lines.splice(i..=end, [joined]);
        // the line holding it may now be a plain value of an outer one
        i = i.saturating_sub(1);
        while i > 0 && !opener(&lines[i]) {
            i -= 1;
        }
    }
    lines.join("\n")
}

/// The widest line [`compact`] makes.
pub const COMPACT_WIDTH: usize = 100;
