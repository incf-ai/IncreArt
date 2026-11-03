//! Export…: copies of library images in a folder outside it (DESIGN.md,
//! Export), named by a template (`{project}_{run}_r{rev}_s{seed:4}`), flat
//! or in the template's folders, as stored or converted, with or without
//! the metadata, an idea's text beside its images. The library is
//! unchanged. The choices made last are kept in `export.rsn` in the app's
//! state folder.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::TileFormat;
use crate::model::{ImageRef, StoreFormat};

pub const FILE: &str = "export.rsn";

/// `export.rsn` in the app's state folder.
pub fn path() -> Option<PathBuf> {
    crate::config::user_dir("XDG_STATE_HOME", ".local/state", "LOCALAPPDATA").map(|d| d.join(FILE))
}

/// A token's value for one image, by token name; a token with none is
/// left out of the name.
pub type Fields = BTreeMap<&'static str, String>;

/// The tokens a name template can use: name, what it is, and whether it is
/// for outputs (`Some(true)`), inputs (`Some(false)`) or both.
pub const TOKENS: &[(&str, &str, Option<bool>)] = &[
    ("project", "the project's name", Some(true)),
    ("run", "the run's ID", Some(true)),
    ("rev", "the run's revision", Some(true)),
    ("seed", "the seed", Some(true)),
    ("rating", "good, neutral, bad or unrated", Some(true)),
    ("prompt", "the prompt as written", Some(true)),
    ("model", "the model", Some(true)),
    ("idea", "the idea's title (else its text); an output's source idea", None),
    ("idea_id", "the idea's ID", None),
    ("idea_n", "the image's number in its idea", Some(false)),
    ("source", "where the idea came from (clipboard, dropped, …)", Some(false)),
    ("title", "the image's title (an input: its idea's)", None),
    ("tags", "its tags", None),
    ("id", "the image's ID", None),
    ("uuid", "the image's UUID", None),
    ("idea_uuid", "the idea's UUID", None),
    ("date", "when it was made, YYYY-MM-DD", None),
    ("time", "when it was made, HH.MM", None),
    ("width", "width in pixels", None),
    ("height", "height in pixels", None),
    ("n", "its place in this export, from 1", None),
];

/// The built-in name templates for outputs, the default first.
pub const OUTPUT_TEMPLATES: &[&str] = &[
    "{project}_{run}_r{rev}_s{seed:4}",
    "{project}/{rating}/{run}_r{rev}_s{seed:4}",
    "{date}_{project}_{n:3}",
    "{title}_{seed}",
    "{id}",
];

/// The built-in name templates for inputs (ideas' images), the default first.
pub const INPUT_TEMPLATES: &[&str] = &["{idea}_{idea_n}", "{date}_{idea}_{idea_n}", "{idea}/{idea_n}", "{id}"];

/// What to do when a file of that name is in the destination already.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "Existing")]
pub enum Existing {
    /// The new one gets a number: `name 2.png`.
    #[default]
    Number,
    Skip,
    Overwrite,
}

/// The export dialog's choices, kept between exports and sessions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "ExportPrefs", default)]
pub struct Prefs {
    pub dest: String,
    /// The name template last used for outputs, and for inputs.
    pub outputs: String,
    pub inputs: String,
    /// Templates saved by the user, besides the built-in ones.
    pub saved: Vec<String>,
    /// Folders from the template's `/` (else every file in `dest`).
    pub folders: bool,
    /// None: as stored (PNG or WebP).
    pub format: Option<TileFormat>,
    /// For JPEG and AVIF, percent.
    pub quality: u8,
    pub keep_metadata: bool,
    /// An idea's text and tags as a `.txt` beside its images.
    pub idea_text: bool,
    pub existing: Existing,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            dest: String::new(),
            outputs: OUTPUT_TEMPLATES[0].into(),
            inputs: INPUT_TEMPLATES[0].into(),
            saved: vec![],
            folders: true,
            format: None,
            quality: 90,
            keep_metadata: true,
            idea_text: false,
            existing: Existing::Number,
        }
    }
}

