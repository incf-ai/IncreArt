//! Types of the library's RSN files and PNG chunks (DESIGN.md, Ideas, Recipe,
//! Image stores, Filesystem layout). Struct names are the names written in
//! the files (`Idea { … }`, `Run { … }`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::id::{Id, Timestamp};
use crate::rsnfmt::bare;

pub const LIBRARY_FORMAT: u32 = 1;
/// A library whose stores may hold WebP files (`store_format: Webp`, or
/// some left from it): older versions, which read only PNG, refuse it.
pub const LIBRARY_FORMAT_WEBP: u32 = 2;
/// See [`LibraryMeta::metadata`].
pub const METADATA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Store {
    Input,
    Output,
}

impl Store {
    /// The store directory, relative to the library root.
    pub fn dir(self) -> &'static str {
        match self {
            Store::Input => "input_images",
            Store::Output => "output_images",
        }
    }

    pub fn from_dir(name: &str) -> Option<Store> {
        match name {
            "input_images" => Some(Store::Input),
            "output_images" => Some(Store::Output),
            _ => None,
        }
    }
}

/// How store images are encoded, chosen per library (DESIGN.md, Image
/// stores). Both are lossless: converting keeps the pixels exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StoreFormat {
    #[default]
    Png,
    /// Lossless WebP, about half the size of PNG.
    Webp,
}

impl StoreFormat {
    pub const ALL: [StoreFormat; 2] = [StoreFormat::Png, StoreFormat::Webp];

    pub fn ext(self) -> &'static str {
        match self {
            StoreFormat::Png => "png",
            StoreFormat::Webp => "webp",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            StoreFormat::Png => "PNG",
            StoreFormat::Webp => "WebP",
        }
    }

    /// The format a store file or image link has, by its extension.
    pub fn of_path(p: &Path) -> Option<StoreFormat> {
        Self::of_name(p.file_name()?.to_str()?).map(|(f, _)| f)
    }

    /// A store file or link name's format and stem.
    pub fn of_name(name: &str) -> Option<(StoreFormat, &str)> {
        Self::ALL.into_iter().find_map(|f| Some((f, name.strip_suffix(f.ext())?.strip_suffix('.')?)))
    }
}

fn is_png(f: &StoreFormat) -> bool {
    *f == StoreFormat::Png
}

/// A store image: at `<store>_images/<id>.png`, or `.webp`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename = "ImageRef")]
pub struct ImageRef {
    pub id: Id,
    pub store: Store,
}

impl ImageRef {
    pub fn input(id: Id) -> ImageRef {
        ImageRef { id, store: Store::Input }
    }

    pub fn output(id: Id) -> ImageRef {
        ImageRef { id, store: Store::Output }
    }

    /// The store file, `<store>_images/<id>.png` or `.webp` (a library can
    /// hold both); the PNG name when there is neither.
    pub fn path(&self, root: &Path) -> PathBuf {
        let png = root.join(self.rel_path_as(StoreFormat::Png));
        if png.exists() {
            return png;
        }
        let webp = root.join(self.rel_path_as(StoreFormat::Webp));
        if webp.exists() { webp } else { png }
    }

    /// [`path`](Self::path) relative to the library root, with `/`.
    pub fn rel_path(&self, root: &Path) -> String {
        let p = self.path(root);
        crate::fsutil::posix(p.strip_prefix(root).unwrap_or(&p))
    }

