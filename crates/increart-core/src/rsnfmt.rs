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