impl Prefs {
    /// The defaults when there's none or it can't be read.
    pub fn load(path: &Path) -> Prefs {
        crate::rsnfmt::read(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> crate::error::Result<()> {
        crate::rsnfmt::write(path, self)
    }

    /// Whether the metadata can be kept in the chosen format: PNG and WebP
    /// carry it over, JPEG and AVIF are written without.
    pub fn metadata_possible(&self) -> bool {
        matches!(self.format.map(TileFormat::usable), None | Some(TileFormat::Png | TileFormat::Webp))
    }

    /// The extension a file is written with, `stored` its store format.
    pub fn ext(&self, stored: StoreFormat) -> &'static str {
        match self.format {
            None => stored.ext(),
            Some(f) => f.usable().ext(),
        }
    }
}

/// One thing exported: an image, an idea's text, or both.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    /// None: an idea without images (only its text is written, and only
    /// with `idea_text`).
    pub image: Option<ImageRef>,
    /// Its store format (the extension when exported as stored).
    pub stored: StoreFormat,
    pub fields: Fields,
    /// A name given to this one (Save as), used instead of the template's.
    pub name: Option<String>,
    /// An idea's `.txt` (on its first image), written with `idea_text`.
    pub text: Option<String>,
}

/// Fills in what only the files say: each image's store format, and the
/// size of one that has none yet (an input's), read from its header, and
/// its UUID.
pub fn read_files(lib: &crate::Library, items: &mut [Item]) {
    for it in items {
        let Some(img) = &it.image else { continue };
        it.stored = lib.format_of(img);
        if !it.fields.contains_key("width")
            && let Ok((w, h)) = image::image_dimensions(lib.image_path(img))
        {
            it.fields.insert("width", w.to_string());
            it.fields.insert("height", h.to_string());
        }
        if let Some(u) = lib.image_uuid(img) {
            it.fields.insert("uuid", u.to_string());
        }
    }
}

/// Exports `items` from `lib` into `dest`: planned against what is there
/// now, then written ([`write`]); the library is locked only to find the
/// store files.
pub fn export(
    lib: &std::sync::Mutex<crate::Library>,
    items: &[Item],
    template: &str,
    prefs: &Prefs,
    dest: &Path,
    progress: impl FnMut(usize, usize) -> bool,
) -> Result<Summary, String> {
    let mut items = items.to_vec();
    let sources: Vec<Option<PathBuf>> = {
        let l = lib.lock().unwrap_or_else(|e| e.into_inner());
        for it in &mut items {
            if let Some(img) = &it.image {
                it.stored = l.format_of(img);
            }
        }
        items.iter().map(|it| it.image.as_ref().map(|img| l.image_path(img))).collect()
    };
    let planned = plan(&items, template, prefs, |p| dest.join(p).exists());
    let skipped = planned.iter().filter(|p| p.skip).count();
    let jobs: Vec<Job> = planned
        .into_iter()
        .filter(|p| !p.skip)
        .map(|p| Job { from: sources[p.item].clone(), image: p.image, text: p.text.zip(items[p.item].text.clone()) })
        .collect();
    let mut s = write(dest, &jobs, prefs, progress)?;
    s.skipped = skipped;
    Ok(s)
}

/// An idea's `.txt`: its title, text and tags.
pub fn idea_text(title: &str, text: &str, tags: &[String]) -> String {
    let mut parts = vec![];
    if !title.trim().is_empty() {
        parts.push(title.trim().to_string());
    }
    if !text.trim().is_empty() {
        parts.push(text.trim().to_string());
    }
    if !tags.is_empty() {
        parts.push(format!("tags: {}", tags.join(", ")));
    }
    format!("{}\n", parts.join("\n\n"))
}

// ------------------------------------------------------------------ template

enum Part<'a> {
    Lit(&'a str),
    Token { name: &'a str, width: Option<usize> },
}

/// `{name}` and `{name:N}` are tokens; any other `{` is text.
fn parse(template: &str) -> Vec<Part<'_>> {
    let mut parts = vec![];
    let mut rest = template;
    while !rest.is_empty() {
        let Some(open) = rest.find('{') else {
            parts.push(Part::Lit(rest));
            break;
        };
        let token = rest[open + 1..].find('}').and_then(|close| {
            let inner = &rest[open + 1..open + 1 + close];
            let (name, width) = match inner.split_once(':') {
                Some((n, w)) => (n, Some(w.parse::<usize>().ok()?)),
                None => (inner, None),
            };
            let ok = !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
            ok.then_some((Part::Token { name, width }, open + close + 2))
        });
        match token {
            Some((t, end)) => {
                if open > 0 {
                    parts.push(Part::Lit(&rest[..open]));
                }
                parts.push(t);
                rest = &rest[end..];
            }
            None => {
                parts.push(Part::Lit(&rest[..open + 1]));
                rest = &rest[open + 1..];
            }
        }
    }
    parts
}

