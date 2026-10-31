//! An open library: `library.lock` held, IDs allocated from memory, and every
//! operation that changes the library on disk.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::container;
use crate::error::{Error, IoContext, Result};
use crate::fsutil;
use crate::id::{self, Id, IdAllocator, IdKind, Timestamp};
use crate::import::{self, Original};
use crate::link;
use crate::model::*;
use crate::png::{self, Chunk, UsageCounts};
use crate::rsnfmt;
use crate::scan::{self, *};
pub use crate::scan::{IdeaLoc, Index, Verdict};

pub const LIBRARY_FILE: &str = "library.rsn";
pub const LOCK_FILE: &str = "library.lock";
pub const PIXEL_INDEX: &str = "cache/pixel-index.rsn";

const LAYOUT: &[&str] = &[
    "input_images",
    "output_images",
    IMPORTED,
    UNREVIEWED_IDEAS,
    INPUT_LIBRARY,
    DISCARDED_IDEAS,
    EDITED_IDEAS,
    PROJECTS,
    DISCARDED_PROJECTS,
    COLLECTIONS,
    "thumbs/input",
    "thumbs/output",
    "cache",
];

/// Which text a history belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HistoryKey {
    /// A project's Generate prompt.
    Project(Id),
    /// The New idea form.
    NewIdea,
    /// An idea's text (Edit idea).
    Idea(Id),
}

/// Where deleted things go: the system trash, or (for tests) a directory.
#[derive(Clone, Debug)]
pub enum Disposal {
    SystemTrash,
    Directory(PathBuf),
}

/// The three folder trees (DESIGN.md, Project folders, Input library folders,
/// collections). Paths into a tree are relative to its base; empty is the top
/// level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tree {
    Projects,
    InputLibrary,
    Collections,
}