    /// The store file's path relative to the root when stored as `format`.
    pub fn rel_path_as(&self, format: StoreFormat) -> String {
        format!("{}/{}.{}", self.store.dir(), self.id, format.ext())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkMode {
    Symlink,
    Hardlink,
    Copy,
}

/// `library.rsn`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Library")]
pub struct LibraryMeta {
    pub format: u32,
    /// How new links are made. Missing (a library unzipped by a tool that
    /// may have turned links into copies) means probe and save on open.
    #[serde(default, serialize_with = "bare", skip_serializing_if = "Option::is_none")]
    pub link_mode: Option<LinkMode>,
    /// How far the store images' metadata has been brought up to date
    /// (1: `increart:blake3` everywhere, `inputs` in output recipes; 2: runs
    /// record their deleted output links).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub metadata: u32,
    /// The format new store images are written in; existing ones change
    /// only when the library is converted.
    #[serde(default, skip_serializing_if = "is_png")]
    pub store_format: StoreFormat,
}

fn no_time(v: &f32) -> bool {
    *v == 0.0
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// `idea.rsn`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Idea")]
pub struct Idea {
    pub id: Id,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub variants: Vec<PromptVariant>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub source: IdeaSource,
    pub created: Timestamp,
    /// Input store image IDs (authoritative; the folder's links mirror it).
    #[serde(default)]
    pub images: Vec<Id>,
    /// Projects it has been promoted into.
    #[serde(default)]
    pub projects: Vec<Id>,
    #[serde(default)]
    pub starred: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "PromptVariant")]
pub struct PromptVariant {
    pub text: String,
    pub origin: VariantOrigin,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VariantOrigin {
    Manual,
    Rewritten { model: String, system_prompt_hash: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IdeaSource {
    Manual,
    Clipboard,
    InboxFolder,
    DroppedFile {
        original_path: String,
    },
    FromOutput {
        image: ImageRef,
    },
    /// A tiled image made of these (Review's Tile…).
    Tiled {
        images: Vec<ImageRef>,
    },
    /// A cropped and marked copy of `image` (Crop / mark…); `edits` reopen
    /// it as it was drawn.
    Edited {
        image: ImageRef,
        edits: ImageEdits,
    },
}

/// A crop and marks drawn over an image, in the original's pixels
/// (`crate::markup` renders them).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "ImageEdits")]
pub struct ImageEdits {
    /// x, y, width, height; `None`: the whole image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<[u32; 4]>,
    #[serde(default)]
    pub marks: Vec<Mark>,
}

impl ImageEdits {
    pub fn is_empty(&self) -> bool {
        self.crop.is_none() && self.marks.is_empty()
    }
}

/// A line drawn over an image: its shape, colour (RGBA) and width in pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Mark")]
pub struct Mark {
    pub shape: Shape,
    pub color: [u8; 4],
    pub width: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    /// A freehand line through these points.
    Pencil(Vec<[f32; 2]>),
    /// Corners.
    Rect([f32; 2], [f32; 2]),
    /// Its bounding box's corners.
    Ellipse([f32; 2], [f32; 2]),
}

/// `project.rsn`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Project")]
pub struct Project {
    pub id: Id,
    pub created: Timestamp,
    /// The ideas it was promoted from.
    #[serde(default)]
    pub sources: Vec<Id>,
    #[serde(default, serialize_with = "bare", skip_serializing_if = "Option::is_none")]
    pub cover: Option<ImageRef>,
    /// How images taken from outputs (Edit, Use in project, …) are used.
    #[serde(default, skip_serializing_if = "EditWith::is_default")]
    pub edit_with: EditWith,
    /// Titles given to its output images, by image ID.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub titles: std::collections::BTreeMap<Id, String>,
}

/// A project's choice for images taken from outputs: as references
/// (`ref_images`, Qwen-Image's native editing; the default) or as the
/// img2img start image (`init_image` + strength).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum EditWith {
    #[default]
    References,
    Img2img {
        strength: f32,
    },
}

impl EditWith {
    pub fn is_default(&self) -> bool {
        *self == EditWith::References
    }
}

/// `runs/<run-id>/run.rsn`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Run")]
pub struct Run {
    pub id: Id,
    #[serde(default)]
    pub label: String,
    pub created: Timestamp,
    pub prompt: String,
    #[serde(default)]
    pub negative_prompt: String,
    #[serde(default)]
    pub alpha: bool,
    pub width: u32,
    pub height: u32,
    pub sample: Sample,
    pub mode: Mode,
    #[serde(default)]
    pub refs: Vec<ImageRef>,
    pub model: ModelSelection,
    #[serde(default, serialize_with = "bare")]
    pub source_idea: Option<Id>,
    /// Empty for a run copied as a template (nothing queued).
    #[serde(default)]
    pub revisions: Vec<Revision>,
    /// Seeds dropped by Cancel, never to be generated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cancelled: Vec<Seeds>,
    /// Seeds that failed for good (a bad request, a generation error).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<SeedFailure>,
    /// One style of a sweep (one Queue, a run per prompt preset).
    #[serde(default, serialize_with = "bare", skip_serializing_if = "Option::is_none")]
    pub sweep: Option<SweepTag>,
    /// How long each generated image took (DESIGN.md, Timing).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timings: Vec<SeedTiming>,
    /// Seeds whose output link was deleted: generated, and not missing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<Seeds>,
}

/// One generated image's timing, recorded when it is stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "SeedTiming")]
pub struct SeedTiming {
    pub rev: u32,
    pub seed: u64,
    /// The server's device (e.g. `ROCm0`).
    #[serde(default)]
    pub device: String,
    /// When it was stored.
    pub done: Timestamp,
    /// Generating, on the server (job started → completed).
    pub elapsed_s: f32,
    /// Waiting in the server's queue first (job created → started).
    #[serde(default)]
    pub queued_s: f32,
    /// Seconds per sampling step, timed from the progress reports; 0: not
    /// measured (no previews).
    #[serde(default)]
    pub step_s: f32,
}

/// Which sweep a run belongs to, and its preset or setting (DESIGN.md, Sweeps).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename = "Sweep")]
pub struct SweepTag {
    /// The sweep's label (`r05`), unique in its project.
    pub name: String,
    /// The prompt preset (a style sweep; empty in a setting sweep).
    pub task: String,
    pub preset: String,
    /// The setting this run has (a setting sweep: steps, CFG, …).
    #[serde(default, serialize_with = "bare", skip_serializing_if = "Option::is_none")]
    pub param: Option<SweepParam>,
    /// The style's prompt was prompt enhance's (perhaps edited after).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub enhanced: bool,
}

impl SweepTag {
    /// Its row on the contact sheet: the style, or the setting ("cfg 3").
    pub fn row(&self) -> String {
        self.param.as_ref().map_or_else(|| self.preset.clone(), SweepParam::label)
    }
}

/// A setting sweep's value for one run: `name` is `steps`, `cfg`,
/// `flow_shift`, `strength` or `model` (the model preset's name as `value`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename = "SweepParam")]
pub struct SweepParam {
    pub name: String,
    pub value: String,
}

impl SweepParam {
    /// "20 steps", "cfg 3", "shift 3.1", "strength 0.6", or the model.
    pub fn label(&self) -> String {
        let v = &self.value;
        match self.name.as_str() {
            "steps" => format!("{v} steps"),
            "cfg" => format!("cfg {v}"),
            "flow_shift" => format!("shift {v}"),
            "strength" => format!("strength {v}"),
            _ => v.clone(),
        }
    }
}

impl Run {
    /// One past the highest seed of any revision (the next seed to add).
    pub fn next_seed(&self) -> u64 {
        self.revisions.iter().map(|r| r.seeds.first + r.seeds.count).max().unwrap_or(0)
    }

    pub fn seed_count(&self) -> u64 {
        self.revisions.iter().map(|r| r.seeds.count).sum()
    }

    /// Every `(rev, seed)` of every revision.
    pub fn all_seeds(&self) -> impl Iterator<Item = (u32, u64)> + '_ {
        self.revisions.iter().flat_map(|r| (r.seeds.first..r.seeds.first + r.seeds.count).map(move |s| (r.rev, s)))
    }

    /// Seeds generated (they have a timing) whose output link wasn't deleted:
    /// each should have a link.
    pub fn generated_seeds(&self) -> impl Iterator<Item = (u32, u64)> + '_ {
        self.all_seeds().filter(|(_, s)| {
            self.timings.iter().any(|t| t.seed == *s)
                && !self.deleted.iter().any(|d| (d.first..d.first + d.count).contains(s))
        })
    }

    /// Seeds that are neither cancelled nor failed (generated or still to be).
    pub fn live_seeds(&self) -> impl Iterator<Item = (u32, u64)> + '_ {
        self.all_seeds().filter(|(_, s)| {
            !self.cancelled.iter().any(|c| (c.first..c.first + c.count).contains(s))
                && !self.failed.iter().any(|f| f.seed == *s)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "SeedFailure")]
pub struct SeedFailure {
    pub rev: u32,
    pub seed: u64,
    pub error: String,
}

/// Compresses seeds into consecutive ranges.
pub fn seed_ranges(mut seeds: Vec<u64>) -> Vec<Seeds> {
    seeds.sort_unstable();
    seeds.dedup();
    let mut out: Vec<Seeds> = vec![];
    for s in seeds {
        match out.last_mut() {
            Some(r) if r.first + r.count == s => r.count += 1,
            _ => out.push(Seeds { first: s, count: 1 }),
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Sample")]
pub struct Sample {
    pub method: String,
    /// Empty: the server's default for the recipe.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scheduler: String,
    pub steps: u32,
    pub cfg: f32,
    /// `None`: the recipe's own (resolution-dependent) flow schedule.
    #[serde(default, serialize_with = "bare", skip_serializing_if = "Option::is_none")]
    pub flow_shift: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_sigmas: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Mode {
    Generate,
    Edit {
        #[serde(default, serialize_with = "bare")]
        init: Option<ImageRef>,
        strength: f32,
    },
}

/// Recipe id + slot → model file (identified by the SHA-256 of its bytes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Model")]
pub struct ModelSelection {
    pub recipe: String,
    #[serde(default)]
    pub files: BTreeMap<String, ModelFile>,
    /// Applied per request, so not part of what a server loads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loras: Vec<LoraFile>,
}

impl ModelSelection {
    /// Its LoRAs for a details view (`style.safetensors 0.8, …`); None
    /// without any.
    pub fn loras_text(&self) -> Option<String> {
        (!self.loras.is_empty()).then(|| {
            let each: Vec<String> = self
                .loras
                .iter()
                .map(|l| format!("{} {}", crate::config::lora_request_path(&l.id), l.weight))
                .collect();
            each.join(", ")
        })
    }
}

/// A LoRA an image was made with: its ID (`loras/…`, see
/// `config::lora_file_id`), its file's SHA-256 (empty: not found), and its
/// weight.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Lora")]
pub struct LoraFile {
    pub id: String,
    #[serde(default)]
    pub sha256: String,
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "ModelFile")]
pub struct ModelFile {
    pub id: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Revision")]
pub struct Revision {
    pub rev: u32,
    pub created: Timestamp,
    pub seeds: Seeds,
}

/// Consecutive seeds `first .. first + count`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "Seeds")]
pub struct Seeds {
    pub first: u64,
    pub count: u64,
}

/// The `increart:recipe` chunk of an output image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Recipe")]
pub struct Recipe {
    pub prompt_original: String,
    /// After rewrite and/or the RGBA wrapper.
    pub prompt_sent: String,
    #[serde(default)]
    pub negative_prompt: String,
    pub width: u32,
    pub height: u32,
    pub seed: u64,
    #[serde(default)]
    pub alpha: bool,
    pub sample: Sample,
    pub mode: Mode,
    #[serde(default)]
    pub refs: Vec<ImageRef>,
    /// The image this was edited from.
    #[serde(default, serialize_with = "bare")]
    pub parent: Option<ImageRef>,
    #[serde(default, serialize_with = "bare")]
    pub source_idea: Option<Id>,
    #[serde(default, serialize_with = "bare")]
    pub rewrite: Option<Rewrite>,
    pub model: ModelSelection,
    #[serde(default)]
    pub sdcpp: String,
    #[serde(default)]
    pub increart: String,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub elapsed_s: f32,
    /// Waiting in the server's queue (s), and seconds per sampling step
    /// (0: not measured); as in [`SeedTiming`].
    #[serde(default, skip_serializing_if = "no_time")]
    pub queued_s: f32,
    #[serde(default, skip_serializing_if = "no_time")]
    pub step_s: f32,
    /// The images it was made from, with their files and BLAKE3 pixel
    /// hashes (filled in by the library when the output is stored).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputImage>,
}

/// One image an output was made from (`Recipe::inputs`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "InputImage")]
pub struct InputImage {
    pub role: InputRole,
    pub image: ImageRef,
    /// The store file, relative to the library root.
    pub file: String,
    /// The file it was imported from (input images).
    #[serde(default, serialize_with = "bare", skip_serializing_if = "Option::is_none")]
    pub original_name: Option<String>,
    /// BLAKE3 of its pixels (as its `increart:blake3`).
    pub blake3: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputRole {
    /// A reference, as `<imageN>` in the prompt.
    Ref(u32),
    /// The img2img start image.
    Start,
}

impl Recipe {
    /// Every store image this recipe points at (refs, edit init, parent).
    pub fn image_refs(&self) -> impl Iterator<Item = &ImageRef> {
        let init = match &self.mode {
            Mode::Edit { init, .. } => init.as_ref(),
            Mode::Generate => None,
        };
        self.refs.iter().chain(init).chain(self.parent.as_ref())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Rewrite")]
pub struct Rewrite {
    pub model: String,
    pub system_prompt_hash: String,
}

/// The `increart:source` chunk (JSON) of an input image: what the original
/// file was.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub version: u32,
    pub original_name: Option<String>,
    pub original_path: Option<String>,
    pub original_format: String,
    pub original_size: u64,
    pub original_sha256: String,
    pub original_mtime: Option<Timestamp>,
    /// Last status change of the original file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_ctime: Option<Timestamp>,
    /// Last access of the original file, before importing read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_atime: Option<Timestamp>,
    /// Birth time of the original file, where the filesystem keeps one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_created: Option<Timestamp>,
    /// Permission bits of the original file (`0o644` as 420).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_uid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_gid: Option<u32>,
    pub original_width: u32,
    pub original_height: u32,
    pub imported: Option<Timestamp>,
    #[serde(default)]
    pub conversions: Vec<String>,
}

/// `projects/<project>/draft.rsn`: the Generate panel's unsent form for a
/// project, autosaved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "GenerateDraft")]
pub struct GenerateDraft {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub negative_prompt: String,
    pub width: u32,
    pub height: u32,
    /// The model preset's name (`config.rsn` recipes).
    #[serde(default)]
    pub preset: String,
    pub steps: u32,
    pub cfg: f32,
    #[serde(default, serialize_with = "bare", skip_serializing_if = "Option::is_none")]
    pub flow_shift: Option<f32>,
    pub seeds: u64,
    #[serde(default)]
    pub first_seed: u64,
    #[serde(default)]
    pub alpha: bool,
    #[serde(default)]
    pub refs: Vec<ImageRef>,
    /// img2img start image (with `strength`), if any.
    #[serde(default, serialize_with = "bare", skip_serializing_if = "Option::is_none")]
    pub init: Option<ImageRef>,
    /// The Generate form's image list, in order, with each image's roles
    /// (`refs` and `init` above are what it sends; older drafts have only
    /// those).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<GenerateImage>,
    #[serde(default = "default_strength")]
    pub strength: f32,
    #[serde(default, serialize_with = "bare", skip_serializing_if = "Option::is_none")]
    pub source_idea: Option<Id>,
    /// Sweep mode's settings.
    #[serde(default, skip_serializing_if = "crate::prompts::Sweep::is_default")]
    pub sweep: crate::prompts::Sweep,
}

/// An image in the Generate form's list: a reference (`<imageN>`, numbered
/// in list order), the img2img start image, both, or neither (kept in the
/// list, not sent).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "Image")]
pub struct GenerateImage {
    pub image: ImageRef,
    #[serde(default)]
    pub reference: bool,
    #[serde(default)]
    pub start: bool,
}

pub fn default_strength() -> f32 {
    0.75
}

/// `input/draft.rsn`: the New idea form, autosaved until the idea is added.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "IdeaDraft")]
pub struct IdeaDraftFile {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tags: String,
    /// Input store images already imported for the draft.
    #[serde(default)]
    pub images: Vec<Id>,
}

/// `cache/pixel-index.rsn`: pixel hash → image ID, per store, and which
/// store images are transparent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "PixelIndex")]
pub struct PixelIndexFile {
    /// [`PIXEL_INDEX_VERSION`]; an index of another version is rebuilt.
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub input: BTreeMap<String, Id>,
    #[serde(default)]
    pub output: BTreeMap<String, Id>,
    /// Images that are [transparent](crate::pixel::Pixels::is_transparent).
    #[serde(default)]
    pub transparent: BTreeSet<ImageRef>,
}

/// 2: adds `transparent`.
pub const PIXEL_INDEX_VERSION: u32 = 2;

/// An output link's name, `<run-id>_r<rev>_s<seed>.png` (`.webp` for a
/// WebP store file), seed padded to at least 4 digits.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputName {
    pub run: Id,
    pub rev: u32,
    pub seed: u64,
}

impl OutputName {
    pub fn file_name(&self, format: StoreFormat) -> String {
        format!("{}_r{}_s{:04}.{}", self.run, self.rev, self.seed, format.ext())
    }

    pub fn parse(name: &str) -> Option<OutputName> {
        let (_, stem) = StoreFormat::of_name(name)?;
        let (run, rest) = stem.split_once("_r")?;
        let (rev, seed) = rest.split_once("_s")?;
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(rev) || !digits(seed) {
            return None;
        }
        Some(OutputName { run: run.parse().ok()?, rev: rev.parse().ok()?, seed: seed.parse().ok()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(
            seed_ranges(vec![5, 1, 2, 3, 7, 2]),
            [Seeds { first: 1, count: 3 }, Seeds { first: 5, count: 1 }, Seeds { first: 7, count: 1 }]
        );
    }

    #[test]
    fn output_names() {
        let n = OutputName { run: "2026.10.19-09.00-0000".parse().unwrap(), rev: 0, seed: 3 };
        assert_eq!(n.file_name(StoreFormat::Png), "2026.10.19-09.00-0000_r0_s0003.png");
        assert_eq!(OutputName::parse(&n.file_name(StoreFormat::Png)), Some(n.clone()));
        assert_eq!(n.file_name(StoreFormat::Webp), "2026.10.19-09.00-0000_r0_s0003.webp");
        assert_eq!(OutputName::parse(&n.file_name(StoreFormat::Webp)), Some(n.clone()));
        let big = OutputName { seed: 123456, ..n };
        assert_eq!(big.file_name(StoreFormat::Png), "2026.10.19-09.00-0000_r0_s123456.png");
        assert_eq!(OutputName::parse(&big.file_name(StoreFormat::Png)), Some(big));
        assert_eq!(OutputName::parse("2026.10.19-09.00-0000_r0_s1.jpg"), None);
        assert_eq!(OutputName::parse("2026.10.19-09.00-0000.png"), None);
        assert_eq!(OutputName::parse("x_r0_s1.png"), None);
    }
}