/// The tokens in `template` that aren't tokens ([`TOKENS`]).
pub fn unknown_tokens(template: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for p in parse(template) {
        if let Part::Token { name, .. } = p
            && !TOKENS.iter().any(|t| t.0 == name)
            && !out.iter().any(|o| o == name)
        {
            out.push(name.into());
        }
    }
    out
}

/// Whether `template` makes folders (has a `/` outside its tokens).
pub fn has_folders(template: &str) -> bool {
    parse(template).iter().any(|p| matches!(p, Part::Lit(s) if s.contains(['/', '\\'])))
}

/// What separates a name's parts: dropped beside a token with no value,
/// and from a name's ends.
const SEPARATORS: &[char] = &['_', '-', '.', ' ', ','];

/// A component longer than this (in characters) is cut.
const MAX_COMPONENT: usize = 120;

/// A value in a file name: no path or forbidden characters, whitespace
/// as one space.
fn clean_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => out.push('_'),
            c if c.is_whitespace() || c.is_control() => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }
    out.trim().to_string()
}

/// A token's value, `width` padding a number with zeros, cutting text.
fn value(fields: &Fields, n: usize, name: &str, width: Option<usize>) -> String {
    let v = match name {
        "n" => n.to_string(),
        _ => clean_value(fields.get(name).map_or("", String::as_str)),
    };
    match width {
        Some(w) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => format!("{v:0>w$}"),
        Some(w) => v.chars().take(w).collect::<String>().trim_end().to_string(),
        None => v,
    }
}

/// A file or folder name made safe: the ends' separators trimmed, a
/// separator repeated once, no name Windows reserves, at most
/// [`MAX_COMPONENT`] characters.
fn clean_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if SEPARATORS.contains(&c) && out.ends_with(c) {
            continue;
        }
        out.push(c);
    }
    let mut out: String = out.trim_matches(SEPARATORS).chars().take(MAX_COMPONENT).collect();
    out = out.trim_end_matches(SEPARATORS).to_string();
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.push('_');
    }
    out
}

/// A name typed for one file (Save as) made safe, without an image's or a
/// text's extension if it was typed with one. None when nothing is left.
pub fn file_name(name: &str) -> Option<String> {
    let name = name.trim();
    let stem = match name.rsplit_once('.') {
        Some((stem, ext)) if ["png", "webp", "jpg", "jpeg", "avif", "txt"].contains(&ext.to_lowercase().as_str()) => {
            stem
        }
        _ => name,
    };
    Some(clean_component(&clean_value(stem))).filter(|s| !s.is_empty())
}

/// The name `template` gives an image (its `fields`, `n`th in the
/// export), without the extension: folders joined by `/` with `folders`,
/// else by `_`. A token with no value is left out with the separator
/// beside it. None when nothing is left.
pub fn render(template: &str, fields: &Fields, n: usize, folders: bool) -> Option<String> {
    // (text, a token with no value)
    let mut pieces: Vec<(String, bool)> = parse(template)
        .into_iter()
        .map(|p| match p {
            Part::Lit(s) => (s.to_string(), false),
            Part::Token { name, width } => {
                let v = value(fields, n, name, width);
                let empty = v.is_empty();
                (v, empty)
            }
        })
        .collect();
    let lits: Vec<bool> = parse(template).iter().map(|p| matches!(p, Part::Lit(_))).collect();
    // text between two tokens with no value goes with them (`{width}x{height}`)
    for i in 1..pieces.len().saturating_sub(1) {
        if lits[i] && pieces[i - 1].1 && pieces[i + 1].1 {
            pieces[i].0.clear();
        }
    }
    let only_separators = |s: &str| !s.is_empty() && s.chars().all(|c| SEPARATORS.contains(&c));
    for i in 0..pieces.len() {
        if !pieces[i].1 {
            continue;
        }
        if i > 0 && only_separators(&pieces[i - 1].0) {
            pieces[i - 1].0.clear();
        } else if i + 1 < pieces.len() && only_separators(&pieces[i + 1].0) {
            pieces[i + 1].0.clear();
        }
    }
    let joined: String = pieces.into_iter().map(|p| p.0).collect();
    let parts: Vec<String> =
        joined.split(['/', '\\']).map(clean_component).filter(|c| !c.is_empty() && c != "." && c != "..").collect();
    if parts.is_empty() {
        return None;
    }
    Some(if folders { parts.join("/") } else { clean_component(&parts.join("_")) })
}