impl Tree {
    pub fn base(self) -> &'static str {
        match self {
            Tree::Projects => PROJECTS,
            Tree::InputLibrary => INPUT_LIBRARY,
            Tree::Collections => COLLECTIONS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Node {
    Folder,
    /// A project (Projects), an idea (InputLibrary) or a link (Collections).
    Leaf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewIdea {
    pub title: String,
    pub text: String,
    pub tags: Vec<String>,
    pub source: IdeaSource,
    /// Input store images, in order.
    pub images: Vec<Id>,
    pub starred: bool,
    pub loc: IdeaLoc,
}

impl NewIdea {
    pub fn text(title: &str, text: &str) -> NewIdea {
        NewIdea {
            title: title.into(),
            text: text.into(),
            tags: vec![],
            source: IdeaSource::Manual,
            images: vec![],
            starred: false,
            loc: IdeaLoc::Unreviewed,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewRun {
    pub label: String,
    pub prompt: String,
    pub negative_prompt: String,
    pub alpha: bool,
    pub width: u32,
    pub height: u32,
    pub sample: Sample,
    pub mode: Mode,
    pub refs: Vec<ImageRef>,
    pub model: ModelSelection,
    pub source_idea: Option<Id>,
    pub first_seed: u64,
    pub seeds: u64,
    pub sweep: Option<crate::model::SweepTag>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    pub image: Id,
    /// An image with the same pixels was already in the store.
    pub reused: bool,
}

type Clock = Box<dyn Fn() -> Timestamp + Send + Sync>;

pub struct Library {
    root: PathBuf,
    meta: LibraryMeta,
    _lock: File,
    ids: IdAllocator,
    pixels: PixelIndexFile,
    clock: Clock,
    disposal: Disposal,
    problems: Vec<String>,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library").field("root", &self.root).field("meta", &self.meta).finish()
    }
}

// ------------------------------------------------------------------ open, create

impl Library {
    /// Creates a new library in `root` (missing or empty), choosing the link
    /// mode by probing unless `link_mode` is given, and opens it.
    pub fn create(root: &Path, link_mode: Option<LinkMode>) -> Result<Library> {
        if root.exists() && fs::read_dir(root).at(root)?.next().is_some() {
            return Err(Error::Refused(format!("{} is not empty", root.display())));
        }
        let root = link::absolute(root)?;
        for d in LAYOUT {
            fs::create_dir_all(root.join(d)).at(root.join(d))?;
        }
        let link_mode = match link_mode {
            Some(m) => m,
            None => link::probe(&root)?,
        };
        rsnfmt::write(
            &root.join(LIBRARY_FILE),
            &LibraryMeta {
                format: LIBRARY_FORMAT,
                link_mode: Some(link_mode),
                metadata: METADATA_VERSION,
                store_format: StoreFormat::Png,
            },
        )?;
        Library::open(&root)
    }

    /// Opens a library: takes `library.lock` (refusing if another instance
    /// holds it), seeds the ID counters and syncs the pixel index.
    pub fn open(root: &Path) -> Result<Library> {
        let root = link::absolute(root)?;
        let meta_path = root.join(LIBRARY_FILE);
        if !meta_path.exists() {
            return Err(Error::NotALibrary(root));
        }
        let meta: LibraryMeta = rsnfmt::read(&meta_path)?;
        if meta.format != LIBRARY_FORMAT && meta.format != LIBRARY_FORMAT_WEBP {
            return Err(Error::UnsupportedFormat(meta.format));
        }
        let lock_path = root.join(LOCK_FILE);
        let mut lock = OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path).at(&lock_path)?;
        // tried for a moment: a process being started elsewhere in this one
        // holds a copy of every open file (the lock too) until it execs
        let mut tries = 0;
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(fs::TryLockError::WouldBlock) if tries < 10 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(fs::TryLockError::WouldBlock) => return Err(Error::Locked(root)),
                Err(fs::TryLockError::Error(e)) => return Err(e).at(&lock_path),
            }
        }
        lock.set_len(0).at(&lock_path)?;
        writeln!(lock, "{}", std::process::id()).at(&lock_path)?;
        let mut meta = meta;
        // the folders every library has (git keeps no empty ones)
        for d in LAYOUT {
            fs::create_dir_all(root.join(d)).at(root.join(d))?;
        }
        // a shipped library (the example, from git or a zip): its links
        // copies or symlink stand-ins, made real links of the probed kind
        if meta.link_mode.is_none() {
            let mode = link::probe(&root)?;
            link::replace_stand_ins(&root, mode)?;
            meta.link_mode = Some(mode);
            rsnfmt::write(&meta_path, &meta)?;
        }

        let mut lib = Library {
            root,
            meta,
            _lock: lock,
            ids: IdAllocator::default(),
            pixels: PixelIndexFile::default(),
            clock: Box::new(id::now),
            disposal: Disposal::SystemTrash,
            problems: Vec::new(),
        };
        let ix = lib.scan()?;
        if lib.meta.metadata < 2 {
            lib.record_deleted_outputs(&ix)?;
        }
        for e in &ix.ideas {
            lib.ids.observe(IdKind::Idea, &e.idea.id);
        }
        for p in &ix.projects {
            lib.ids.observe(IdKind::Project, &p.project.id);
            for r in &p.runs {
                lib.ids.observe(IdKind::Run, &r.id);
            }
        }
        lib.problems.extend(ix.problems);
        for store in [Store::Input, Store::Output] {
            for (id, formats) in lib.store_files(store)? {
                lib.ids.observe(store_kind(store), &id);
                if formats.len() > 1 {
                    let r = ImageRef { id, store };
                    lib.problems.push(format!(
                        "{}: stored as both PNG and WebP (an interrupted conversion): convert the library again",
                        r.rel_path_as(StoreFormat::Png)
                    ));
                }
            }
        }
        lib.sync_pixel_index()?;
        if lib.meta.metadata < 1 {
            lib.upgrade_metadata()?;
        }
        if lib.meta.metadata < METADATA_VERSION {
            lib.meta.metadata = METADATA_VERSION;
            rsnfmt::write(&lib.root.join(LIBRARY_FILE), &lib.meta)?;
        }
        Ok(lib)
    }

    /// What a usage check goes through: every store image, with the counts
    /// the runs give it. [`open`](Self::open) leaves the check to the caller,
    /// as it reads every store image (seconds on a cold disk); the GUI runs
    /// it in the background, one image at a time.
    pub fn usage_check(&self) -> Result<UsageCheck> {
        let ix = scan::scan(&self.root)?;
        let used = usage_of_runs(ix.projects.iter().flat_map(|p| &p.runs));
        let mut images = Vec::new();
        for store in [Store::Input, Store::Output] {
            images.extend(self.store_ids(store)?.into_iter().map(|id| ImageRef { id, store }));
        }
        Ok(UsageCheck { root: self.root.clone(), images, used })
    }

    /// Checks one store image's `increart:usage` against the runs: a
    /// missing, torn (bad CRC) or unreadable chunk is rewritten from the runs,
    /// and a count lower than the runs show is raised. Counts are never
    /// lowered (they count runs that ever used the image), so a run starting
    /// meanwhile can't be undone. A failure goes to [`problems`](Self::problems).
    pub fn check_usage(&mut self, check: &UsageCheck, r: &ImageRef) {
        let derived = check.used.get(r).copied().unwrap_or_default();
        let path = self.image_path(r);
        if !path.exists() {
            return; // removed since the check began
        }
        let res = container::find_header_chunk(&path, png::KEY_USAGE).and_then(|found| match stored_usage(&found) {
            Some(s) if s.max(derived) == s => Ok(()),
            s => self.write_usage(r, found.map(|(at, _, _)| at), s.unwrap_or_default().max(derived)),
        });
        if let Err(e) = res {
            self.problems.push(format!("usage counts of {}: {e}", path.display()));
        }
    }

    /// [`check_usage`](Self::check_usage) of every store image, in one go.
    pub fn check_all_usage(&mut self) -> Result<()> {
        let check = self.usage_check()?;
        for r in &check.images {
            self.check_usage(&check, r);
        }
        Ok(())
    }

    /// The image's `increart:usage` counts (zero when it has none).
    pub fn usage_counts(&self, r: &ImageRef) -> Result<UsageCounts> {
        let path = self.image_path(r);
        Ok(container::header_text(&path, png::KEY_USAGE)?.and_then(|t| UsageCounts::parse(&t)).unwrap_or_default())
    }

    /// Writes `counts` as the image's `increart:usage`: over the chunk at
    /// `at` in place, or (no chunk yet) by rewriting the file with one.
    fn write_usage(&self, r: &ImageRef, at: Option<u64>, counts: UsageCounts) -> Result<()> {
        let path = self.image_path(r);
        match at {
            Some(at) => container::patch_chunk(&path, at, &counts.chunk()),
            None => {
                let bytes = fs::read(&path).at(&path)?;
                let new = container::set_chunks(&bytes, &[counts.chunk()]).map_err(|e| Error::format(&path, e))?;
                self.rewrite_image(&path, &new)
            }
        }
    }

    /// Adds one to the counts of each image a run uses (when it starts
    /// generating). A failure is reported in [`problems`](Self::problems)
    /// rather than failing the run; the next open repairs the counts.
    fn bump_usage(&mut self, run: &Run) {
        for (r, add) in usage_of_runs([run]) {
            let path = self.image_path(&r);
            let res = container::find_header_chunk(&path, png::KEY_USAGE).and_then(|found| {
                let (at, old) = match found {
                    Some((at, c, true)) => {
                        let t = std::str::from_utf8(&c.data[png::KEY_USAGE.len() + 1..]).ok();
                        (Some(at), t.and_then(UsageCounts::parse).unwrap_or_default())
                    }
                    Some((at, _, false)) => (Some(at), UsageCounts::default()),
                    None => (None, UsageCounts::default()),
                };
                let new = UsageCounts { refs: old.refs + add.refs, starts: old.starts + add.starts };
                self.write_usage(&r, at, new)
            });
            if let Err(e) = res {
                self.problems.push(format!("usage counts of {}: {e}", path.display()));
            }
        }
    }

    /// Brings the store images' metadata up to date, once per library: adds
    /// `increart:blake3` where missing (inputs first), then output recipes'
    /// `inputs`. Only text chunks change; the pixels aren't re-encoded. An
    /// image that can't be upgraded is reported in [`problems`](Self::problems).
    fn upgrade_metadata(&mut self) -> Result<()> {
        for store in [Store::Input, Store::Output] {
            for id in self.store_ids(store)? {
                let r = ImageRef { id, store };
                if let Err(e) = self.upgrade_image(&r) {
                    self.problems.push(format!("upgrading {}: {e}", self.image_path(&r).display()));
                }
            }
        }
        Ok(())
    }

    fn upgrade_image(&self, r: &ImageRef) -> Result<()> {
        let path = self.image_path(r);
        let texts = container::header_texts(&path)?;
        let text = |key: &str| texts.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        let mut chunks = vec![];
        let mut bytes = None;
        if text(png::KEY_BLAKE3).is_none() {
            let b = fs::read(&path).at(&path)?;
            let px = import::decode_pixels(&b).map_err(|e| Error::image(&path, e))?;
            chunks.push(Chunk::text(png::KEY_BLAKE3, &px.blake3()));
            bytes = Some(b);
        }
        if let Some(t) = text(png::KEY_RECIPE) {
            let mut recipe: Recipe = rsnfmt::from_str(t).map_err(|e| Error::format(&path, rsnfmt::describe(t, &e)))?;
            if recipe.inputs.is_empty() && recipe.image_refs().next().is_some() {
                recipe.inputs = self.input_images(&recipe);
                if !recipe.inputs.is_empty() {
                    chunks.push(Chunk::itxt(png::KEY_RECIPE, &rsnfmt::to_string(&recipe)));
                }
            }
        }
        if chunks.is_empty() {
            return Ok(());
        }
        let bytes = match bytes {
            Some(b) => b,
            None => fs::read(&path).at(&path)?,
        };
        let new = container::set_chunks(&bytes, &chunks).map_err(|e| Error::format(&path, e))?;
        self.rewrite_image(&path, &new)
    }

    /// Replaces a store image's bytes (same pixels, other text chunks).
    fn rewrite_image(&self, path: &Path, new: &[u8]) -> Result<()> {
        match self.link_mode() {
            // the links share the file: rewrite it in place
            LinkMode::Hardlink => {
                let mut f = OpenOptions::new().write(true).truncate(true).open(path).at(path)?;
                f.write_all(new).and_then(|_| fsutil::sync(&f)).at(path)
            }
            LinkMode::Symlink | LinkMode::Copy => fsutil::write_atomic(path, new),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn link_mode(&self) -> LinkMode {
        self.meta.link_mode.unwrap_or(LinkMode::Copy)
    }

    /// Problems found when opening (unreadable files, wrong pixel hashes).
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Replaces the clock used for IDs and timestamps (tests, imports with a
    /// known time).
    pub fn set_clock(&mut self, clock: impl Fn() -> Timestamp + Send + Sync + 'static) {
        self.clock = Box::new(clock);
    }

    pub fn set_disposal(&mut self, disposal: Disposal) {
        self.disposal = disposal;
    }

    fn now(&self) -> Timestamp {
        (self.clock)()
    }

    pub fn scan(&self) -> Result<Index> {
        let mut ix = scan::scan(&self.root)?;
        ix.transparent = self.pixels.transparent.clone();
        Ok(ix)
    }

    pub fn path(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root.join(rel)
    }

    pub fn image_path(&self, r: &ImageRef) -> PathBuf {
        r.path(&self.root)
    }

    /// The format a store image is in now (links to it carry its extension).
    pub fn format_of(&self, r: &ImageRef) -> StoreFormat {
        StoreFormat::of_path(&self.image_path(r)).unwrap_or_default()
    }

    fn rel(&self, p: &Path) -> PathBuf {
        p.strip_prefix(&self.root).unwrap_or(p).to_path_buf()
    }

    /// IDs of every file in a store.
    pub fn store_ids(&self, store: Store) -> Result<Vec<Id>> {
        Ok(self.store_files(store)?.into_keys().collect())
    }

    /// Every image in a store and the formats it's stored in (two only
    /// after an interrupted conversion).
    fn store_files(&self, store: Store) -> Result<BTreeMap<Id, Vec<StoreFormat>>> {
        let dir = self.root.join(store.dir());
        let mut out: BTreeMap<Id, Vec<StoreFormat>> = BTreeMap::new();
        for e in fs::read_dir(&dir).at(&dir)? {
            let p = e.at(&dir)?.path();
            let Some((format, stem)) = p.file_name().and_then(|n| StoreFormat::of_name(n.to_str()?)) else {
                continue;
            };
            if let Ok(id) = stem.parse() {
                out.entry(id).or_default().push(format);
            }
        }
        Ok(out)
    }

    fn dispose(&self, path: &Path) -> Result<()> {
        match &self.disposal {
            Disposal::SystemTrash => trash::delete(path)
                .map_err(|e| Error::Io { path: path.to_path_buf(), source: std::io::Error::other(e.to_string()) }),
            Disposal::Directory(dir) => {
                fs::create_dir_all(dir).at(dir)?;
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                let to = dir.join(free_name_in(dir, &name)?);
                fs::rename(path, &to).at(path)
            }
        }
    }
}

fn store_kind(store: Store) -> IdKind {
    match store {
        Store::Input => IdKind::InputImage,
        Store::Output => IdKind::OutputImage,
    }
}

// ------------------------------------------------------------------ stores, pixel index

impl Library {
    /// Writes a new store file with an exclusive create, moving on to the next
    /// number if the ID is taken (in either format). `make` builds the bytes
    /// for the given ID and says their format.
    fn write_store(&mut self, store: Store, make: impl Fn(&Id) -> (StoreFormat, Vec<u8>)) -> Result<Id> {
        loop {
            let id = self.ids.allocate(store_kind(store), self.now());
            let r = ImageRef { id: id.clone(), store };
            if StoreFormat::ALL.iter().any(|f| fsutil::exists(&self.root.join(r.rel_path_as(*f)))) {
                continue;
            }
            let (format, bytes) = make(&id);
            let path = self.root.join(r.rel_path_as(format));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let written = f.write_all(&bytes).and_then(|_| fsutil::sync(&f));
                    if let Err(e) = written {
                        let _ = fs::remove_file(&path);
                        return Err(e).at(&path);
                    }
                    return Ok(id);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e).at(&path),
            }
        }
    }

    fn index_of(&mut self, store: Store) -> &mut BTreeMap<String, Id> {
        match store {
            Store::Input => &mut self.pixels.input,
            Store::Output => &mut self.pixels.output,
        }
    }

    /// The store image with these pixels, if any.
    pub fn find_pixels(&self, store: Store, pixel_hash: &str) -> Option<&Id> {
        match store {
            Store::Input => self.pixels.input.get(pixel_hash),
            Store::Output => self.pixels.output.get(pixel_hash),
        }
    }

    /// Loads `cache/pixel-index.rsn` (rebuilding it if missing, unreadable or
    /// of another version), drops entries whose file is gone and indexes files
    /// it lacks, checking their `increart:pixels` against a real hash.
    fn sync_pixel_index(&mut self) -> Result<()> {
        let path = self.root.join(PIXEL_INDEX);
        let loaded = rsnfmt::read::<PixelIndexFile>(&path).ok().filter(|p| p.version == PIXEL_INDEX_VERSION);
        let mut changed = loaded.is_none();
        self.pixels = loaded.unwrap_or_else(|| PixelIndexFile { version: PIXEL_INDEX_VERSION, ..Default::default() });
        for store in [Store::Input, Store::Output] {
            let ids: BTreeSet<Id> = self.store_ids(store)?.into_iter().collect();
            let index = self.index_of(store);
            let before = index.len();
            index.retain(|_, id| ids.contains(id));
            changed |= index.len() != before;
            let before = self.pixels.transparent.len();
            self.pixels.transparent.retain(|r| r.store != store || ids.contains(&r.id));
            changed |= self.pixels.transparent.len() != before;
            let index = self.index_of(store);
            let known: HashSet<Id> = index.values().cloned().collect();
            for id in ids.into_iter().filter(|id| !known.contains(id)) {
                let r = ImageRef { id, store };
                let path = self.image_path(&r);
                let bytes = fs::read(&path).at(&path)?;
                let real = match import::decode_pixels(&bytes) {
                    Ok(px) => {
                        if px.is_transparent() {
                            self.pixels.transparent.insert(r.clone());
                        }
                        px.hash()
                    }
                    Err(e) => {
                        self.problems.push(format!("{}: {e}", path.display()));
                        continue;
                    }
                };
                match container::text(&bytes, png::KEY_PIXELS) {
                    Ok(Some(h)) if h == real => {}
                    Ok(Some(h)) => {
                        self.problems.push(format!("{}: increart:pixels {h} != pixel hash {real}", path.display()))
                    }
                    _ => self.problems.push(format!("{}: no increart:pixels chunk", path.display())),
                }
                self.index_of(store).entry(real).or_insert(r.id);
                changed = true;
            }
        }
        if changed {
            self.save_pixel_index()?;
        }
        Ok(())
    }

    pub fn save_pixel_index(&self) -> Result<()> {
        rsnfmt::write(&self.root.join(PIXEL_INDEX), &self.pixels)
    }

    /// Deletes and rebuilds the pixel index from the store files.
    pub fn rebuild_pixel_index(&mut self) -> Result<()> {
        let _ = fs::remove_file(self.root.join(PIXEL_INDEX));
        self.sync_pixel_index()
    }

    /// A store image as PNG bytes, after checking its pixels against its
    /// `increart:pixels` (refs are verified before they are sent; servers
    /// read PNG, so a WebP one is re-encoded).
    pub fn read_verified(&self, r: &ImageRef) -> Result<Vec<u8>> {
        let path = self.image_path(r);
        let bytes = fs::read(&path).map_err(|_| Error::NotFound(format!("{:?} image {}", r.store, r.id)))?;
        let real = import::decode_pixels(&bytes).map_err(|e| Error::image(&path, e))?.hash();
        match container::text(&bytes, png::KEY_PIXELS).map_err(|e| Error::format(&path, e))? {
            Some(h) if h == real => container::as_png(bytes).map_err(|e| Error::format(&path, e)),
            Some(h) => Err(Error::format(&path, format!("increart:pixels {h} != pixel hash {real}"))),
            None => Err(Error::format(&path, "no increart:pixels chunk")),
        }
    }

    /// Reads a store image's `increart:source` (inputs).
    pub fn source_info(&self, id: &Id) -> Result<Option<SourceInfo>> {
        let path = self.image_path(&ImageRef::input(id.clone()));
        let Some(json) = container::header_text(&path, png::KEY_SOURCE)? else {
            return Ok(None);
        };
        serde_json::from_str(&json).map(Some).map_err(|e| Error::format(&path, e))
    }

    /// Reads an output image's `increart:recipe`.
    pub fn recipe(&self, id: &Id) -> Result<Option<Recipe>> {
        let path = self.image_path(&ImageRef::output(id.clone()));
        let Some(text) = container::header_text(&path, png::KEY_RECIPE)? else {
            return Ok(None);
        };
        rsnfmt::from_str(&text).map(Some).map_err(|e| Error::format(&path, rsnfmt::describe(&text, &e)))
    }

    /// A store image's BLAKE3 pixel hash: its `increart:blake3`, else
    /// computed from its pixels.
    pub fn blake3_of(&self, r: &ImageRef) -> Result<String> {
        let path = self.image_path(r);
        if let Some(h) = container::header_text(&path, png::KEY_BLAKE3)? {
            return Ok(h);
        }
        let bytes = fs::read(&path).at(&path)?;
        Ok(import::decode_pixels(&bytes).map_err(|e| Error::image(&path, e))?.blake3())
    }

    /// A recipe's refs (`<imageN>`, in order) and start image, with their
    /// files and hashes. An image that can't be read is left out.
    pub fn input_images(&self, recipe: &Recipe) -> Vec<InputImage> {
        let start = match &recipe.mode {
            Mode::Edit { init, .. } => init.as_ref(),
            Mode::Generate => None,
        };
        let roles = recipe.refs.iter().enumerate().map(|(i, r)| (InputRole::Ref(i as u32 + 1), r));
        roles
            .chain(start.map(|r| (InputRole::Start, r)))
            .filter_map(|(role, image)| {
                let blake3 = self.blake3_of(image).ok()?;
                let original_name = match image.store {
                    Store::Input => self.source_info(&image.id).ok().flatten().and_then(|s| s.original_name),
                    Store::Output => None,
                };
                let file = fsutil::posix(&self.rel(&self.image_path(image)));
                Some(InputImage { role, image: image.clone(), file, original_name, blake3 })
            })
            .collect()
    }
}

// ------------------------------------------------------------------ import

impl Library {
    /// Imports an image into the input store (re-encoded in the library's
    /// store format, PNG or WebP, with
    /// `increart:id`, `increart:pixels`, `increart:source` and the original
    /// `eXIf`), or returns the existing image with the same pixels.
    pub fn import_bytes(&mut self, bytes: &[u8], original: &Original) -> Result<Imported> {
        let label = original.name.clone().unwrap_or_else(|| "image".into());
        let prepared = import::prepare(bytes, original).map_err(|e| Error::image(&label, e))?;
        if let Some(id) = self.find_pixels(Store::Input, &prepared.pixel_hash) {
            return Ok(Imported { image: id.clone(), reused: true });
        }
        let mut source = prepared.source;
        source.imported = Some(self.now());
        let json = serde_json::to_string_pretty(&source).expect("serializable");
        let hash = prepared.pixel_hash.clone();
        let format = self.meta.store_format;
        let id = self.write_store(Store::Input, |id| {
            let mut chunks = vec![
                Chunk::text(png::KEY_ID, id.as_str()),
                Chunk::text(png::KEY_PIXELS, &hash),
                Chunk::text(png::KEY_BLAKE3, &prepared.blake3),
                UsageCounts::default().chunk(),
                Chunk::itxt(png::KEY_SOURCE, &json),
            ];
            if let Some(exif) = &prepared.exif {
                chunks.push(Chunk::exif(exif.clone()));
            }
            container::encode(format, &prepared.pixels, &chunks)
        })?;
        self.pixels.input.insert(prepared.pixel_hash, id.clone());
        if prepared.pixels.is_transparent() {
            self.pixels.transparent.insert(ImageRef::input(id.clone()));
        }
        self.save_pixel_index()?;
        Ok(Imported { image: id, reused: false })
    }

    pub fn import_file(&mut self, path: &Path) -> Result<Imported> {
        let original = Original::of_file(path, &link::absolute(path)?);
        let bytes = fs::read(path).at(path)?;
        self.import_bytes(&bytes, &original)
    }

    /// Imports every file waiting in `input/inbox_drop/` as a new idea in the
    /// Idea inbox (an image becomes an image-only idea, a `.txt` file a text
    /// idea) and moves the original to `imported/`. Files that are neither
    /// are left in place. Returns the new ideas.
    pub fn import_inbox_drop(&mut self) -> Result<Vec<Id>> {
        let mut out = Vec::new();
        for rel in self.scan()?.inbox_drop {
            let path = self.root.join(&rel);
            let is_text = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("txt"));
            let mut idea = NewIdea { source: IdeaSource::InboxFolder, ..NewIdea::text("", "") };
            if is_text {
                idea.text = fs::read_to_string(&path).at(&path)?.trim().to_string();
            } else {
                let bytes = fs::read(&path).at(&path)?;
                if image::guess_format(&bytes).is_err() {
                    continue;
                }
                match self.import_file(&path) {
                    Ok(imp) => idea.images.push(imp.image),
                    Err(e) => {
                        self.problems.push(e.to_string());
                        continue;
                    }
                }
            }
            out.push(self.new_idea(idea)?);
            let imported = self.root.join(IMPORTED);
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let to = imported.join(free_name_in(&imported, &name)?);
            fs::create_dir_all(&imported).at(&imported)?;
            fs::rename(&path, &to).at(&path)?;
        }
        Ok(out)
    }
}

// ------------------------------------------------------------------ ideas

impl Library {
    pub fn new_idea(&mut self, new: NewIdea) -> Result<Id> {
        for img in &new.images {
            self.require_image(&ImageRef::input(img.clone()))?;
        }
        let base = self.root.join(new.loc.dir());
        if !base.is_dir() {
            return Err(Error::NotFound(format!("folder {}", self.rel(&base).display())));
        }
        let now = self.now();
        let id = self.ids.allocate(IdKind::Idea, now);
        let idea = Idea {
            id: id.clone(),
            title: new.title,
            text: new.text,
            variants: vec![],
            tags: new.tags,
            source: new.source,
            created: now,
            images: new.images,
            projects: vec![],
            starred: new.starred,
        };
        let dir = base.join(id.as_str());
        fs::create_dir(&dir).at(&dir)?;
        self.write_idea_dir(&dir, &idea)?;
        Ok(id)
    }

    fn require_image(&self, r: &ImageRef) -> Result<()> {
        if self.image_path(r).is_file() {
            Ok(())
        } else {
            Err(Error::NotFound(format!("{:?} image {}", r.store, r.id)))
        }
    }

    /// Writes `idea.rsn` and makes the folder's image links match `images`
    /// (`<n>-<original name>.png`, or `.webp` like the store file).
    fn write_idea_dir(&self, dir: &Path, idea: &Idea) -> Result<()> {
        rsnfmt::write(&dir.join(IDEA_FILE), idea)?;
        for e in fs::read_dir(dir).at(dir)? {
            let p = e.at(dir)?.path();
            if StoreFormat::of_path(&p).is_some() {
                fs::remove_file(&p).at(&p)?;
            }
        }
        for (n, img) in idea.images.iter().enumerate() {
            let stem = self
                .source_info(img)
                .ok()
                .flatten()
                .and_then(|s| s.original_name)
                .map(|name| Path::new(&name).file_stem().unwrap_or_default().to_string_lossy().into_owned())
                .map(|s| sanitize(&s))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "image".into());
            let r = ImageRef::input(img.clone());
            let at = dir.join(format!("{}-{stem}.{}", n + 1, self.format_of(&r).ext()));
            link::create(self.link_mode(), &self.image_path(&r), &at)?;
        }
        Ok(())
    }

    /// Where an idea is, and its directory (absolute).
    pub fn find_idea(&self, id: &Id) -> Result<(IdeaLoc, PathBuf)> {
        for loc in [IdeaLoc::Unreviewed, IdeaLoc::Discarded, IdeaLoc::Edited] {
            let dir = self.root.join(loc.dir()).join(id.as_str());
            if dir.join(IDEA_FILE).is_file() {
                return Ok((loc, dir));
            }
        }
        let lib = self.root.join(INPUT_LIBRARY);
        for p in fsutil::walk(&lib)? {
            if p.file_name().is_some_and(|n| n == id.as_str()) && p.join(IDEA_FILE).is_file() {
                let folder = p.parent().unwrap().strip_prefix(&lib).unwrap().to_path_buf();
                return Ok((IdeaLoc::Library(folder), p));
            }
        }
        Err(Error::NotFound(format!("idea {id}")))
    }

    pub fn load_idea(&self, id: &Id) -> Result<Idea> {
        let (_, dir) = self.find_idea(id)?;
        rsnfmt::read(&dir.join(IDEA_FILE))
    }

    /// Saves an edited idea (title, text, variants, tags, images, star) in
    /// place, and refreshes its image links.
    pub fn save_idea(&self, idea: &Idea) -> Result<()> {
        for img in &idea.images {
            self.require_image(&ImageRef::input(img.clone()))?;
        }
        let (_, dir) = self.find_idea(&idea.id)?;
        self.write_idea_dir(&dir, idea)
    }

    pub fn update_idea(&self, id: &Id, f: impl FnOnce(&mut Idea)) -> Result<Idea> {
        let mut idea = self.load_idea(id)?;
        f(&mut idea);
        self.save_idea(&idea)?;
        Ok(idea)
    }

    /// Moves an idea's folder (the folder is the verdict).
    pub fn move_idea(&self, id: &Id, to: &IdeaLoc) -> Result<PathBuf> {
        let (loc, dir) = self.find_idea(id)?;
        let target = self.root.join(to.dir());
        if !target.is_dir() {
            return Err(Error::NotFound(format!("folder {}", to.dir().display())));
        }
        if &loc == to {
            return Ok(self.rel(&dir));
        }
        let dest = target.join(id.as_str());
        link::move_with_links(&self.root, &dir, &dest)?;
        Ok(self.rel(&dest))
    }

    /// Input review: good keeps and stars, neutral keeps, bad discards. Kept
    /// ideas land at the top of the input library.
    pub fn review_idea(&self, id: &Id, verdict: Verdict) -> Result<PathBuf> {
        let to = match verdict {
            Verdict::Bad => IdeaLoc::Discarded,
            Verdict::Good | Verdict::Neutral => {
                self.update_idea(id, |i| i.starred = verdict == Verdict::Good)?;
                IdeaLoc::top_library()
            }
        };
        self.move_idea(id, &to)
    }

    /// Saves an inbox idea to the input library: retitled (when `title` is
    /// given), unstarred, into `folder` (relative to `input/library/`; its
    /// missing folders are made).
    pub fn save_to_library(&self, id: &Id, title: Option<&str>, folder: &Path) -> Result<PathBuf> {
        self.make_library_folder(folder)?;
        let title = title.map(str::trim);
        if let Some(t) = title
            && t.is_empty()
        {
            return Err(Error::InvalidName { name: t.into(), reason: "empty" });
        }
        self.update_idea(id, |i| {
            i.starred = false;
            if let Some(t) = title {
                i.title = t.to_string();
            }
        })?;
        self.move_idea(id, &IdeaLoc::Library(folder.to_path_buf()))
    }

    /// Makes `folder` of the input library (relative to `input/library/`)
    /// and its missing parents, checking each name.
    pub fn make_library_folder(&self, folder: &Path) -> Result<()> {
        let mut dir = self.root.join(INPUT_LIBRARY);
        for c in folder.components() {
            validate_name(&c.as_os_str().to_string_lossy())?;
            dir.push(c);
        }
        if !dir.is_dir() {
            fs::create_dir_all(&dir).at(&dir)?;
        }
        Ok(())
    }

    /// Deletes a discarded idea for good (its folder goes to the trash; the
    /// store images stay until Clean up).
    pub fn delete_discarded_idea(&self, id: &Id) -> Result<()> {
        match self.find_idea(id)? {
            (IdeaLoc::Discarded, dir) => self.dispose(&dir),
            _ => Err(Error::Refused(format!("idea {id} is not discarded"))),
        }
    }

    /// A copy with a new ID: same text, variants, tags and images (new links
    /// to the same store files); `projects` starts empty.
    pub fn copy_idea(&mut self, id: &Id, to: &IdeaLoc) -> Result<Id> {
        let src = self.load_idea(id)?;
        let new = self.new_idea(NewIdea {
            title: src.title.clone(),
            text: src.text.clone(),
            tags: src.tags.clone(),
            source: src.source.clone(),
            images: src.images.clone(),
            starred: src.starred,
            loc: to.clone(),
        })?;
        self.update_idea(&new, |i| i.variants = src.variants)?;
        Ok(new)
    }

    /// An idea made from an output image (`R`: image only, `I`: image plus
    /// its prompt as text). The pixels are imported into the input store.
    pub fn idea_from_output(&mut self, output: &Id, with_prompt: bool, loc: IdeaLoc) -> Result<Id> {
        let r = ImageRef::output(output.clone());
        let path = self.image_path(&r);
        let bytes = fs::read(&path).at(&path)?;
        let name = format!("{output}.{}", self.format_of(&r).ext());
        let original = Original { name: Some(name), path: Some(r.rel_path(&self.root)), ..Default::default() };
        let imported = self.import_bytes(&bytes, &original)?;
        let text = match (with_prompt, self.recipe(output)?) {
            (true, Some(recipe)) => recipe.prompt_original,
            _ => String::new(),
        };
        self.new_idea(NewIdea {
            title: String::new(),
            text,
            tags: vec![],
            source: IdeaSource::FromOutput { image: r },
            images: vec![imported.image],
            starred: false,
            loc,
        })
    }

    /// Imports a tiled image (encoded, e.g. PNG) and makes an idea of it,
    /// remembering the images it was made of.
    /// A cropped and marked copy of an image: the PNG `bytes` as a new idea
    /// in `input/edited/` (made if missing), titled `title`.
    pub fn edited_copy(&mut self, bytes: &[u8], image: ImageRef, edits: ImageEdits, title: &str) -> Result<Imported> {
        let dir = self.root.join(EDITED_IDEAS);
        fs::create_dir_all(&dir).at(&dir)?;
        let original = Original { name: Some("edited.png".into()), ..Default::default() };
        let imported = self.import_bytes(bytes, &original)?;
        self.new_idea(NewIdea {
            title: title.into(),
            text: String::new(),
            tags: vec![],
            source: IdeaSource::Edited { image, edits },
            images: vec![imported.image.clone()],
            starred: false,
            loc: IdeaLoc::Edited,
        })?;
        Ok(imported)
    }

    pub fn idea_from_tiles(&mut self, bytes: &[u8], images: Vec<ImageRef>, title: &str, loc: IdeaLoc) -> Result<Id> {
        let original = Original { name: Some("tiles.png".into()), ..Default::default() };
        let imported = self.import_bytes(bytes, &original)?;
        self.new_idea(NewIdea {
            title: title.into(),
            text: String::new(),
            tags: vec![],
            source: IdeaSource::Tiled { images },
            images: vec![imported.image],
            starred: false,
            loc,
        })
    }
}

// ------------------------------------------------------------------ folder trees

/// The name the Output library's folder view gives its virtual folder of
/// smart folders, at the top of `collections/`.
pub const SMART_FOLDER: &str = "smart";

/// Refuses "smart" (any case) at the top of the collections tree, where it
/// would hide the smart folders.
fn reserved(tree: Tree, parent: &Path, name: &str) -> Result<()> {
    if tree == Tree::Collections && parent.as_os_str().is_empty() && name.eq_ignore_ascii_case(SMART_FOLDER) {
        return Err(Error::InvalidName {
            name: name.into(),
            reason: "reserved for the smart folders at the top of collections",
        });
    }
    Ok(())
}

/// Checks a folder/project name: not empty, no `/` or `\`, not starting with
/// `.` (DESIGN.md, Project folders).
pub fn validate_name(name: &str) -> Result<()> {
    let bad = |reason| Err(Error::InvalidName { name: name.into(), reason });
    if name.trim().is_empty() {
        bad("empty")
    } else if name.contains(['/', '\\']) {
        bad("contains / or \\")
    } else if name.starts_with('.') {
        bad("starts with .")
    } else if name.chars().any(char::is_control) {
        bad("contains control characters")
    } else {
        Ok(())
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c })
        .collect::<String>()
        .trim_start_matches('.')
        .to_string()
}

/// Is `name` taken among the entries of `dir`, case-insensitively (ignoring
/// `except`, for a case-only rename)?
fn name_taken(dir: &Path, name: &str, except: Option<&Path>) -> Result<bool> {
    let Ok(rd) = fs::read_dir(dir) else { return Ok(false) };
    let lower = name.to_lowercase();
    for e in rd {
        let p = e.at(dir)?.path();
        if Some(p.as_path()) != except && p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == lower) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `base`, else `base 2`, `base 3`, … (before the extension, if any).
fn free_name_in(dir: &Path, base: &str) -> Result<String> {
    if !name_taken(dir, base, None)? {
        return Ok(base.into());
    }
    let (stem, ext) = match base.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.contains(' ') => (s, format!(".{e}")),
        _ => (base, String::new()),
    };
    for n in 2.. {
        let name = format!("{stem} {n}{ext}");
        if !name_taken(dir, &name, None)? {
            return Ok(name);
        }
    }
    unreachable!()
}

impl Library {
    fn tree_path(&self, tree: Tree, path: &Path) -> Result<PathBuf> {
        if path.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err(Error::Refused(format!("bad tree path {}", path.display())));
        }
        Ok(self.root.join(tree.base()).join(path))
    }

    fn node(&self, tree: Tree, abs: &Path) -> Result<Node> {
        let meta = fs::symlink_metadata(abs).map_err(|_| Error::NotFound(self.rel(abs).display().to_string()))?;
        Ok(match tree {
            Tree::Projects if abs.join(PROJECT_FILE).is_file() => Node::Leaf,
            Tree::InputLibrary if abs.join(IDEA_FILE).is_file() => Node::Leaf,
            Tree::Collections if !meta.is_dir() => Node::Leaf,
            _ if meta.is_dir() => Node::Folder,
            _ => return Err(Error::Refused(format!("{} is not a folder", self.rel(abs).display()))),
        })
    }

    /// A folder of `tree` (or the top level) that new entries can go into.
    fn folder(&self, tree: Tree, path: &Path) -> Result<PathBuf> {
        let abs = self.tree_path(tree, path)?;
        match self.node(tree, &abs)? {
            Node::Folder => Ok(abs),
            Node::Leaf => Err(Error::Refused(format!("{} is not a folder", path.display()))),
        }
    }

    /// A free name in a folder: `base`, `base 2`, …
    pub fn free_name(&self, tree: Tree, parent: &Path, base: &str) -> Result<String> {
        free_name_in(&self.folder(tree, parent)?, base)
    }

    /// Creates a folder (`None`: a free "new folder" name). Returns its path
    /// in the tree.
    pub fn create_folder(&self, tree: Tree, parent: &Path, name: Option<&str>) -> Result<PathBuf> {
        let dir = self.folder(tree, parent)?;
        let name = match name {
            Some(n) => {
                validate_name(n)?;
                reserved(tree, parent, n)?;
                if name_taken(&dir, n, None)? {
                    return Err(Error::InvalidName { name: n.into(), reason: "already exists here" });
                }
                n.to_string()
            }
            None => free_name_in(&dir, "new folder")?,
        };
        fs::create_dir(dir.join(&name)).at(dir.join(&name))?;
        Ok(parent.join(name))
    }

    /// Renames a folder or project. Ideas are renamed by editing their title.
    pub fn rename_node(&self, tree: Tree, path: &Path, name: &str) -> Result<PathBuf> {
        let abs = self.tree_path(tree, path)?;
        if path.as_os_str().is_empty() {
            return Err(Error::Refused("can't rename the top level".into()));
        }
        if tree == Tree::InputLibrary && self.node(tree, &abs)? == Node::Leaf {
            return Err(Error::Refused("an idea is renamed by editing its title".into()));
        }
        self.node(tree, &abs)?;
        validate_name(name)?;
        reserved(tree, path.parent().unwrap_or(Path::new("")), name)?;
        let parent = abs.parent().unwrap();
        if name_taken(parent, name, Some(&abs))? {
            return Err(Error::InvalidName { name: name.into(), reason: "already exists here" });
        }
        let to = parent.join(name);
        fs::rename(&abs, &to).at(&abs)?;
        Ok(path.with_file_name(name))
    }

    /// Moves a node into another folder (or the top level), like cut + paste.
    /// A clashing name gets a number suffix. Returns the new path.
    pub fn move_node(&self, tree: Tree, path: &Path, new_parent: &Path) -> Result<PathBuf> {
        let abs = self.tree_path(tree, path)?;
        self.node(tree, &abs)?;
        if path.as_os_str().is_empty() {
            return Err(Error::Refused("can't move the top level".into()));
        }
        if new_parent.starts_with(path) {
            return Err(Error::Refused("can't move a folder into itself".into()));
        }
        let dest_dir = self.folder(tree, new_parent)?;
        if path.parent() == Some(new_parent) {
            return Ok(path.to_path_buf());
        }
        reserved(tree, new_parent, &path.file_name().unwrap().to_string_lossy())?;
        let name = free_name_in(&dest_dir, &path.file_name().unwrap().to_string_lossy())?;
        link::move_with_links(&self.root, &abs, &dest_dir.join(&name))?;
        Ok(new_parent.join(name))
    }

    /// Copies a node into a folder (copy + paste). Projects copy their setup
    /// (new project and run IDs, runs as unqueued templates) but no images;
    /// ideas get a new ID and new links; collection links are re-linked.
    /// Folders copy their subtree the same way.
    pub fn copy_node(&mut self, tree: Tree, path: &Path, new_parent: &Path) -> Result<PathBuf> {
        let abs = self.tree_path(tree, path)?;
        let kind = self.node(tree, &abs)?;
        if path.as_os_str().is_empty() {
            return Err(Error::Refused("can't copy the top level".into()));
        }
        if new_parent.starts_with(path) {
            return Err(Error::Refused("can't copy a folder into itself".into()));
        }
        reserved(tree, new_parent, &path.file_name().unwrap().to_string_lossy())?;
        let dest_dir = self.folder(tree, new_parent)?;
        match (tree, kind) {
            (Tree::InputLibrary, Node::Leaf) => {
                let id: Id = path.file_name().unwrap().to_string_lossy().parse()?;
                let new = self.copy_idea(&id, &IdeaLoc::Library(new_parent.to_path_buf()))?;
                Ok(new_parent.join(new.as_str()))
            }
            (Tree::Projects, Node::Leaf) => {
                let name = free_name_in(&dest_dir, &path.file_name().unwrap().to_string_lossy())?;
                self.copy_project_setup(&abs, &dest_dir.join(&name))?;
                Ok(new_parent.join(name))
            }
            (Tree::Collections, Node::Leaf) => {
                let name = free_name_in(&dest_dir, &path.file_name().unwrap().to_string_lossy())?;
                let r = link::read_ref(&abs, Store::Output)?
                    .ok_or_else(|| Error::Refused(format!("{} is not a link", path.display())))?;
                link::create(self.link_mode(), &self.image_path(&r), &dest_dir.join(&name))?;
                Ok(new_parent.join(name))
            }
            (_, Node::Folder) => {
                let name = free_name_in(&dest_dir, &path.file_name().unwrap().to_string_lossy())?;
                let new = new_parent.join(&name);
                fs::create_dir(dest_dir.join(&name)).at(dest_dir.join(&name))?;
                for child in fs::read_dir(&abs).at(&abs)? {
                    let child = child.at(&abs)?.path();
                    if fsutil::hidden(&child) || (tree != Tree::Collections && !child.is_dir()) {
                        continue;
                    }
                    let child_rel = path.join(child.file_name().unwrap());
                    self.copy_node(tree, &child_rel, &new)?;
                }
                Ok(new)
            }
        }
    }

    /// Copies a project's setup and draft (no images) next to it, as `name`.
    pub fn duplicate_project(&mut self, path: &Path, name: &str) -> Result<PathBuf> {
        let abs = self.tree_path(Tree::Projects, path)?;
        if self.node(Tree::Projects, &abs)? != Node::Leaf {
            return Err(Error::Refused(format!("{} is not a project", path.display())));
        }
        validate_name(name)?;
        let parent = abs.parent().unwrap();
        if name_taken(parent, name, None)? {
            return Err(Error::InvalidName { name: name.into(), reason: "already exists here" });
        }
        self.copy_project_setup(&abs, &parent.join(name))?;
        Ok(path.with_file_name(name))
    }

    fn copy_project_setup(&mut self, from: &Path, to: &Path) -> Result<()> {
        let src: Project = rsnfmt::read(&from.join(PROJECT_FILE))?;
        let now = self.now();
        let project = Project {
            id: self.ids.allocate(IdKind::Project, now),
            created: now,
            sources: src.sources,
            cover: None,
            edit_with: src.edit_with,
            titles: Default::default(),
        };
        self.write_project_dir(to, &project)?;
        for run_dir in fsutil::subdirs(&from.join("runs"))? {
            let mut run: Run = rsnfmt::read(&run_dir.join(RUN_FILE))?;
            run.id = self.ids.allocate(IdKind::Run, now);
            run.created = now;
            run.revisions.clear();
            run.cancelled.clear();
            run.failed.clear();
            run.timings.clear();
            run.deleted.clear();
            let dir = to.join("runs").join(run.id.as_str());
            fs::create_dir_all(&dir).at(&dir)?;
            rsnfmt::write(&dir.join(RUN_FILE), &run)?;
        }
        if from.join(DRAFT_FILE).is_file() {
            fs::copy(from.join(DRAFT_FILE), to.join(DRAFT_FILE)).at(to.join(DRAFT_FILE))?;
        }
        Ok(())
    }

    /// Deletes a node. Projects and collection folders go to the trash (only
    /// links: store images stay until Clean up); a collection link is
    /// removed. In the input library an idea is discarded, and a folder is
    /// removed after discarding every idea inside. Returns the discarded
    /// ideas (for undo).
    pub fn delete_node(&self, tree: Tree, path: &Path) -> Result<Vec<Id>> {
        let abs = self.tree_path(tree, path)?;
        let kind = self.node(tree, &abs)?;
        if path.as_os_str().is_empty() {
            return Err(Error::Refused("can't delete the top level".into()));
        }
        match (tree, kind) {
            (Tree::InputLibrary, _) => {
                let mut ideas = Vec::new();
                let mut dirs = vec![abs.clone()];
                dirs.extend(fsutil::walk(&abs)?);
                for d in dirs {
                    if d.join(IDEA_FILE).is_file()
                        && let Ok(id) = d.file_name().unwrap().to_string_lossy().parse::<Id>()
                    {
                        ideas.push(id);
                    }
                }
                for id in &ideas {
                    self.move_idea(id, &IdeaLoc::Discarded)?;
                }
                if kind == Node::Folder {
                    self.dispose(&abs)?;
                }
                Ok(ideas)
            }
            (Tree::Collections, Node::Leaf) => {
                fs::remove_file(&abs).at(&abs)?;
                Ok(vec![])
            }
            // projects go to Discarded (restorable), then their folders to
            // the trash
            (Tree::Projects, _) => {
                let mut projects = vec![abs.clone()];
                if kind == Node::Folder {
                    projects = fsutil::walk(&abs)?.into_iter().filter(|d| d.join(PROJECT_FILE).is_file()).collect();
                }
                for p in &projects {
                    self.discard_project(p)?;
                }
                if kind == Node::Folder {
                    self.dispose(&abs)?;
                }
                Ok(vec![])
            }
            _ => {
                self.dispose(&abs)?;
                Ok(vec![])
            }
        }
    }
}