// ------------------------------------------------------------------ plan

/// Where one item goes, relative to the destination.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub item: usize,
    /// The image's file (None: only the text is written).
    pub image: Option<PathBuf>,
    /// The idea's `.txt`, when written.
    pub text: Option<PathBuf>,
    /// A file of that name is there already: left (`Skip`) or replaced
    /// (`Overwrite`).
    pub skip: bool,
    pub replaces: bool,
    /// Its name was another's (in this export or in the destination) and
    /// got a number.
    pub numbered: bool,
}

/// Where each item goes: named by `template` (one given a name: by
/// that; an item whose name comes out empty: by its ID), a name taken by an earlier item numbered (`name
/// 2.png`), one already in the destination (`exists`, relative paths)
/// numbered, skipped or replaced as `prefs.existing` says. Text-only items
/// are left out without `prefs.idea_text`. Names are compared ignoring
/// case (Windows and macOS do).
pub fn plan(items: &[Item], template: &str, prefs: &Prefs, exists: impl Fn(&Path) -> bool) -> Vec<Planned> {
    let mut taken: HashSet<String> = HashSet::new();
    let key = |p: &Path| p.to_string_lossy().to_lowercase();
    let mut out = vec![];
    for (i, item) in items.iter().enumerate() {
        let text = prefs.idea_text && item.text.is_some();
        if item.image.is_none() && !text {
            continue;
        }
        let n = out.len() + 1;
        let stem = (item.name.as_deref().and_then(file_name))
            .or_else(|| render(template, &item.fields, n, prefs.folders))
            .or_else(|| item.fields.get("id").map(|id| clean_component(id)).filter(|s| !s.is_empty()))
            .unwrap_or_else(|| format!("export {n}"));
        let ext = prefs.ext(item.stored);
        let paths = |k: usize| {
            let stem = if k == 1 { stem.clone() } else { format!("{stem} {k}") };
            let image = item.image.as_ref().map(|_| PathBuf::from(format!("{stem}.{ext}")));
            let text = text.then(|| PathBuf::from(format!("{stem}.txt")));
            (image, text)
        };
        let mut k = 1;
        let (mut skip, mut replaces) = (false, false);
        let (image, text) = loop {
            let (image, text) = paths(k);
            let all: Vec<&PathBuf> = image.iter().chain(&text).collect();
            if all.iter().any(|p| taken.contains(&key(p))) {
                k += 1;
                continue;
            }
            if all.iter().any(|p| exists(p)) {
                match prefs.existing {
                    Existing::Number => {
                        k += 1;
                        continue;
                    }
                    Existing::Skip => skip = true,
                    Existing::Overwrite => replaces = true,
                }
            }
            break (image, text);
        };
        for p in image.iter().chain(&text) {
            taken.insert(key(p));
        }
        out.push(Planned { item: i, image, text, skip, replaces, numbered: k > 1 });
    }
    out
}

// ------------------------------------------------------------------ write

/// A store file as exported: as stored or converted, with or without the
/// metadata (JPEG and AVIF never have it).
pub fn convert(bytes: &[u8], format: Option<TileFormat>, quality: u8, keep_metadata: bool) -> Result<Vec<u8>, String> {
    let store = |f: StoreFormat| -> Result<Vec<u8>, String> {
        let out = if crate::container::format_of(bytes) == Some(f) {
            bytes.to_vec()
        } else {
            let (got, out, _) = crate::container::transcode(bytes, f)?;
            if got != f {
                return Err(format!("too large for {} (at most {} on a side)", f.name(), crate::webp::MAX_SIDE));
            }
            out
        };
        if keep_metadata { Ok(out) } else { crate::container::strip_metadata(&out) }
    };
    match format.map(TileFormat::usable) {
        None => match crate::container::format_of(bytes) {
            Some(f) => store(f),
            None => Err("neither PNG nor WebP".into()),
        },
        Some(TileFormat::Png) => store(StoreFormat::Png),
        Some(TileFormat::Webp) => store(StoreFormat::Webp),
        Some(f) => {
            let px = crate::import::decode_pixels(bytes).map_err(|e| e.to_string())?;
            let img = match px.channels {
                4 => image::RgbaImage::from_raw(px.width, px.height, px.data),
                _ => image::RgbImage::from_raw(px.width, px.height, px.data)
                    .map(|i| image::DynamicImage::ImageRgb8(i).to_rgba8()),
            }
            .ok_or("pixels don't fill the image")?;
            crate::export::encode(&img, f, quality)
        }
    }
}