// ------------------------------------------------------------------ projects, runs

impl Library {
    /// Creates a project in a folder (`None`: a free "new project" name) with
    /// the given source ideas, and records it in those ideas' `projects`.
    pub fn create_project(&mut self, parent: &Path, name: Option<&str>, sources: &[Id]) -> Result<(Id, PathBuf)> {
        let dir = self.folder(Tree::Projects, parent)?;
        let name = match name {
            Some(n) => {
                validate_name(n)?;
                if name_taken(&dir, n, None)? {
                    return Err(Error::InvalidName { name: n.into(), reason: "already exists here" });
                }
                n.to_string()
            }
            None => free_name_in(&dir, "new project")?,
        };
        let now = self.now();
        let project = Project {
            id: self.ids.allocate(IdKind::Project, now),
            created: now,
            sources: sources.to_vec(),
            cover: None,
            edit_with: EditWith::default(),
            titles: Default::default(),
        };
        for s in sources {
            self.load_idea(s)?;
        }
        self.write_project_dir(&dir.join(&name), &project)?;
        for s in sources {
            self.update_idea(s, |i| {
                if !i.projects.contains(&project.id) {
                    i.projects.push(project.id.clone());
                }
            })?;
        }
        Ok((project.id, parent.join(name)))
    }

    fn write_project_dir(&self, dir: &Path, project: &Project) -> Result<()> {
        fs::create_dir(dir).at(dir)?;
        for sub in ["runs", "unreviewed", "reviewed/good", "reviewed/neutral", "reviewed/bad"] {
            fs::create_dir_all(dir.join(sub)).at(dir.join(sub))?;
        }
        rsnfmt::write(&dir.join(PROJECT_FILE), project)
    }

    /// Moves a project (its directory, absolute) to `discarded_projects/`,
    /// noting where it was.
    fn discard_project(&self, dir: &Path) -> Result<PathBuf> {
        let projects = self.root.join(PROJECTS);
        let rel =
            dir.strip_prefix(&projects).map_err(|_| Error::Refused(format!("{} is not a project", dir.display())))?;
        let name = rel.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let from = rel.parent().unwrap_or(Path::new("")).to_path_buf();
        let base = self.root.join(DISCARDED_PROJECTS);
        fs::create_dir_all(&base).at(&base)?;
        let to = base.join(free_name_in(&base, &name)?);
        link::move_with_links(&self.root, dir, &to)?;
        rsnfmt::write(&to.join(DISCARDED_FILE), &DiscardedProject { name, from, discarded: self.now() })?;
        Ok(self.rel(&to))
    }

    /// A discarded project's directory (absolute).
    fn find_discarded_project(&self, id: &Id) -> Result<PathBuf> {
        for p in fsutil::subdirs(&self.root.join(DISCARDED_PROJECTS))? {
            let file = p.join(PROJECT_FILE);
            if file.is_file() && rsnfmt::read::<Project>(&file).is_ok_and(|pr| &pr.id == id) {
                return Ok(p);
            }
        }
        Err(Error::NotFound(format!("discarded project {id}")))
    }