/// One file to write: an image (`from` its store file) and/or a text.
#[derive(Clone, Debug)]
pub struct Job {
    pub from: Option<PathBuf>,
    pub image: Option<PathBuf>,
    pub text: Option<(PathBuf, String)>,
}

/// What an export did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// Images written (texts not counted).
    pub images: usize,
    pub texts: usize,
    pub skipped: usize,
    /// One line per file that couldn't be written.
    pub failed: Vec<String>,
    /// Stopped before the end.
    pub stopped: bool,
}

/// Writes `jobs` (paths relative to `dest`; skipped ones left out by the
/// caller), each image read from its store file and converted. A file that
/// fails is noted and the rest go on. `progress(done, total)` is asked
/// after each; false stops.
pub fn write(
    dest: &Path,
    jobs: &[Job],
    prefs: &Prefs,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<Summary, String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut s = Summary::default();
    let keep = prefs.keep_metadata && prefs.metadata_possible();
    for (i, job) in jobs.iter().enumerate() {
        if !progress(i, jobs.len()) {
            s.stopped = true;
            break;
        }
        let parent = |rel: &Path| -> Result<PathBuf, String> {
            let to = dest.join(rel);
            if let Some(dir) = to.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            Ok(to)
        };
        if let (Some(from), Some(rel)) = (&job.from, &job.image) {
            let r = std::fs::read(from)
                .map_err(|e| format!("{}: {e}", from.display()))
                .and_then(|b| {
                    convert(&b, prefs.format, prefs.quality, keep).map_err(|e| format!("{}: {e}", rel.display()))
                })
                .and_then(|b| {
                    let to = parent(rel)?;
                    std::fs::write(&to, b).map_err(|e| format!("{}: {e}", to.display()))
                });
            match r {
                Ok(()) => s.images += 1,
                Err(e) => {
                    s.failed.push(e);
                    continue;
                }
            }
        }
        if let Some((rel, text)) = &job.text {
            match parent(rel).and_then(|to| std::fs::write(&to, text).map_err(|e| format!("{}: {e}", to.display()))) {
                Ok(()) => s.texts += 1,
                Err(e) => s.failed.push(e),
            }
        }
    }
    progress(jobs.len(), jobs.len());
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&'static str, &str)]) -> Fields {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    fn output() -> Fields {
        fields(&[
            ("project", "Cats"),
            ("run", "2026.11.03-10.00-0001"),
            ("rev", "2"),
            ("seed", "42"),
            ("rating", "good"),
            ("id", "2026.11.03-10.05-0007"),
        ])
    }

    #[test]
    fn tokens_widths_and_unknown_tokens() {
        assert_eq!(
            render(OUTPUT_TEMPLATES[0], &output(), 1, true).unwrap(),
            "Cats_2026.11.03-10.00-0001_r2_s0042",
            "the old run / revision / seed names, with the project"
        );
        let f = fields(&[("prompt", "a cat: on a <mat>,\n sleeping")]);
        assert_eq!(render("{prompt:12}", &f, 1, true).unwrap(), "a cat_ on a", "cut, made safe");
        assert_eq!(render("{n:3}", &f, 7, true).unwrap(), "007");
        assert_eq!(render("{ {x} {nope", &f, 1, true).unwrap(), "{ {nope", "not tokens: text");
        assert_eq!(unknown_tokens("{project}_{nope}_{seed:x}_{nope}"), vec!["nope".to_string()]);
    }

    #[test]
    fn a_token_with_no_value_goes_with_its_separator() {
        let f = fields(&[("project", "Cats"), ("seed", "42")]);
        assert_eq!(render("{project}_{title}_{seed}", &f, 1, true).unwrap(), "Cats_42");
        assert_eq!(render("{title} - {project}", &f, 1, true).unwrap(), "Cats");
        assert_eq!(render("{project}_{title}", &f, 1, true).unwrap(), "Cats");
        assert_eq!(render("{title}", &f, 1, true), None, "nothing left");
        assert_eq!(render("{project}_{width}x{height}", &f, 1, true).unwrap(), "Cats", "text between two goes too");
    }

    #[test]
    fn folders_or_flat() {
        let t = OUTPUT_TEMPLATES[1];
        assert_eq!(render(t, &output(), 1, true).unwrap(), "Cats/good/2026.11.03-10.00-0001_r2_s0042");
        assert_eq!(render(t, &output(), 1, false).unwrap(), "Cats_good_2026.11.03-10.00-0001_r2_s0042");
        let f = fields(&[("project", "../a/b"), ("seed", "1")]);
        assert_eq!(render("{project}/{seed}", &f, 1, true).unwrap(), "a_b/1", "a value never makes a folder");
        assert_eq!(render("../{seed}", &f, 1, true).unwrap(), "1", "nor leaves the destination");
        assert_eq!(render("con", &f, 1, true).unwrap(), "con_", "a name Windows reserves");
        assert!(has_folders(t) && !has_folders(OUTPUT_TEMPLATES[0]));
    }

    fn item(id: &str, text: Option<&str>) -> Item {
        Item {
            image: Some(ImageRef::output(id.parse().unwrap())),
            fields: fields(&[("id", id), ("project", "Cats")]),
            text: text.map(Into::into),
            ..Item::default()
        }
    }

    #[test]
    fn clashes_numbered_skipped_or_replaced() {
        let items = [item("2026.11.03-10.05-0001", None), item("2026.11.03-10.05-0002", None)];
        let mut prefs = Prefs::default();
        let there = |p: &Path| p == Path::new("Cats.png");
        let names = |plan: &[Planned]| plan.iter().map(|p| p.image.clone().unwrap()).collect::<Vec<_>>();

        let p = plan(&items, "{project}", &prefs, |_| false);
        assert_eq!(names(&p), [PathBuf::from("Cats.png"), PathBuf::from("Cats 2.png")]);
        assert!(!p[0].numbered && p[1].numbered);

        let p = plan(&items, "{project}", &prefs, there);
        assert_eq!(names(&p), [PathBuf::from("Cats 2.png"), PathBuf::from("Cats 3.png")], "the file there kept");

        prefs.existing = Existing::Skip;
        let p = plan(&items, "{project}", &prefs, there);
        assert!(p[0].skip && !p[1].skip, "only the one whose name is there");
        assert_eq!(names(&p)[1], PathBuf::from("Cats 2.png"), "two in one export are both kept");

        prefs.existing = Existing::Overwrite;
        assert!(plan(&items, "{project}", &prefs, there)[0].replaces);

        prefs.format = Some(TileFormat::Jpeg);
        assert_eq!(
            plan(&items, "{title}", &prefs, |_| false)[0].image,
            Some("2026.11.03-10.05-0001.jpg".into()),
            "by ID"
        );
    }

    #[test]
    fn a_name_given_instead_of_the_template() {
        let mut it = item("2026.11.03-10.05-0001", None);
        it.name = Some(" my {cat}__on/a mat.PNG ".into());
        let prefs = Prefs { format: Some(TileFormat::Webp), ..Prefs::default() };
        let p =
            plan(std::slice::from_ref(&it), "{project}/{run}", &prefs, |p| p == Path::new("my {cat}_on_a mat.webp"));
        assert_eq!(p[0].image, Some("my {cat}_on_a mat 2.webp".into()), "as typed, made safe, numbered");
        assert_eq!(file_name("cat.jpeg"), Some("cat".into()), "a typed extension dropped");
        assert_eq!(file_name("v1.2"), Some("v1.2".into()));
        it.name = Some(" .png".into());
        assert_eq!(
            plan(&[it], "{project}", &prefs, |_| false)[0].image,
            Some("Cats.webp".into()),
            "empty: the template"
        );
    }

    #[test]
    fn idea_texts_only_when_asked() {
        let mut lone = item("2026.11.03-10.05-0003", Some("a cat"));
        lone.image = None;
        let items = [item("2026.11.03-10.05-0001", Some("a dog")), item("2026.11.03-10.05-0002", None), lone];
        let mut prefs = Prefs::default();
        let p = plan(&items, "{id}", &prefs, |_| false);
        assert_eq!(p.len(), 2, "the idea without images left out");
        assert!(p.iter().all(|p| p.text.is_none()));
        prefs.idea_text = true;
        let p = plan(&items, "{id}", &prefs, |_| false);
        assert_eq!(p[0].text, Some("2026.11.03-10.05-0001.txt".into()), "beside its first image");
        assert_eq!((p[2].image.clone(), p[2].text.clone()), (None, Some("2026.11.03-10.05-0003.txt".into())));
        // a .txt there numbers the image too
        let p = plan(&items[..1], "{id}", &prefs, |p| p == Path::new("2026.11.03-10.05-0001.txt"));
        assert_eq!(p[0].image, Some("2026.11.03-10.05-0001 2.png".into()));
        assert_eq!(idea_text("", "a dog", &["pets".into()]), "a dog\n\ntags: pets\n");
    }

    #[test]
    fn converted_with_or_without_metadata() {
        let px = crate::pixel::Pixels { width: 4, height: 3, channels: 3, data: vec![200; 36] };
        let chunk = crate::png::Chunk::text("increart:recipe", "x");
        let (_, png) = crate::container::encode(StoreFormat::Png, &px, std::slice::from_ref(&chunk)).unwrap();
        let has = |b: &[u8]| crate::container::text(b, "increart:recipe").unwrap().is_some();

        assert_eq!(convert(&png, None, 90, true).unwrap(), png, "as stored: the same file");
        assert!(!has(&convert(&png, None, 90, false).unwrap()));
        let webp = convert(&png, Some(TileFormat::Webp), 90, true).unwrap();
        assert_eq!(crate::container::format_of(&webp), Some(StoreFormat::Webp));
        assert!(has(&webp), "carried over");
        let jpeg = convert(&png, Some(TileFormat::Jpeg), 90, true).unwrap();
        assert_eq!(image::guess_format(&jpeg).unwrap(), image::ImageFormat::Jpeg);
    }

    #[test]
    fn written_into_folders_a_failure_noted() {
        let dir = tempfile::tempdir().unwrap();
        let px = crate::pixel::Pixels { width: 2, height: 2, channels: 3, data: vec![9; 12] };
        let (_, png) = crate::container::encode(StoreFormat::Png, &px, &[]).unwrap();
        let src = dir.path().join("src.png");
        std::fs::write(&src, &png).unwrap();
        let dest = dir.path().join("out");
        let jobs = [
            Job { from: Some(src), image: Some("a/b.png".into()), text: Some(("a/b.txt".into(), "hi\n".into())) },
            Job { from: Some(dir.path().join("gone.png")), image: Some("c.png".into()), text: None },
        ];
        let mut seen = vec![];
        let s = write(&dest, &jobs, &Prefs::default(), |d, t| {
            seen.push((d, t));
            true
        })
        .unwrap();
        assert_eq!((s.images, s.texts, s.failed.len(), s.stopped), (1, 1, 1, false));
        assert_eq!(std::fs::read(dest.join("a/b.png")).unwrap(), png);
        assert_eq!(std::fs::read_to_string(dest.join("a/b.txt")).unwrap(), "hi\n");
        assert_eq!(seen.last(), Some(&(2, 2)));

        let s = write(&dest, &jobs, &Prefs::default(), |_, _| false).unwrap();
        assert!(s.stopped && s.images == 0);
    }

    #[test]
    fn prefs_saved_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE);
        let prefs = Prefs {
            dest: "/tmp/x".into(),
            saved: vec!["{seed}".into()],
            format: Some(TileFormat::Avif),
            existing: Existing::Skip,
            ..Prefs::default()
        };
        prefs.save(&p).unwrap();
        assert_eq!(Prefs::load(&p), prefs);
        // a field no longer there (one_name) is ignored, the rest kept
        let old = std::fs::read_to_string(&p).unwrap().replacen("dest:", "one_name: true,\n    dest:", 1);
        assert!(old.contains("one_name"));
        std::fs::write(&p, old).unwrap();
        assert_eq!(Prefs::load(&p), prefs);
        assert_eq!(Prefs::load(&dir.path().join("none.rsn")), Prefs::default());
        assert!(!Prefs { format: Some(TileFormat::Jpeg), ..Prefs::default() }.metadata_possible());
    }
}