    /// Puts a discarded project back where it was (the top level if that
    /// folder is gone), under its name (numbered if taken). Returns its path
    /// relative to `projects/`.
    pub fn restore_project(&self, id: &Id) -> Result<PathBuf> {
        let dir = self.find_discarded_project(id)?;
        let note = rsnfmt::read::<DiscardedProject>(&dir.join(DISCARDED_FILE)).ok();
        let name = note.as_ref().map(|n| n.name.clone());
        let name = name.unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into_owned());
        let from = note.map(|n| n.from).unwrap_or_default();
        let projects = self.root.join(PROJECTS);
        let parent = if projects.join(&from).is_dir() { from } else { PathBuf::new() };
        let name = free_name_in(&projects.join(&parent), &name)?;
        let to = projects.join(&parent).join(&name);
        link::move_with_links(&self.root, &dir, &to)?;
        let note = to.join(DISCARDED_FILE);
        if note.exists() {
            fs::remove_file(&note).at(&note)?;
        }
        Ok(parent.join(name))
    }

    /// Deletes a discarded project for good (its folder goes to the trash;
    /// the store images stay until Clean up).
    pub fn delete_discarded_project(&self, id: &Id) -> Result<()> {
        let dir = self.find_discarded_project(id)?;
        self.dispose(&dir)
    }

    /// A project's directory (absolute), wherever it has been moved.
    pub fn find_project(&self, id: &Id) -> Result<PathBuf> {
        for p in fsutil::walk(&self.root.join(PROJECTS))? {
            let file = p.join(PROJECT_FILE);
            if file.is_file() && rsnfmt::read::<Project>(&file).is_ok_and(|pr| &pr.id == id) {
                return Ok(p);
            }
        }
        Err(Error::NotFound(format!("project {id}")))
    }

    pub fn load_project(&self, id: &Id) -> Result<Project> {
        rsnfmt::read(&self.find_project(id)?.join(PROJECT_FILE))
    }

    pub fn set_edit_with(&self, project: &Id, edit_with: EditWith) -> Result<()> {
        let dir = self.find_project(project)?;
        let mut p: Project = rsnfmt::read(&dir.join(PROJECT_FILE))?;
        p.edit_with = edit_with;
        rsnfmt::write(&dir.join(PROJECT_FILE), &p)
    }

    pub fn set_cover(&self, project: &Id, cover: Option<ImageRef>) -> Result<()> {
        if let Some(c) = &cover {
            self.require_image(c)?;
        }
        let dir = self.find_project(project)?;
        let mut p: Project = rsnfmt::read(&dir.join(PROJECT_FILE))?;
        p.cover = cover;
        rsnfmt::write(&dir.join(PROJECT_FILE), &p)
    }

    /// Gives one of a project's output images a title; an empty one
    /// removes it.
    pub fn set_title(&self, project: &Id, image: &Id, title: &str) -> Result<()> {
        self.require_image(&ImageRef::output(image.clone()))?;
        let dir = self.find_project(project)?;
        let mut p: Project = rsnfmt::read(&dir.join(PROJECT_FILE))?;
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() {
            p.titles.remove(image);
        } else {
            p.titles.insert(image.clone(), title);
        }
        rsnfmt::write(&dir.join(PROJECT_FILE), &p)
    }

    pub fn create_run(&mut self, project: &Id, new: NewRun) -> Result<Id> {
        let dir = self.find_project(project)?;
        for r in &new.refs {
            self.require_image(r)?;
        }
        if let Mode::Edit { init: Some(init), .. } = &new.mode {
            self.require_image(init)?;
        }
        let now = self.now();
        let id = self.ids.allocate(IdKind::Run, now);
        let revisions = if new.seeds > 0 {
            vec![Revision { rev: 0, created: now, seeds: Seeds { first: new.first_seed, count: new.seeds } }]
        } else {
            vec![]
        };
        let run = Run {
            id: id.clone(),
            label: new.label,
            created: now,
            prompt: new.prompt,
            negative_prompt: new.negative_prompt,
            alpha: new.alpha,
            width: new.width,
            height: new.height,
            sample: new.sample,
            mode: new.mode,
            refs: new.refs,
            model: new.model,
            source_idea: new.source_idea,
            revisions,
            cancelled: vec![],
            failed: vec![],
            sweep: new.sweep,
            timings: vec![],
            deleted: vec![],
        };
        let run_dir = dir.join("runs").join(id.as_str());
        fs::create_dir_all(&run_dir).at(&run_dir)?;
        rsnfmt::write(&run_dir.join(RUN_FILE), &run)?;
        if !run.revisions.is_empty() {
            self.bump_usage(&run);
        }
        Ok(id)
    }

    /// The project directory (absolute) holding a run, and the run.
    pub fn find_run(&self, id: &Id) -> Result<(PathBuf, Run)> {
        for p in fsutil::walk(&self.root.join(PROJECTS))? {
            let file = p.join("runs").join(id.as_str()).join(RUN_FILE);
            if p.join(PROJECT_FILE).is_file() && file.is_file() {
                return Ok((p, rsnfmt::read(&file)?));
            }
        }
        Err(Error::NotFound(format!("run {id}")))
    }

    fn save_run(&self, project_dir: &Path, run: &Run) -> Result<()> {
        rsnfmt::write(&project_dir.join("runs").join(run.id.as_str()).join(RUN_FILE), run)
    }

    /// Seeds of a run still to generate: live seeds with no output link in
    /// the project yet, and no timing (one generated already). In
    /// `(rev, seed)` order.
    pub fn pending_seeds(&self, run: &Id) -> Result<Vec<(u32, u64)>> {
        let (dir, r) = self.find_run(run)?;
        let mut done: HashSet<u64> = r.timings.iter().map(|t| t.seed).collect();
        for verdict in std::iter::once(None).chain(Verdict::ALL.map(Some)) {
            let folder = dir.join(Verdict::output_dir(verdict));
            let Ok(rd) = fs::read_dir(&folder) else { continue };
            for e in rd.flatten() {
                if let Some(n) = OutputName::parse(&e.file_name().to_string_lossy())
                    && &n.run == run
                {
                    done.insert(n.seed);
                }
            }
        }
        Ok(r.live_seeds().filter(|(_, s)| !done.contains(s)).collect())
    }

    /// Records seeds as cancelled (Cancel: they are never generated).
    pub fn cancel_seeds(&self, run: &Id, seeds: &[u64]) -> Result<()> {
        if seeds.is_empty() {
            return Ok(());
        }
        let (dir, mut r) = self.find_run(run)?;
        let mut all: Vec<u64> = r.cancelled.iter().flat_map(|c| c.first..c.first + c.count).collect();
        all.extend(seeds);
        r.cancelled = seed_ranges(all);
        self.save_run(&dir, &r)
    }

    /// Records a seed that failed for good.
    pub fn record_failure(&self, run: &Id, rev: u32, seed: u64, error: &str) -> Result<()> {
        let (dir, mut r) = self.find_run(run)?;
        r.failed.retain(|f| f.seed != seed);
        r.failed.push(SeedFailure { rev, seed, error: error.into() });
        self.save_run(&dir, &r)
    }

    /// Adds `count` seeds continuing after the run's highest seed, to the last
    /// revision or (if `new_revision`, or the run has none) a new one.
    /// Returns the revision and its new seeds.
    pub fn add_seeds(&mut self, run: &Id, count: u64, new_revision: bool) -> Result<(u32, Seeds)> {
        let (dir, mut r) = self.find_run(run)?;
        let first_use = r.revisions.is_empty();
        let first = r.next_seed();
        let seeds = Seeds { first, count };
        let rev = match r.revisions.last_mut() {
            Some(last) if !new_revision && last.seeds.first + last.seeds.count == first => {
                last.seeds.count += count;
                last.rev
            }
            last => {
                let rev = last.map_or(0, |l| l.rev + 1);
                r.revisions.push(Revision { rev, created: self.now(), seeds });
                rev
            }
        };
        self.save_run(&dir, &r)?;
        if first_use {
            self.bump_usage(&r);
        }
        Ok((rev, seeds))
    }
}

// ------------------------------------------------------------------ outputs, review, collections

impl Library {
    /// Stores a generated PNG: adds `increart:id`, `increart:pixels` and
    /// `increart:recipe` (other chunks such as sd-server's `parameters` are
    /// kept), writes `output_images/<id>.png` (without re-encoding) or, in a
    /// WebP library, `.webp`, and links it into the run's project as
    /// `unreviewed/<run>_r<rev>_s<seed>.png` (`.webp`).
    /// Returns the image ID and the link (relative to the root).
    pub fn write_output(
        &mut self,
        run: &Id,
        rev: u32,
        seed: u64,
        png_bytes: &[u8],
        recipe: &Recipe,
    ) -> Result<(Id, PathBuf)> {
        let (project_dir, mut run_file) = self.find_run(run)?;
        let label = format!("output of run {run}");
        let pixels = import::decode_pixels(png_bytes).map_err(|e| Error::image(&label, e))?;
        let hash = pixels.hash();
        let blake3 = pixels.blake3();
        let mut recipe = recipe.clone();
        if recipe.inputs.is_empty() {
            recipe.inputs = self.input_images(&recipe);
        }
        let recipe_text = rsnfmt::to_string(&recipe);
        // Fail on a malformed file before taking an ID.
        let server_meta = container::metadata(png_bytes).map_err(|e| Error::format(&label, e))?;
        let format = self.meta.store_format;
        let id = self.write_store(Store::Output, |id| {
            let chunks = [
                Chunk::text(png::KEY_ID, id.as_str()),
                Chunk::text(png::KEY_PIXELS, &hash),
                Chunk::text(png::KEY_BLAKE3, &blake3),
                UsageCounts::default().chunk(),
                Chunk::itxt(png::KEY_RECIPE, &recipe_text),
            ];
            if format == StoreFormat::Png && container::format_of(png_bytes) == Some(StoreFormat::Png) {
                // the server's PNG as it is, chunks added
                return (StoreFormat::Png, png::set_chunks(png_bytes, &chunks).expect("checked above"));
            }
            // its chunks (`parameters`) and ours on the same pixels
            let keys: Vec<_> = chunks.iter().filter_map(|c| c.as_text().map(|(k, _)| k)).collect();
            let mut all: Vec<Chunk> =
                server_meta.iter().filter(|c| !c.as_text().is_some_and(|(k, _)| keys.contains(&k))).cloned().collect();
            all.extend(chunks);
            container::encode(format, &pixels, &all)
        })?;
        self.pixels.output.entry(hash).or_insert_with(|| id.clone());
        if pixels.is_transparent() {
            self.pixels.transparent.insert(ImageRef::output(id.clone()));
        }
        self.save_pixel_index()?;
        let name = OutputName { run: run.clone(), rev, seed };
        let r = ImageRef::output(id.clone());
        let at = project_dir.join(Verdict::output_dir(None)).join(name.file_name(self.format_of(&r)));
        link::create(self.link_mode(), &self.image_path(&r), &at)?;
        // its timing, for the run's statistics (none when it wasn't timed)
        if recipe.elapsed_s > 0.0 {
            let r = &mut run_file;
            r.timings.retain(|t| t.seed != seed);
            r.timings.push(SeedTiming {
                rev,
                seed,
                device: recipe.device.clone(),
                done: self.now(),
                elapsed_s: recipe.elapsed_s,
                queued_s: recipe.queued_s,
                step_s: recipe.step_s,
            });
            self.save_run(&project_dir, &run_file)?;
        }
        Ok((id, self.rel(&at)))
    }

    /// Moves an output link to `unreviewed/` (`None`) or `reviewed/<verdict>/`
    /// of its project. `link` is relative to the root. Returns the new path.
    pub fn review_output(&self, link_path: &Path, verdict: Option<Verdict>) -> Result<PathBuf> {
        let abs = self.root.join(link_path);
        if !fsutil::exists(&abs) {
            return Err(Error::NotFound(link_path.display().to_string()));
        }
        let project = abs
            .ancestors()
            .skip(1)
            .take(3)
            .find(|d| d.join(PROJECT_FILE).is_file())
            .ok_or_else(|| Error::Refused(format!("{} is not an output link", link_path.display())))?
            .to_path_buf();
        let to = project.join(Verdict::output_dir(verdict)).join(abs.file_name().unwrap());
        if to == abs {
            return Ok(link_path.to_path_buf());
        }
        link::move_with_links(&self.root, &abs, &to)?;
        Ok(self.rel(&to))
    }

    /// Saves a project's Generate draft (`projects/<project>/draft.rsn`).
    pub fn save_generate_draft(&self, project: &Id, draft: &GenerateDraft) -> Result<()> {
        rsnfmt::write(&self.find_project(project)?.join(DRAFT_FILE), draft)
    }

    pub fn clear_generate_draft(&self, project: &Id) -> Result<()> {
        let path = self.find_project(project)?.join(DRAFT_FILE);
        match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).at(&path),
            _ => Ok(()),
        }
    }

    /// Saves the New idea draft (`input/draft.rsn`).
    pub fn save_idea_draft(&self, draft: &IdeaDraftFile) -> Result<()> {
        rsnfmt::write(&self.root.join(IDEA_DRAFT), draft)
    }

    pub fn clear_idea_draft(&self) -> Result<()> {
        let path = self.root.join(IDEA_DRAFT);
        match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).at(&path),
            _ => Ok(()),
        }
    }

    /// Where a text history lives (see [`crate::history`]).
    pub fn history_path(&self, key: &HistoryKey) -> Result<PathBuf> {
        Ok(match key {
            HistoryKey::Project(id) => self.find_project(id)?.join("prompt-history.jsonl"),
            HistoryKey::NewIdea => self.root.join("input/new-idea-history.jsonl"),
            HistoryKey::Idea(id) => self.find_idea(id)?.1.join("text-history.jsonl"),
        })
    }

    /// Deletes an output link (e.g. a rejected image): it goes to the trash;
    /// the image stays in the store until Clean up. Its run records the
    /// seed as deleted (not to be generated again, and not missing).
    pub fn delete_output_link(&self, link_path: &Path) -> Result<()> {
        let abs = self.root.join(link_path);
        let in_project = abs.ancestors().skip(1).take(3).any(|d| d.join(PROJECT_FILE).is_file());
        if !in_project || !fsutil::exists(&abs) {
            return Err(Error::Refused(format!("{} is not an output link", link_path.display())));
        }
        let name = abs.file_name().and_then(|n| OutputName::parse(&n.to_string_lossy()));
        if let Some(n) = name
            && let Ok((dir, mut run)) = self.find_run(&n.run)
        {
            let mut all: Vec<u64> = run.deleted.iter().flat_map(|d| d.first..d.first + d.count).collect();
            all.push(n.seed);
            run.deleted = seed_ranges(all);
            self.save_run(&dir, &run)?;
        }
        self.dispose(&abs)
    }

    /// Records as deleted every generated seed whose output link is gone
    /// (metadata 2: links deleted before deletions were recorded).
    fn record_deleted_outputs(&self, ix: &Index) -> Result<()> {
        for p in &ix.projects {
            for run in &p.runs {
                let missing: Vec<u64> = p.missing_outputs(run).map(|(_, s)| s).collect();
                if missing.is_empty() {
                    continue;
                }
                let (dir, mut r) = self.find_run(&run.id)?;
                r.deleted =
                    seed_ranges(r.deleted.iter().flat_map(|d| d.first..d.first + d.count).chain(missing).collect());
                self.save_run(&dir, &r)?;
            }
        }
        Ok(())
    }

    /// Links an output image into a collection folder as `<image-id>.png`
    /// (`.webp` like the store file; unchanged if it is already there).
    /// Returns the link path.
    pub fn add_to_collection(&self, folder: &Path, image: &Id) -> Result<PathBuf> {
        let r = ImageRef::output(image.clone());
        self.require_image(&r)?;
        let dir = self.folder(Tree::Collections, folder)?;
        let at = |f: StoreFormat| dir.join(format!("{image}.{}", f.ext()));
        if let Some(there) = StoreFormat::ALL.into_iter().map(at).find(|p| fsutil::exists(p)) {
            return Ok(self.rel(&there));
        }
        let at = at(self.format_of(&r));
        link::create(self.link_mode(), &self.image_path(&r), &at)?;
        Ok(self.rel(&at))
    }
}

// ------------------------------------------------------------------ store format

/// How many store images are in each format, and their bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreCounts {
    pub png: (usize, u64),
    pub webp: (usize, u64),
}

/// Converting the stores to one format (DESIGN.md, Image stores), a few
/// images at a time so the library stays usable meanwhile:
/// [`Library::conversion`] lists the images, [`Conversion::encode`]
/// re-encodes one without the library (the slow part), and
/// [`Library::install`] puts a batch in place and moves their links.
#[derive(Clone, Debug)]
pub struct Conversion {
    pub root: PathBuf,
    pub to: StoreFormat,
    pub images: Vec<ImageRef>,
}

/// An image re-encoded by [`Conversion::encode`], ready to install.
#[derive(Clone, Debug)]
pub struct Converted {
    image: ImageRef,
    from: StoreFormat,
    to: StoreFormat,
    bytes: Vec<u8>,
}

impl Conversion {
    fn from(&self) -> StoreFormat {
        match self.to {
            StoreFormat::Png => StoreFormat::Webp,
            StoreFormat::Webp => StoreFormat::Png,
        }
    }

    /// Re-encodes one image after checking its pixels against its
    /// `increart:pixels`; the result is checked by decoding it again.
    /// `None` when there's nothing to do: gone meanwhile, or too large for
    /// WebP (it stays PNG).
    pub fn encode(&self, r: &ImageRef) -> Result<Option<Converted>> {
        let from = self.from();
        let path = self.root.join(r.rel_path_as(from));
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).at(&path),
        };
        let (format, out, hash) = container::transcode(&bytes, self.to).map_err(|e| Error::format(&path, e))?;
        match container::text(&bytes, png::KEY_PIXELS).map_err(|e| Error::format(&path, e))? {
            Some(h) if h == hash => {}
            Some(h) => return Err(Error::format(&path, format!("increart:pixels {h} != pixel hash {hash}"))),
            None => return Err(Error::format(&path, "no increart:pixels chunk")),
        }
        Ok((format == self.to).then(|| Converted { image: r.clone(), from, to: format, bytes: out }))
    }
}

impl Library {
    pub fn store_format(&self) -> StoreFormat {
        self.meta.store_format
    }

    /// Sets the format new store images are written in. Existing images
    /// stay as they are until [`conversion`](Self::conversion).
    pub fn set_store_format(&mut self, format: StoreFormat) -> Result<()> {
        self.meta.store_format = format;
        self.save_meta()
    }

    /// Writes `library.rsn`, its format number saying whether the stores may
    /// hold WebP (older versions refuse such a library rather than miss its
    /// images).
    fn save_meta(&mut self) -> Result<()> {
        let webp = self.meta.store_format == StoreFormat::Webp || self.store_counts()?.webp.0 > 0;
        self.meta.format = if webp { LIBRARY_FORMAT_WEBP } else { LIBRARY_FORMAT };
        rsnfmt::write(&self.root.join(LIBRARY_FILE), &self.meta)
    }

    pub fn store_counts(&self) -> Result<StoreCounts> {
        let mut c = StoreCounts::default();
        for store in [Store::Input, Store::Output] {
            let dir = self.root.join(store.dir());
            for e in fs::read_dir(&dir).at(&dir)? {
                let e = e.at(&dir)?;
                let len = e.metadata().map(|m| m.len()).unwrap_or(0);
                match StoreFormat::of_path(&e.path()) {
                    Some(StoreFormat::Png) => (c.png.0 += 1, c.png.1 += len),
                    Some(StoreFormat::Webp) => (c.webp.0 += 1, c.webp.1 += len),
                    None => continue,
                };
            }
        }
        Ok(c)
    }

    /// The store images not yet in `to` (or in both formats, after an
    /// interrupted conversion).
    pub fn conversion(&self, to: StoreFormat) -> Result<Conversion> {
        let mut images = Vec::new();
        for store in [Store::Input, Store::Output] {
            for (id, formats) in self.store_files(store)? {
                if formats.iter().any(|f| *f != to) {
                    images.push(ImageRef { id, store });
                }
            }
        }
        Ok(Conversion { root: self.root.clone(), to, images })
    }

    /// Puts re-encoded images in place: each one's metadata is taken again
    /// from its current file (usage counts may have changed since it was
    /// encoded), the new file is written with the old one's modification
    /// time, every link to it is remade with the new extension, then the
    /// old file goes. Returns the images that failed, with why.
    pub fn install(&mut self, batch: Vec<Converted>) -> Result<Vec<(ImageRef, String)>> {
        let links = self.links_by_image()?;
        let mut failed = Vec::new();
        for c in batch {
            let links = links.get(&c.image).map(Vec::as_slice).unwrap_or_default();
            if let Err(e) = self.install_one(&c, links) {
                failed.push((c.image, e.to_string()));
            }
        }
        self.save_meta()?;
        Ok(failed)
    }

    fn install_one(&self, c: &Converted, links: &[PathBuf]) -> Result<()> {
        let old = self.root.join(c.image.rel_path_as(c.from));
        let Ok(now) = fs::read(&old) else {
            return Ok(()); // gone meanwhile
        };
        let meta = container::metadata(&now).map_err(|e| Error::format(&old, e))?;
        let bytes = container::set_chunks(&c.bytes, &meta).map_err(|e| Error::format(&old, e))?;
        let new = self.root.join(c.image.rel_path_as(c.to));
        fsutil::write_atomic(&new, &bytes)?;
        if let Ok(t) = fs::metadata(&old).and_then(|m| m.modified()) {
            let _ = File::options().write(true).open(&new).and_then(|f| f.set_modified(t));
        }
        for rel in links {
            let at = self.root.join(rel);
            if StoreFormat::of_path(&at) == Some(c.to) {
                continue; // made by an interrupted conversion
            }
            let renamed = at.with_extension(c.to.ext());
            if fsutil::exists(&renamed) {
                fs::remove_file(&renamed).at(&renamed)?;
            }
            link::create(self.link_mode(), &new, &renamed)?;
            fs::remove_file(&at).at(&at)?;
        }
        fs::remove_file(&old).at(&old)
    }

    /// Every link into the stores (review folders, discarded projects' too,
    /// collections, idea folders), by the image it refers to.
    fn links_by_image(&self) -> Result<HashMap<ImageRef, Vec<PathBuf>>> {
        let ix = scan::scan(&self.root)?;
        let mut out: HashMap<ImageRef, Vec<PathBuf>> = HashMap::new();
        let discarded = ix.discarded_projects.iter().map(|d| &d.entry);
        for o in ix.projects.iter().chain(discarded).flat_map(|p| &p.outputs) {
            out.entry(o.image.clone()).or_default().push(o.path.clone());
        }
        for c in &ix.collection_links {
            out.entry(c.image.clone()).or_default().push(c.path.clone());
        }
        for e in &ix.ideas {
            let dir = self.root.join(&e.dir);
            for f in fs::read_dir(&dir).at(&dir)? {
                let p = f.at(&dir)?.path();
                if StoreFormat::of_path(&p).is_some()
                    && let Some(r) = link::read_ref(&p, Store::Input)?
                {
                    out.entry(r).or_default().push(self.rel(&p));
                }
            }
        }
        Ok(out)
    }
}

// ------------------------------------------------------------------ clean up

impl Library {
    /// Store images nothing refers to: not linked from any folder, not in an
    /// `idea.rsn`, a `run.rsn` or a project cover, not an idea's `FromOutput`
    /// source, and not a ref / edit init / parent in the recipe of an output
    /// that is itself kept.
    pub fn unreferenced(&self) -> Result<Vec<ImageRef>> {
        let ix = self.scan()?;
        let mut marked: HashSet<ImageRef> = HashSet::new();
        let mut todo: Vec<ImageRef> = Vec::new();
        let dirs = [
            ("input", Store::Input),
            (PROJECTS, Store::Output),
            (DISCARDED_PROJECTS, Store::Output),
            (COLLECTIONS, Store::Output),
        ];
        for (dir, store) in dirs {
            for p in fsutil::walk(&self.root.join(dir))? {
                if p.starts_with(self.root.join(INBOX_DROP)) || p.is_dir() && !fsutil::is_symlink(&p) {
                    continue;
                }
                if let Ok(Some(r)) = link::read_ref(&p, store) {
                    todo.push(r);
                }
            }
        }
        for e in &ix.ideas {
            todo.extend(e.idea.images.iter().cloned().map(ImageRef::input));
            match &e.idea.source {
                // an edited copy keeps its original, to be edited again
                IdeaSource::FromOutput { image } | IdeaSource::Edited { image, .. } => todo.push(image.clone()),
                IdeaSource::Tiled { images } => todo.extend(images.iter().cloned()),
                _ => {}
            }
        }
        if let Some(d) = &ix.idea_draft {
            todo.extend(d.images.iter().cloned().map(ImageRef::input));
        }
        // discarded projects keep theirs, to be restored
        for p in ix.projects.iter().chain(ix.discarded_projects.iter().map(|d| &d.entry)) {
            todo.extend(p.project.cover.iter().cloned());
            todo.extend(p.draft.iter().flat_map(|d| d.refs.iter().cloned()));
            for r in &p.runs {
                todo.extend(r.refs.iter().cloned());
                if let Mode::Edit { init: Some(init), .. } = &r.mode {
                    todo.push(init.clone());
                }
            }
        }
        while let Some(r) = todo.pop() {
            if !marked.insert(r.clone()) {
                continue;
            }
            if r.store == Store::Output
                && let Ok(Some(recipe)) = self.recipe(&r.id)
            {
                todo.extend(recipe.image_refs().cloned());
            }
        }
        let mut out = Vec::new();
        for store in [Store::Input, Store::Output] {
            for id in self.store_ids(store)? {
                let r = ImageRef { id, store };
                if !marked.contains(&r) {
                    out.push(r);
                }
            }
        }
        Ok(out)
    }

    /// **Clean up library**: moves every [unreferenced](Self::unreferenced)
    /// store image to the trash. Returns what was removed.
    pub fn clean_up(&mut self) -> Result<Vec<ImageRef>> {
        let gone = self.unreferenced()?;
        for r in &gone {
            self.dispose(&self.image_path(r))?;
            self.index_of(r.store).retain(|_, id| id != &r.id);
            self.pixels.transparent.remove(r);
        }
        self.save_pixel_index()?;
        Ok(gone)
    }
}

/// The images a usage check goes through ([`Library::usage_check`]).
pub struct UsageCheck {
    root: PathBuf,
    pub images: Vec<ImageRef>,
    used: HashMap<ImageRef, UsageCounts>,
}

impl UsageCheck {
    /// Whether the image needs nothing from [`Library::check_usage`]: gone,
    /// or its stored counts already cover the runs'. Only reads, so the check
    /// can do it without the library lock and lock only for the images that
    /// need a write (which `check_usage` reads again first).
    pub fn covers(&self, r: &ImageRef) -> bool {
        let path = r.path(&self.root);
        if !path.exists() {
            return true;
        }
        let derived = self.used.get(r).copied().unwrap_or_default();
        container::find_header_chunk(&path, png::KEY_USAGE)
            .is_ok_and(|found| stored_usage(&found).is_some_and(|s| s.max(derived) == s))
    }
}

/// The counts in a found `increart:usage` chunk, if whole and readable.
fn stored_usage(found: &Option<(u64, Chunk, bool)>) -> Option<UsageCounts> {
    found.as_ref().filter(|(_, _, ok)| *ok).and_then(|(_, c, _)| {
        let t = std::str::from_utf8(c.data.get(png::KEY_USAGE.len() + 1..)?).ok()?;
        UsageCounts::parse(t)
    })
}

/// What runs that generated (have a revision) add to images' usage counts:
/// one ref per run naming an image as a reference (however often), one start
/// per run starting from it. A run duplicated without seeds hasn't used them.
fn usage_of_runs<'a>(runs: impl IntoIterator<Item = &'a Run>) -> HashMap<ImageRef, UsageCounts> {
    let mut out: HashMap<ImageRef, UsageCounts> = HashMap::new();
    for run in runs.into_iter().filter(|r| !r.revisions.is_empty()) {
        for r in run.refs.iter().collect::<HashSet<_>>() {
            out.entry(r.clone()).or_default().refs += 1;
        }
        if let Mode::Edit { init: Some(init), .. } = &run.mode {
            out.entry(init.clone()).or_default().starts += 1;
        }
    }
    out
}
