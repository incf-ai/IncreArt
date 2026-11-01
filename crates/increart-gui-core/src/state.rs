//! Everything the GUI shows, and the lists derived from the library index.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use increart_core::HistoryKey;
use increart_core::config::{Config, RecipePreset, Resolution, ServerConfig, ServerPreset, TreeOrder};
use increart_core::history::{HistoryEntry, HistoryKind};
use increart_core::id::Timestamp;
use increart_core::model::{GenerateDraft, GenerateImage, IdeaDraftFile};
use increart_core::model::{IdeaSource, ImageEdits};
use increart_core::model::{Recipe, SourceInfo};
use increart_core::prompts::{PromptPreset, PromptPresets, SuggestAsk, Sweep};
use increart_core::scan::{IdeaEntry, OutputLink, ProjectEntry};
use increart_core::{Id, IdeaLoc, ImageRef, Index, Tree, Verdict};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    /// The welcome screen: how the tabs fit together, and buttons to start.
    Home,
    IdeaInbox,
    InputLibrary,
    Projects,
    Review,
    OutputLibrary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboxTab {
    NewIdea,
    Grid,
    Folder,
    Import,
    Discarded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibTab {
    Grid,
    Folder,
    Discarded,
}

/// Input library Grid filters (they apply on top of the folder).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibFilter {
    /// Matches title, text and tags.
    pub text: String,
    pub starred_only: bool,
    pub with_images_only: bool,
    /// `Some(true)`: ideas used in a project only; `Some(false)`: in none yet.
    pub used_in_project: Option<bool>,
    /// Most used first (as ref or start image), instead of newest first.
    pub most_used: bool,
    /// The edited copies (Crop / mark…) too, from `input/edited/`.
    pub show_hidden: bool,
}

/// An input image's metadata from its file: where it was imported from, and
/// its pixel hash.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputMeta {
    pub source: Option<SourceInfo>,
    pub blake3: Option<String>,
}

/// The right-side details panels; each starts shown (every session) and
/// can be hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InfoPanel {
    /// The Input library's idea.
    Library,
    /// The Output library's selected image (its lineage).
    Output,
    /// Review Grid's current image.
    Review,
    /// Projects → Grid and Folder: the selected project, run, image or
    /// folder.
    Projects,
}

/// Where an idea came from, as the Idea inbox's source filter groups it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdeaOrigin {
    Typed,
    Clipboard,
    InboxFolder,
    DroppedFile,
    /// From Review: an output, or a tile of several.
    FromOutput,
}

impl IdeaOrigin {
    pub const ALL: [IdeaOrigin; 5] = [
        IdeaOrigin::Typed,
        IdeaOrigin::Clipboard,
        IdeaOrigin::InboxFolder,
        IdeaOrigin::DroppedFile,
        IdeaOrigin::FromOutput,
    ];

    pub fn of(source: &IdeaSource) -> IdeaOrigin {
        match source {
            IdeaSource::Manual => IdeaOrigin::Typed,
            IdeaSource::Clipboard => IdeaOrigin::Clipboard,
            IdeaSource::InboxFolder => IdeaOrigin::InboxFolder,
            IdeaSource::DroppedFile { .. } => IdeaOrigin::DroppedFile,
            IdeaSource::FromOutput { .. } | IdeaSource::Tiled { .. } | IdeaSource::Edited { .. } => {
                IdeaOrigin::FromOutput
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            IdeaOrigin::Typed => "typed",
            IdeaOrigin::Clipboard => "clipboard",
            IdeaOrigin::InboxFolder => "inbox folder",
            IdeaOrigin::DroppedFile => "dropped file",
            IdeaOrigin::FromOutput => "from outputs",
        }
    }
}

/// How long ago an idea was added, as the Idea inbox's age filter groups it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdeaAge {
    /// The last 24 hours.
    Day,
    /// The last 7 days (the last day included).
    Week,
    /// Before that.
    Older,
}

impl IdeaAge {
    pub const ALL: [IdeaAge; 3] = [IdeaAge::Day, IdeaAge::Week, IdeaAge::Older];

    /// Whether an idea added at `created` is in this group at `now`.
    pub fn holds(self, created: Timestamp, now: Timestamp) -> bool {
        let age = now - created;
        match self {
            IdeaAge::Day => age <= chrono::Duration::days(1),
            IdeaAge::Week => age <= chrono::Duration::days(7),
            IdeaAge::Older => age > chrono::Duration::days(7),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            IdeaAge::Day => "last 24 hours",
            IdeaAge::Week => "last 7 days",
            IdeaAge::Older => "older",
        }
    }
}

/// Idea inbox Grid search and filters; they narrow the triage queue (the
/// cursor and the rating keys go through what's shown).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InboxFilter {
    /// Matches title, text, variants and tags.
    pub text: String,
    /// Only ideas with this tag.
    pub tag: Option<String>,
    /// `Some(true)`: with images only; `Some(false)`: text only.
    pub images: Option<bool>,
    pub origin: Option<IdeaOrigin>,
    pub age: Option<IdeaAge>,
    /// Oldest first, instead of newest first.
    pub oldest_first: bool,
}

impl InboxFilter {
    /// Whether any filter hides ideas (the order doesn't).
    pub fn narrows(&self) -> bool {
        !self.text.trim().is_empty()
            || self.tag.is_some()
            || self.images.is_some()
            || self.origin.is_some()
            || self.age.is_some()
    }

    /// Whether the idea passes, `now` for the age filter.
    pub fn matches(&self, i: &increart_core::model::Idea, now: Timestamp) -> bool {
        let q = self.text.trim().to_lowercase();
        (q.is_empty()
            || i.title.to_lowercase().contains(&q)
            || i.text.to_lowercase().contains(&q)
            || i.variants.iter().any(|v| v.text.to_lowercase().contains(&q))
            || i.tags.iter().any(|t| t.to_lowercase().contains(&q)))
            && self.tag.as_ref().is_none_or(|t| i.tags.contains(t))
            && self.images.is_none_or(|w| w == !i.images.is_empty())
            && self.origin.is_none_or(|o| o == IdeaOrigin::of(&i.source))
            && self.age.is_none_or(|a| a.holds(i.created, now))
    }
}

/// How often an image was used to make others, from the run records.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Runs that name it as a reference (`<imageN>`).
    pub ref_runs: usize,
    /// Runs that start from it (img2img).
    pub start_runs: usize,
    /// Images those runs made so far.
    pub images: usize,
}

impl Usage {
    pub fn runs(&self) -> usize {
        self.ref_runs + self.start_runs
    }

    /// "ref in 3 runs · start in 1 run · 14 images"; `None` if never used.
    pub fn label(&self) -> Option<String> {
        if self.runs() == 0 {
            return None;
        }
        let runs = |n: usize| if n == 1 { "1 run".to_string() } else { format!("{n} runs") };
        let mut parts = vec![];
        if self.ref_runs > 0 {
            parts.push(format!("ref in {}", runs(self.ref_runs)));
        }
        if self.start_runs > 0 {
            parts.push(format!("start in {}", runs(self.start_runs)));
        }
        parts.push(if self.images == 1 { "1 image".into() } else { format!("{} images", self.images) });
        Some(parts.join(" · "))
    }

    fn add(&mut self, o: Usage) {
        self.ref_runs += o.ref_runs;
        self.start_runs += o.start_runs;
        self.images += o.images;
    }
}

/// Output library Grid filters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputFilter {
    /// Matches the run's prompt.
    pub text: String,
    pub has_refs: bool,
    pub transparent: bool,
    /// The ratings shown (the preset tabs set them).
    pub ratings: Ratings,
}

impl OutputFilter {
    /// Whether a filter besides the ratings hides images.
    pub fn narrows(&self) -> bool {
        *self != OutputFilter { ratings: self.ratings, ..OutputFilter::default() }
    }
}

/// Generate's From library… picker: a full-window browser of the input
/// library's and the output library's images, to add as references (or,
/// for Compare with…, the one image to compare against).
#[derive(Clone, Debug, PartialEq)]
pub struct RefPicker {
    pub source: RefSource,
    /// Matches an idea's title, text and tags, or an output's prompt.
    pub text: String,
    /// Outputs: which ratings.
    pub ratings: Ratings,
    /// Chosen to add, in the order clicked.
    pub chosen: Vec<ImageRef>,
    pub target: PickTarget,
}

/// The title dialog: an output image and the title being typed.
#[derive(Clone, Debug, PartialEq)]
pub struct TitleEdit {
    pub image: ImageRef,
    pub text: String,
}

/// Where the reference picker is browsing.
#[derive(Clone, Debug, PartialEq)]
pub enum RefSource {
    /// The input library, or one of its folders (relative to
    /// `input/library/`).
    Inputs(Option<PathBuf>),
    /// The Idea inbox.
    Inbox,
    Outputs(OutputSource),
    /// What the Tile window's images were made from.
    TileInputs,
    /// What an image was made from: its start image and references.
    ImageInputs(ImageRef),
    /// What a project's runs were made from.
    ProjectInputs(Id),
    /// The edited copies (Crop / mark…), hidden from the input library.
    Edited,
}

/// Where the reference picker's choices go.
#[derive(Clone, Debug, PartialEq)]
pub enum PickTarget {
    /// Generate's references.
    Generate,
    /// The Tile window: from that cell on, else after its last tile.
    Tile(Option<(u32, u32)>),
    /// Compare with…: the image clicked becomes A, against Review's image
    /// (or the Output library's selected one) as B.
    Compare,
}

impl RefSource {
    /// Tied to the image or tiling it was opened for: not where the
    /// picker opens next time.
    pub fn contextual(&self) -> bool {
        matches!(self, RefSource::TileInputs | RefSource::ImageInputs(_) | RefSource::ProjectInputs(_))
    }
}

/// An image the reference picker shows.
#[derive(Clone, Debug, PartialEq)]
pub struct RefCandidate {
    pub image: ImageRef,
    /// The idea's title, or "project · run · seed".
    pub title: String,
    /// The idea's text, or the run's prompt.
    pub text: String,
    /// Outputs: the rating.
    pub verdict: Option<Option<Verdict>>,
}

/// Which ratings the Output library Grid shows; kept (good + neutral) by
/// default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ratings {
    pub good: bool,
    pub neutral: bool,
    pub bad: bool,
    pub unreviewed: bool,
}

impl Ratings {
    pub const KEPT: Ratings = Ratings { good: true, neutral: true, bad: false, unreviewed: false };
    pub const ALL: Ratings = Ratings { good: true, neutral: true, bad: true, unreviewed: true };

    pub fn allows(&self, v: Option<Verdict>) -> bool {
        match v {
            Some(Verdict::Good) => self.good,
            Some(Verdict::Neutral) => self.neutral,
            Some(Verdict::Bad) => self.bad,
            None => self.unreviewed,
        }
    }
}

impl Default for Ratings {
    fn default() -> Self {
        Ratings::KEPT
    }
}

/// The Output library Grid's preset tabs: a click sets the filters to the
/// preset's (its ratings, nothing else).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputPreset {
    #[default]
    Kept,
    Good,
    Rejected,
    Unreviewed,
    All,
}

impl OutputPreset {
    pub const EVERY: [OutputPreset; 5] =
        [OutputPreset::Kept, OutputPreset::Good, OutputPreset::Rejected, OutputPreset::Unreviewed, OutputPreset::All];

    pub fn label(self) -> &'static str {
        match self {
            OutputPreset::Kept => "Kept",
            OutputPreset::Good => "Good",
            OutputPreset::Rejected => "Rejected",
            OutputPreset::Unreviewed => "Unreviewed",
            OutputPreset::All => "All",
        }
    }

    pub fn ratings(self) -> Ratings {
        let none = Ratings { good: false, neutral: false, bad: false, unreviewed: false };
        match self {
            OutputPreset::Kept => Ratings::KEPT,
            OutputPreset::Good => Ratings { good: true, ..none },
            OutputPreset::Rejected => Ratings { bad: true, ..none },
            OutputPreset::Unreviewed => Ratings { unreviewed: true, ..none },
            OutputPreset::All => Ratings::ALL,
        }
    }

    pub fn filter(self) -> OutputFilter {
        OutputFilter { ratings: self.ratings(), ..OutputFilter::default() }
    }
}

/// Computed, read-only views of the kept images.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Smart {
    /// One day, from the image IDs (`YYYY.MM.DD`).
    Day(String),
    /// Made from runs of projects made from this idea.
    Idea(Id),
    /// Runs with Transparent on.
    Transparent,
}

/// The history window (Generate history, or an idea's text history).
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryView {
    pub key: HistoryKey,
    /// Newest first.
    pub entries: Vec<HistoryEntry>,
    /// By entry: the runs it was queued as (stored, or for older entries
    /// found by their label).
    pub runs: Vec<Vec<Id>>,
    /// By entry: the project's newest run before it (stored, or for older
    /// entries found by time).
    pub after: Vec<Option<Id>>,
    pub selected: usize,
    pub search: String,
    pub checkpoints_only: bool,
    /// Entries whose settings differ from the entry before.
    pub settings_only: bool,
    /// Only the entries that led to this run: (the run, the run before it).
    pub run_filter: Option<(Id, Option<Id>)>,
    /// One row per entry, instead of one per run of identical texts.
    pub every_entry: bool,
}

/// A row of the history window: one entry, or a run of entries in a row
/// with the same text (e.g. the same prompt queued several times).
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryRow<'a> {
    /// The newest entry's index (the one selected / restored).
    pub index: usize,
    pub entry: &'a HistoryEntry,
    /// Every entry in the row, newest first.
    pub indexes: Vec<usize>,
    /// Their kinds, oldest first, without repeats.
    pub kinds: Vec<&'a HistoryKind>,
}

impl HistoryView {
    pub fn new(key: HistoryKey) -> HistoryView {
        HistoryView {
            key,
            entries: vec![],
            runs: vec![],
            after: vec![],
            selected: 0,
            search: String::new(),
            checkpoints_only: false,
            settings_only: false,
            run_filter: None,
            every_entry: false,
        }
    }

    /// The form of the entry before `i` (older) that has one.
    pub fn previous_form(&self, i: usize) -> Option<&GenerateDraft> {
        self.entries.get(i + 1..)?.iter().find_map(|e| e.form.as_deref())
    }

    /// What changed in entry `i`'s settings since the entry before.
    pub fn changes(&self, i: usize) -> Vec<String> {
        match (self.previous_form(i), self.entries.get(i).and_then(|e| e.form.as_deref())) {
            (Some(a), Some(b)) => settings_changes(a, b),
            _ => vec![],
        }
    }

    /// The runs of entry `i`.
    pub fn runs_of(&self, i: usize) -> &[Id] {
        self.runs.get(i).map_or(&[], Vec::as_slice)
    }

    /// The rows shown (search and filters applied).
    pub fn shown(&self) -> Vec<HistoryRow<'_>> {
        let q = self.search.to_lowercase();
        let mut rows: Vec<HistoryRow> = vec![];
        let entries = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !self.checkpoints_only || e.kind.is_checkpoint())
            .filter(|(i, _)| !self.settings_only || !self.changes(*i).is_empty())
            .filter(|(i, _)| {
                self.run_filter.as_ref().is_none_or(|(run, before)| {
                    // the entries since the run before, up to this run's checkpoint
                    let queued = self.runs_of(*i);
                    if queued.is_empty() { self.after.get(*i) == Some(before) } else { queued.contains(run) }
                })
            })
            .filter(|(_, e)| q.is_empty() || e.text.to_lowercase().contains(&q) || e.extra.to_lowercase().contains(&q));
        for (i, e) in entries {
            match rows.last_mut() {
                Some(r) if !self.every_entry && r.entry.same_content(e) => {
                    r.indexes.push(i);
                    if !r.kinds.contains(&&e.kind) {
                        r.kinds.insert(0, &e.kind);
                    }
                }
                _ => rows.push(HistoryRow { index: i, entry: e, indexes: vec![i], kinds: vec![&e.kind] }),
            }
        }
        rows
    }
}

/// What differs between two Generate forms' settings, e.g. "steps 20 → 30",
/// "+1 image" (the prompt, label and first seed aside).
pub fn settings_changes(a: &GenerateDraft, b: &GenerateDraft) -> Vec<String> {
    let mut v = vec![];
    if (a.width, a.height) != (b.width, b.height) {
        v.push(format!("{}×{}", b.width, b.height));
    }
    if a.preset != b.preset {
        v.push(format!("model {}", b.preset));
    }
    if a.steps != b.steps {
        v.push(format!("steps {} → {}", a.steps, b.steps));
    }
    if a.cfg != b.cfg {
        v.push(format!("cfg {} → {}", a.cfg, b.cfg));
    }
    if a.flow_shift != b.flow_shift {
        let s = |f: Option<f32>| f.map_or("default".into(), |f| f.to_string());
        v.push(format!("flow shift {} → {}", s(a.flow_shift), s(b.flow_shift)));
    }
    if a.seeds != b.seeds {
        v.push(format!("seeds {} → {}", a.seeds, b.seeds));
    }
    let images = |d: &GenerateDraft| {
        if d.images.is_empty() { images_of(&d.refs, d.init.as_ref()) } else { d.images.clone() }
    };
    let (ia, ib) = (images(a), images(b));
    let added = ib.iter().filter(|x| !ia.iter().any(|y| y.image == x.image)).count();
    let removed = ia.iter().filter(|x| !ib.iter().any(|y| y.image == x.image)).count();
    if added > 0 {
        v.push(format!("+{added} image{}", if added == 1 { "" } else { "s" }));
    }
    if removed > 0 {
        v.push(format!("−{removed} image{}", if removed == 1 { "" } else { "s" }));
    }
    if added == 0 && removed == 0 && ia != ib {
        let order = |l: &[GenerateImage]| l.iter().map(|x| x.image.clone()).collect::<Vec<_>>();
        v.push(if order(&ia) != order(&ib) { "image order".into() } else { "image roles".into() });
    }
    if a.strength != b.strength && (a.init.is_some() || b.init.is_some() || ib.iter().any(|x| x.start)) {
        v.push(format!("strength {} → {}", a.strength, b.strength));
    }
    if a.source_idea != b.source_idea {
        v.push("source idea".into());
    }
    if a.sweep.on != b.sweep.on {
        v.push(if b.sweep.on { "sweep on".into() } else { "sweep off".into() });
    } else if a.sweep != b.sweep {
        v.push("sweep settings".into());
    }
    if a.alpha != b.alpha {
        v.push(if b.alpha { "transparent".into() } else { "opaque".into() });
    }
    v
}

/// What the reducer remembers of a history's newest entry (to merge edits).
#[derive(Clone, Debug, PartialEq)]
pub struct LastEntry {
    pub at: f64,
    pub entry: HistoryEntry,
}

/// The Edit idea window.
#[derive(Clone, Debug, PartialEq)]
pub struct IdeaEdit {
    pub id: Id,
    pub title: String,
    pub text: String,
    pub tags: String,
    pub images: Vec<Id>,
}

/// The Export… dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportDialog {
    pub images: Vec<ImageRef>,
    /// What is being exported, for the title ("3 images", "collection animals").
    pub what: String,
    pub dest: String,
    pub keep_metadata: bool,
    /// Name files by image ID (else run / revision / seed).
    pub by_id: bool,
}

/// Crop / mark… (DESIGN.md, Crop and mark): an image, what is drawn on it,
/// and where the copy goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Markup {
    /// The image drawn on (an edited copy reopens on its original, with
    /// its edits).
    pub image: ImageRef,
    pub edits: ImageEdits,
    pub undo: Vec<ImageEdits>,
    pub redo: Vec<ImageEdits>,
    /// Opened from this Generate image (project, index): the copy goes
    /// in next to it, or in its place when it is itself an edited copy.
    pub slot: Option<(Id, usize)>,
    pub replace: bool,
    /// The copy is being made.
    pub saving: bool,
}

/// Where a saved copy goes.
#[derive(Clone, Debug, PartialEq)]
pub enum EditedTo {
    /// Only into `input/edited/`.
    Library,
    /// Also into the selected project's Generate form, as a reference.
    UseInGenerate,
    /// Next to (or in place of) a Generate image: project, index, replace.
    Generate(Id, usize, bool),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarkTool {
    Crop,
    #[default]
    Pencil,
    Rect,
    Ellipse,
    Eraser,
}

/// The crop's aspect ratio.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CropAspect {
    #[default]
    Free,
    /// The original image's.
    Original,
    /// The Generate form's size.
    Generate,
}

/// The editor's tools, kept between images for the session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkTools {
    pub tool: MarkTool,
    /// Colour and opacity (alpha, in quarters: 25, 50, 75 or 100%).
    pub color: [u8; 4],
    /// Line width, in thousandths of the image's longer side.
    pub width: f32,
    pub aspect: CropAspect,
    /// Crop sides to multiples of 32.
    pub snap32: bool,
}

impl Default for MarkTools {
    fn default() -> Self {
        Self { tool: MarkTool::Pencil, color: [230, 30, 30, 255], width: 3.0, aspect: CropAspect::Free, snap32: false }
    }
}

/// The Tile dialog: picked images on a grid (DESIGN.md, Tiling); its
/// settings are `config.tile`.
#[derive(Clone, Debug, PartialEq)]
pub struct TileDialog {
    pub layout: crate::tile::TileLayout,
    /// The tile whose caption and span are edited.
    pub selected: Option<usize>,
    /// Compositions under way.
    pub working: usize,
    /// Opened from Generate: its img2img start image (`{kind}` "start").
    pub start: Option<ImageRef>,
}

/// What "Captions from" fills every tile's caption with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionFill {
    Clear,
    /// The sweep row (its style or setting: "cfg 3"), else the run label.
    Style,
    Seed,
    Title,
    Run,
    Prompt,
    /// "20 steps · cfg 3", and flow shift and strength when set.
    Settings,
    /// The sweep row and the seed: "cfg 3 · seed 5".
    StyleSeed,
}

/// What a caption's `{placeholders}` can be (DESIGN.md, Tiling).
pub const CAPTION_FIELDS: [(&str, &str); 13] = [
    ("n", "the tile's number"),
    ("style", "the sweep row (style or setting), else the run label"),
    ("seed", "the seed"),
    ("settings", "\"20 steps · cfg 3 · …\""),
    ("steps", "steps"),
    ("cfg", "cfg"),
    ("shift", "flow shift (if set)"),
    ("strength", "edit strength (if an edit)"),
    ("title", "the image's title"),
    ("run", "the run label"),
    ("prompt", "the prompt"),
    ("project", "the project's name"),
    ("kind", "output, input or pasted"),
];

/// A run's settings in a line: "20 steps · cfg 3 · shift 3.1 · strength 0.6".
pub fn settings_line(r: &increart_core::model::Run) -> String {
    let fmt = crate::update::fmt_value;
    let mut v = vec![format!("{} steps", r.sample.steps), format!("cfg {}", fmt(r.sample.cfg))];
    if let Some(x) = r.sample.flow_shift {
        v.push(format!("shift {}", fmt(x)));
    }
    if let increart_core::model::Mode::Edit { strength, init: Some(_) } = &r.mode {
        v.push(format!("strength {}", fmt(*strength)));
    }
    v.join(" · ")
}

impl CaptionFill {
    pub const EVERY: [CaptionFill; 8] = [
        CaptionFill::Style,
        CaptionFill::StyleSeed,
        CaptionFill::Seed,
        CaptionFill::Settings,
        CaptionFill::Title,
        CaptionFill::Run,
        CaptionFill::Prompt,
        CaptionFill::Clear,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CaptionFill::Clear => "none",
            CaptionFill::Style => "style / setting",
            CaptionFill::StyleSeed => "style / setting · seed",
            CaptionFill::Settings => "settings",
            CaptionFill::Seed => "seed",
            CaptionFill::Title => "title",
            CaptionFill::Run => "run label",
            CaptionFill::Prompt => "prompt",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectsTab {
    Grid,
    Folder,
    Generate,
    Jobs,
    /// Discarded projects: restore or delete for good.
    Discarded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FvMode {
    Icons,
    /// Small icons and names in columns.
    Compact,
    Details,
}

/// Which of the four folder views (DESIGN.md, Folder view): one per tab
/// that has a Folder subtab, each with its own location and history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FvPlace {
    Projects,
    /// The Output library: collections and the virtual `smart/` folder.
    Collections,
    InputLibrary,
    /// The Idea inbox: `input/` without the library (`inbox_drop/`,
    /// `unreviewed/`, `discarded/`).
    Inbox,
}

impl FvPlace {
    pub const ALL: [FvPlace; 4] = [FvPlace::Projects, FvPlace::Collections, FvPlace::InputLibrary, FvPlace::Inbox];

    /// Its top folder, relative to the library root.
    pub fn root(self) -> &'static str {
        match self {
            FvPlace::Projects => "projects",
            FvPlace::Collections => "collections",
            FvPlace::InputLibrary => "input/library",
            FvPlace::Inbox => "input",
        }
    }

    /// The library tree its folders are (the inbox isn't one).
    pub fn tree(self) -> Option<Tree> {
        match self {
            FvPlace::Projects => Some(Tree::Projects),
            FvPlace::Collections => Some(Tree::Collections),
            FvPlace::InputLibrary => Some(Tree::InputLibrary),
            FvPlace::Inbox => None,
        }
    }
}

/// What a folder-view entry is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    Project,
    /// `runs/<run-id>/`.
    Run,
    /// A link to a store image.
    Image(ImageRef),
    /// `*.rsn`, `*.jsonl` (shown with Ctrl+H).
    Meta,
    File,
    /// A folder of the Output library's virtual, read-only `smart/` tree.
    Smart,
    /// An idea's folder (`idea.rsn` and links to its images).
    Idea(Id),
}

impl EntryKind {
    pub fn is_dir(&self) -> bool {
        matches!(self, EntryKind::Folder | EntryKind::Project | EntryKind::Run | EntryKind::Smart | EntryKind::Idea(_))
    }
}

/// The Output library folder view's virtual folder of smart folders.
pub const SMART_DIR: &str = "collections/smart";

/// Whether `path` (relative to the library root) is in the virtual
/// `smart/` tree.
pub fn is_smart(path: &Path) -> bool {
    path.starts_with(SMART_DIR)
}

#[derive(Clone, Debug, PartialEq)]
pub struct FvEntry {
    pub name: String,
    pub kind: EntryKind,
    /// Bytes for a file, entries for a directory.
    pub size: u64,
    pub modified: Option<chrono::DateTime<chrono::Utc>>,
    /// A short description (run label, verdict, …).
    pub info: String,
}

/// The nearest multiple of 32 (sd.cpp's latent grid for Qwen-Image), at
/// least 32.
pub fn round32(v: u32) -> u32 {
    ((v + 16) / 32 * 32).max(32)
}

/// An output size with the aspect of a `w` × `h` image: kept within
/// 256…2048 on both sides, rounded to multiples of 32.
pub fn fit_size(w: u32, h: u32) -> (u32, u32) {
    let (wf, hf) = (w.max(1) as f32, h.max(1) as f32);
    let down = (2048.0 / wf.max(hf)).min(1.0);
    let up = (256.0 / (wf * down).min(hf * down)).max(1.0);
    let s = down * up;
    let side = |v: f32| round32((v * s).round() as u32).clamp(256, 2048);
    (side(wf), side(hf))
}

/// What the Scale dialog resizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleTarget {
    /// The Generate form's size.
    Generate,
    /// A row of the resolutions editor.
    Resolution(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleBy {
    Megapixels,
    Width,
    Height,
}

/// The model presets editor (DESIGN.md, Model presets): a working copy of
/// `config.recipes`.
#[derive(Clone, Debug, PartialEq)]
pub struct RecipeEditor {
    pub rows: Vec<RecipeRow>,
    pub selected: usize,
    /// A file picked outside the model folders, said in the editor.
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecipeRow {
    pub preset: RecipePreset,
    /// Its index in `config.recipes` (`None`: added here), so Generate keeps
    /// its model through renames and moves.
    pub origin: Option<usize>,
    /// The custom sigmas as typed (comma-separated; empty: the recipe's).
    pub sigmas: String,
}

impl RecipeEditor {
    pub fn new(recipes: &[RecipePreset], selected: usize) -> RecipeEditor {
        let rows = recipes
            .iter()
            .enumerate()
            .map(|(i, p)| RecipeRow {
                preset: p.clone(),
                origin: Some(i),
                sigmas: p.custom_sigmas.iter().map(f32::to_string).collect::<Vec<_>>().join(", "),
            })
            .collect();
        RecipeEditor { rows, selected: selected.min(recipes.len().saturating_sub(1)), error: None }
    }

    /// Why it can't be saved, if it can't.
    pub fn problem(&self) -> Option<String> {
        let mut names = BTreeSet::new();
        for r in &self.rows {
            let p = &r.preset;
            let name = p.name.trim();
            if name.is_empty() {
                return Some("A model preset has no name".into());
            }
            if !names.insert(name) {
                return Some(format!("Two model presets are called {name}"));
            }
            if p.recipe.trim().is_empty() {
                return Some(format!("{name} has no recipe"));
            }
            if !p.files.contains_key("diffusion_model") && !p.files.contains_key("model") {
                return Some(format!("{name} has no diffusion model file"));
            }
            if let Some((slot, _)) = p.files.iter().find(|(_, f)| f.trim().is_empty()) {
                return Some(format!("{name}: no file for {slot}"));
            }
            if parse_sigmas(&r.sigmas).is_none() {
                return Some(format!("{name}: the sigmas aren't numbers"));
            }
        }
        if self.rows.is_empty() { Some("Keep at least one model preset".into()) } else { None }
    }
}

/// Comma- (or space-) separated sigmas; `None` if one isn't a number.
pub fn parse_sigmas(text: &str) -> Option<Vec<f32>> {
    text.split([',', ' ']).filter(|t| !t.trim().is_empty()).map(|t| t.trim().parse().ok()).collect()
}

/// The Scale dialog: a new size with the same aspect, from a megapixel
/// count or one side.
#[derive(Clone, Debug, PartialEq)]
pub struct ScaleDialog {
    pub target: ScaleTarget,
    /// The size being scaled.
    pub width: u32,
    pub height: u32,
    pub by: ScaleBy,
    pub megapixels: f32,
    /// The width or height asked for.
    pub side: u32,
}

impl ScaleDialog {
    pub fn new(target: ScaleTarget, width: u32, height: u32) -> ScaleDialog {
        let megapixels = (width as f32 * height as f32 / 1e6 * 100.0).round() / 100.0;
        ScaleDialog { target, width, height, by: ScaleBy::Megapixels, megapixels, side: width }
    }

    /// The smallest and largest side allowed for the target.
    pub fn limits(&self) -> (u32, u32) {
        match self.target {
            ScaleTarget::Generate => (256, 2048),
            ScaleTarget::Resolution(_) => (32, 4096),
        }
    }

    pub fn result(&self) -> (u32, u32) {
        let value = match self.by {
            ScaleBy::Megapixels => self.megapixels as f64,
            _ => self.side as f64,
        };
        scaled(self.width, self.height, self.by, value, self.limits())
    }
}

/// `w` × `h` scaled, keeping its aspect, to `value` megapixels or to that
/// width / height; both sides rounded to multiples of 32 within `limits`.
pub fn scaled(w: u32, h: u32, by: ScaleBy, value: f64, limits: (u32, u32)) -> (u32, u32) {
    let r = w.max(1) as f64 / h.max(1) as f64;
    let (nw, nh) = match by {
        ScaleBy::Megapixels => {
            let nh = (value.max(0.0) * 1e6 / r).sqrt();
            (nh * r, nh)
        }
        ScaleBy::Width => (value, value / r),
        ScaleBy::Height => (value * r, value),
    };
    let side = |v: f64| round32(v.round() as u32).clamp(limits.0, limits.1);
    (side(nw), side(nh))
}

impl FvEntry {
    /// What the view shows: an idea's title (its folder is named by its
    /// ID), else the name.
    pub fn label(&self) -> &str {
        match self.kind {
            EntryKind::Idea(_) if !self.info.is_empty() && self.info != "idea" => &self.info,
            _ => &self.name,
        }
    }
}

/// A file-manager view of part of the library (DESIGN.md, Projects → Folder).
#[derive(Clone, Debug, PartialEq)]
pub struct FolderView {
    pub place: FvPlace,
    /// The top it can't go above (relative to the library root).
    pub root: PathBuf,
    /// Where it is (relative to the library root).
    pub path: PathBuf,
    pub back: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
    pub entries: Vec<FvEntry>,
    /// The path `entries` were listed for (`None`: list again).
    pub listed: Option<PathBuf>,
    pub selected: BTreeSet<String>,
    /// For Shift-click ranges.
    pub anchor: Option<String>,
    pub mode: FvMode,
    pub icon_size: f32,
    pub show_meta: bool,
    /// Ctrl+L: the typed path.
    pub path_edit: Option<String>,
    /// A few images from inside each folder, drawn behind its icon. Asked
    /// for only once a folder's tile is on screen.
    pub previews: BTreeMap<String, Vec<ImageRef>>,
    pub previews_asked: BTreeSet<String>,
    /// The folder `previews` are for.
    pub previews_for: PathBuf,
    /// Where the arrow keys move from (the last entry clicked or reached).
    pub cursor: Option<String>,
    /// Typed letters jump to the first matching name; a pause starts over.
    pub typeahead: String,
    pub typeahead_at: f64,
}

impl FolderView {
    pub fn new(place: FvPlace) -> FolderView {
        FolderView {
            place,
            root: place.root().into(),
            path: place.root().into(),
            back: vec![],
            forward: vec![],
            entries: vec![],
            listed: None,
            selected: BTreeSet::new(),
            anchor: None,
            mode: FvMode::Icons,
            icon_size: 112.0,
            show_meta: false,
            path_edit: None,
            previews: BTreeMap::new(),
            previews_asked: BTreeSet::new(),
            previews_for: PathBuf::new(),
            cursor: None,
            typeahead: String::new(),
            typeahead_at: f64::NEG_INFINITY,
        }
    }

    pub fn tree(&self) -> Option<Tree> {
        self.place.tree()
    }

    /// `path` relative to the tree's top (for the tree operations).
    pub fn tree_rel(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root).map(Path::to_path_buf).unwrap_or_default()
    }

    /// Entries shown (metadata files only with Ctrl+H).
    pub fn shown(&self) -> Vec<&FvEntry> {
        self.entries.iter().filter(|e| self.show_meta || e.kind != EntryKind::Meta).collect()
    }
}

/// Copied or cut entries (library-relative paths).
#[derive(Clone, Debug, PartialEq)]
pub struct FvClipboard {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewView {
    Single,
    Grid,
    /// A (pinned) next to B (the current image).
    Compare,
    /// A sweep's contact sheet: a row per style, a column per seed.
    Sheet,
}

/// The contact sheet's cell sizes (points); the largest is the thumbnails'
/// own size, so cells stay sharp.
pub const SHEET_CELL_MIN: u32 = 64;
pub const SHEET_CELL_DEFAULT: u32 = 128;
pub const SHEET_CELL_MAX: u32 = increart_core::thumbs::SIZE;

/// An image grid with its own zoom (the contact sheet has its own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomGrid {
    Review,
    Output,
    /// The Input library's idea cards (their width).
    Library,
    /// The Idea inbox's idea cards (their width).
    Inbox,
}

/// The image grids' tile sizes (points): smaller can't be judged, larger
/// than the thumbnails would blur.
pub const GRID_CELL_MIN: u32 = 96;
pub const GRID_CELL_MAX: u32 = increart_core::thumbs::SIZE;

impl ZoomGrid {
    pub fn default_size(self) -> u32 {
        match self {
            ZoomGrid::Review => 200,
            ZoomGrid::Output => 180,
            ZoomGrid::Library | ZoomGrid::Inbox => 210,
        }
    }
}

/// The interface zoom (percent): its range, and the step of the status
/// bar's buttons and Ctrl + / Ctrl −.
pub const UI_ZOOM_MIN: u32 = 70;
pub const UI_ZOOM_MAX: u32 = 300;
pub const UI_ZOOM_STEP: u32 = 5;

/// A sweep as a table (Review → Sheet): rows in run order, columns the
/// seeds of any of its runs.
#[derive(Debug)]
pub struct Sheet<'a> {
    pub seeds: Vec<u64>,
    /// The rows the name filter shows.
    pub rows: Vec<SheetRow<'a>>,
    /// Rows the name filter hides.
    pub hidden: usize,
}

#[derive(Debug)]
pub struct SheetRow<'a> {
    pub run: &'a increart_core::model::Run,
    pub cells: Vec<SheetCell<'a>>,
}

#[derive(Debug)]
pub enum SheetCell<'a> {
    /// The newest revision's image of that seed.
    Image(&'a OutputLink),
    /// Asked for, not generated yet.
    Pending,
    /// Not asked for, cancelled or failed.
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputTab {
    Grid,
    Folder,
    /// Discarded images: restore or delete for good.
    Discarded,
}

/// What the Output library's Grid shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputSource {
    AllProjects,
    Project(Id),
    /// Every project under a project folder (relative to `projects/`,
    /// empty = all projects).
    ProjectFolder(PathBuf),
    /// A collection folder (relative to `collections/`, empty = all).
    Collection(PathBuf),
    Smart(Smart),
    /// One run's images (from the history window).
    Run(Id),
}

/// A question with a button named for what it does (`verb`) and Cancel;
/// the button sends `then`. `fix`: another way out, offered first (its
/// button's name and message).
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    pub text: String,
    pub verb: String,
    pub then: Box<crate::Msg>,
    pub fix: Option<(String, Box<crate::Msg>)>,
    /// Its heading, for a question that isn't "are you sure": its verb is
    /// then what's recommended, not what can't be undone.
    pub heading: Option<String>,
}

impl Confirm {
    pub fn new(text: impl Into<String>, verb: impl Into<String>, then: crate::Msg) -> Self {
        Self { text: text.into(), verb: verb.into(), then: Box::new(then), fix: None, heading: None }
    }

    /// Asks under `heading`, `verb` recommended.
    pub fn recommending(mut self, heading: impl Into<String>) -> Self {
        self.heading = Some(heading.into());
        self
    }

    pub fn with_fix(mut self, name: impl Into<String>, then: crate::Msg) -> Self {
        self.fix = Some((name.into(), Box::new(then)));
        self
    }
}

/// The log viewer: the app's log and the started servers' (DESIGN.md,
/// Server logs).
#[derive(Clone, Debug, PartialEq)]
pub struct LogsView {
    /// The log shown: its file name in the log folder.
    pub file: Option<String>,
    /// Found in the lines shown (any case): marked, Enter going from one to
    /// the next.
    pub search: String,
    /// Only the lines with `search` in them.
    pub only_found: bool,
    pub wrap: bool,
    /// Keep to the end as the log grows.
    pub follow: bool,
    pub launch: LaunchShown,
    /// Only warnings and errors (and the app's lines), with the lines
    /// around them.
    pub trouble: bool,
}

/// Which of a log's launches the log viewer shows.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum LaunchShown {
    /// From the last launch line on.
    #[default]
    Latest,
    /// The launch with this launch line.
    Of(String),
    All,
}

impl LogsView {
    pub fn new(file: Option<String>) -> Self {
        LogsView {
            file,
            search: String::new(),
            only_found: false,
            wrap: false,
            follow: true,
            launch: LaunchShown::Latest,
            trouble: false,
        }
    }
}

/// Start-up: pick a library, then servers, then the main window.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Library(LibraryDialog),
    Servers(ServerDialog),
    Main,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryDialog {
    pub path: String,
    pub error: Option<String>,
    pub busy: bool,
    /// The wizard open over the dialog: create a library at `path`, or open
    /// the one there.
    pub wizard: Option<LibraryWizard>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryWizard {
    Create,
    Open,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServerDialog {
    /// What the editor is working on, while it is open.
    pub editing: Option<Editing>,
    /// The editor's working copy, and the servers being opened.
    pub draft: ServerPreset,
    /// `sd-server --list-devices`, once asked.
    pub devices: Option<Result<Vec<String>, String>>,
    /// `koboldcpp --version` for this binary, once asked.
    pub kobold_version: Option<(String, Result<String, String>)>,
    /// Per server of the draft while opening and after.
    pub progress: Vec<ServerProgress>,
    pub opening: bool,
    /// When opening started (`AppState::now`), for "starting — 12 s".
    pub started: f64,
    /// The setup wizard (no presets yet): builds `draft` from the listed
    /// devices, or a server to connect to.
    pub wizard: bool,
    /// The wizard's KoboldCpp page (the sd-server one without).
    pub kobold: Option<Box<KoboldWizard>>,
    /// Opened from the main window: it can be closed again, keeping the
    /// servers in use.
    pub from_main: bool,
    /// The server settings window (KoboldCpp, sd-server, model directory).
    pub settings: bool,
    /// Why the command line's `--servers` didn't skip this screen.
    pub notice: Option<String>,
    /// The **Continue without servers** dialog.
    pub without: Option<Box<Without>>,
}

/// The setup wizard's KoboldCpp page: a preset by VRAM per GPU
/// (`increart_core::tiers`), its models downloaded, then saved as a
/// server preset and a recipe and opened.
#[derive(Clone, Debug, PartialEq)]
pub struct KoboldWizard {
    pub binary: String,
    /// Where the models go (in `koboldcpp/`); added to the model folders.
    pub models_dir: String,
    /// The GPUs listed; None while listing.
    pub gpus: Option<Result<Vec<WizardGpu>, String>>,
    /// "Choose presets myself": each GPU's `preset` counts.
    pub choose: bool,
    /// The RAM available, for the offloaded weights (GiB).
    pub ram_gib: Option<f64>,
    pub layout: increart_core::tiers::Layout,
    /// The GPUs for prompt enhance (by number), when ticked by hand.
    pub pe_gpus: Vec<u32>,
    /// `pe_gpus` was ticked by hand; else [`KoboldWizard::default_pe_gpus`].
    pub pe_by_hand: bool,
    /// How KoboldCpp reaches the GPUs (and how they're numbered).
    pub runtime: increart_core::pe::GpuRuntime,
    /// Downloading the models for this plan.
    pub downloading: Option<(increart_core::tiers::Plan, WizardDownload)>,
    /// Opened once downloaded and saved; not with **Save anyway** (no
    /// KoboldCpp found yet).
    pub open: bool,
    pub error: Option<String>,
}

/// A GPU in the wizard.
#[derive(Clone, Debug, PartialEq)]
pub struct WizardGpu {
    pub gpu: increart_core::tiers::Gpu,
    pub on: bool,
    /// The preset chosen (card size, GB); None: by its memory.
    pub preset: Option<u32>,
    /// Added by hand: its memory is typed in.
    pub by_hand: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WizardDownload {
    /// Still finding what's there already (each file's size asked, the ones
    /// on disk linked) before any is fetched.
    pub checking: bool,
    /// Index into the plan's downloads.
    pub file: usize,
    pub done: u64,
    pub total: Option<u64>,
    /// All the files to fetch: bytes so far (a `.part` carried on counts)
    /// and in all (None: a size the server didn't say).
    pub all_done: u64,
    pub all_total: Option<u64>,
    /// Bytes a second fetched so far this run (None: none yet).
    pub rate: Option<f64>,
    /// Files not fetched: linked to one on disk (file, where it is), or
    /// there already (where empty).
    pub reused: Vec<(String, String)>,
}

impl WizardDownload {
    /// Seconds left at the rate so far.
    pub fn eta_secs(&self) -> Option<f64> {
        let left = self.all_total?.saturating_sub(self.all_done);
        self.rate.filter(|r| *r > 0.0).map(|r| left as f64 / r)
    }
}

impl KoboldWizard {
    /// The ticked GPUs, each with the preset chosen (if choosing).
    pub fn ticked(&self) -> Vec<(increart_core::tiers::Gpu, Option<u32>)> {
        match &self.gpus {
            Some(Ok(list)) => {
                list.iter().filter(|g| g.on).map(|g| (g.gpu.clone(), g.preset.filter(|_| self.choose))).collect()
            }
            _ => vec![],
        }
    }

    pub fn plan(&self) -> Result<increart_core::tiers::Plan, String> {
        self.plan_as(self.layout)
    }

    /// The plan with `layout` (for showing each choice).
    pub fn plan_as(&self, layout: increart_core::tiers::Layout) -> Result<increart_core::tiers::Plan, String> {
        let binary = if self.binary.trim().is_empty() { "koboldcpp" } else { self.binary.trim() };
        let dir = std::path::Path::new(self.models_dir.trim());
        let pe =
            if self.pe_by_hand && layout == self.layout { self.pe_gpus.clone() } else { self.default_pe_gpus(layout) };
        increart_core::tiers::plan(&self.ticked(), layout, &pe, self.runtime, binary, dir, self.ram_gib)
    }

    /// Prompt enhance's GPUs unless ticked by hand: with images on every
    /// GPU, each whose preset has room to keep the chat model loaded beside
    /// the image model; none listed (the layout's default: the biggest card,
    /// or the next) when none has, or with a GPU of its own.
    pub fn default_pe_gpus(&self, layout: increart_core::tiers::Layout) -> Vec<u32> {
        use increart_core::tiers::{Layout, tier, tier_for};
        if layout != Layout::ImagePerCard {
            return vec![];
        }
        self.ticked()
            .iter()
            .filter(|(g, chosen)| chosen.and_then(tier).or_else(|| tier_for(g.vram_gib)).is_some_and(|t| !t.swap))
            .map(|(g, _)| g.number)
            .collect()
    }
}

/// **Continue without servers**: the chat server to use meanwhile, if any.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Without {
    pub chat: Option<increart_core::pe::PeServer>,
}

/// The command line's choice for the servers screen, applied (if valid)
/// once the library opens.
#[derive(Clone, Debug, PartialEq)]
pub enum StartupServers {
    /// A preset by name (any case), or `last` for the last one opened.
    Preset(String),
    Without,
}

/// The server dialog's editor: a preset (index into
/// `config.server_presets`), or a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editing {
    Preset(usize),
    New,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServerProgress {
    Opening,
    Up,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IdeaDraft {
    pub title: String,
    pub text: String,
    /// Comma-separated.
    pub tags: String,
    pub images: Vec<Id>,
}

/// Seconds without edits before a draft is autosaved.
pub const AUTOSAVE_DELAY: f64 = 2.0;
/// Prompt history: an edit within this many seconds of the previous edit…
pub const HISTORY_MERGE_SECS: f64 = 60.0;
/// …changing fewer characters than this replaces it instead of adding one.
pub const HISTORY_MERGE_CHARS: usize = 20;

impl IdeaDraft {
    pub fn to_file(&self) -> IdeaDraftFile {
        IdeaDraftFile {
            title: self.title.clone(),
            text: self.text.clone(),
            tags: self.tags.clone(),
            images: self.images.clone(),
        }
    }

    pub fn from_file(f: &IdeaDraftFile) -> IdeaDraft {
        IdeaDraft { title: f.title.clone(), text: f.text.clone(), tags: f.tags.clone(), images: f.images.clone() }
    }

    pub fn tag_list(&self) -> Vec<String> {
        self.tags.split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.title.trim().is_empty() && self.text.trim().is_empty() && self.images.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenerateForm {
    pub label: String,
    pub prompt: String,
    pub negative: String,
    pub width: u32,
    pub height: u32,
    /// Index into `config.recipes`.
    pub preset: usize,
    pub steps: u32,
    pub cfg: f32,
    pub flow_shift: Option<f32>,
    pub seeds: u64,
    pub first_seed: u64,
    /// The run's images in order, each a reference (`<image1>`, `<image2>`,
    /// … numbered in this order), the img2img start image (at most one),
    /// both, or neither (parked: kept here, not sent).
    pub images: Vec<GenerateImage>,
    /// For the start image.
    pub strength: f32,
    pub source_idea: Option<Id>,
    /// Sweep mode: a run per chosen prompt preset.
    pub sweep: Sweep,
}

/// Whether a run of this project belongs to the named sweep.
/// A row of Review's scope panel under a project.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReviewRun<'a> {
    Single(&'a increart_core::model::Run),
    Sweep(&'a str),
}

/// A run's start image, then its references, added to `v` unless there.
fn run_inputs(run: &increart_core::model::Run, v: &mut Vec<ImageRef>) {
    let init = match &run.mode {
        increart_core::model::Mode::Edit { init, .. } => init.clone(),
        increart_core::model::Mode::Generate => None,
    };
    for i in init.into_iter().chain(run.refs.iter().cloned()) {
        if !v.contains(&i) {
            v.push(i);
        }
    }
}

fn in_sweep(p: &ProjectEntry, run: &Id, sweep: &str) -> bool {
    p.runs.iter().any(|r| &r.id == run && r.sweep.as_ref().is_some_and(|s| s.name == sweep))
}

/// The image list for references and a start image (older drafts and runs
/// keep them apart): the references in order, the start image marked on its
/// reference or added after them.
pub fn images_of(refs: &[ImageRef], init: Option<&ImageRef>) -> Vec<GenerateImage> {
    let mut v: Vec<GenerateImage> =
        refs.iter().map(|r| GenerateImage { image: r.clone(), reference: true, start: Some(r) == init }).collect();
    if let Some(i) = init
        && !refs.contains(i)
    {
        v.push(GenerateImage { image: i.clone(), reference: false, start: true });
    }
    v
}

/// The `<imageN>` tags in a prompt: (byte range, N). Accepts `<image2>`,
/// `<Image 2>` and the like.
pub fn image_tags(prompt: &str) -> Vec<(std::ops::Range<usize>, usize)> {
    let lower = prompt.to_ascii_lowercase();
    let mut out = vec![];
    let mut from = 0;
    while let Some(at) = lower[from..].find("<image").map(|i| i + from) {
        let rest = &lower[at + 6..];
        let digits_at = rest.len() - rest.trim_start_matches(' ').len();
        let digits: String = rest[digits_at..].chars().take_while(char::is_ascii_digit).collect();
        let end = at + 6 + digits_at + digits.len();
        if !digits.is_empty() && lower[end..].starts_with('>') {
            if let Ok(n) = digits.parse() {
                out.push((at..end + 1, n));
            }
            from = end + 1;
        } else {
            from = at + 6;
        }
    }
    out
}

/// The prompt with its tags renumbered by `map` (old N → new N, or `None`
/// for an image that's no longer a reference: "[removed image]").
pub fn retag(prompt: &str, map: &std::collections::BTreeMap<usize, Option<usize>>) -> String {
    let mut s = String::new();
    let mut last = 0;
    for (range, n) in image_tags(prompt) {
        s.push_str(&prompt[last..range.start]);
        match map.get(&n) {
            Some(Some(m)) => s.push_str(&format!("<image{m}>")),
            Some(None) => s.push_str("[removed image]"),
            None => s.push_str(&prompt[range.clone()]),
        }
        last = range.end;
    }
    s.push_str(&prompt[last..]);
    s
}

/// A prompt edit waiting for confirmation: the image list changed the
/// `<imageN>` numbers the prompt uses.
#[derive(Clone, Debug, PartialEq)]
pub struct Retag {
    /// (old N, new N or `None` when that image is no longer a reference),
    /// for the tags the prompt uses that change.
    pub changes: Vec<(usize, Option<usize>)>,
    pub prompt: String,
}

/// The name for the project "New project from this image" (`N`) or
/// "Duplicate project with this image" (`D`) makes.
#[derive(Clone, Debug, PartialEq)]
pub struct NewProjectDialog {
    pub image: ImageRef,
    pub target: UseTarget,
    pub name: String,
}

/// Home's Start a new project: its name, and the references to add once
/// it is made.
#[derive(Clone, Debug, PartialEq)]
pub struct StartProjectDialog {
    pub name: String,
    pub refs: StartRefs,
}

/// Where a new project's first references come from: the picker that
/// opens on its Generate form once it is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartRefs {
    /// No picker; the form starts empty.
    None,
    /// From library…, over the input and output libraries.
    Library,
    /// Image files from the disk.
    Files,
}

/// Naming the project New project from ideas makes (always asked: an idea's
/// title needn't suit a project).
#[derive(Clone, Debug, PartialEq)]
pub struct FromIdeasDialog {
    /// The ideas; empty: the New idea form's draft (Add & new project).
    pub ideas: Vec<Id>,
    /// Where the project goes, relative to `projects/` (step 2 picks it).
    pub parent: PathBuf,
    pub name: String,
    /// A new folder to make in `parent` (on Create) and put the project in;
    /// an existing one of that name (any case) is used. It starts as the
    /// project's name and follows it while they match (`None`: in `parent`
    /// itself).
    pub new_folder: Option<String>,
    /// Step 2's folder filter.
    pub filter: String,
    /// The draft from Save to input library's Save & new project: the
    /// library folder it's saved in (else the top level).
    pub saved_in: Option<PathBuf>,
}

impl FromIdeasDialog {
    pub fn new(ideas: Vec<Id>, parent: PathBuf, name: String) -> FromIdeasDialog {
        let new_folder = Some(name.trim().to_string());
        FromIdeasDialog { ideas, parent, name, new_folder, filter: String::new(), saved_in: None }
    }

    /// The project goes in a new folder named after it.
    pub fn own_folder(&self) -> bool {
        self.new_folder.as_deref().map(str::trim) == Some(self.name.trim())
    }
}

/// Save to input library, from the Idea inbox or the New idea form: the
/// idea's name (one idea or the draft; several keep their titles) and the
/// library folder it goes to.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveIdeaDialog {
    /// The ideas; empty: the New idea form's draft (Add & save).
    pub ideas: Vec<Id>,
    /// Starts as the idea's title.
    pub name: String,
    /// Relative to `input/library/` (empty: the top level).
    pub folder: PathBuf,
    /// A new folder to make in `folder` and save into (an existing one of
    /// that name, any case, is used).
    pub new_folder: Option<String>,
    /// The folder tree's filter.
    pub filter: String,
}

impl SaveIdeaDialog {
    /// Starting in the folder the last save went to, if it's still there.
    pub fn new(state: &AppState, ideas: Vec<Id>, name: String) -> SaveIdeaDialog {
        let folder = state
            .save_idea_last
            .clone()
            .filter(|f| f.as_os_str().is_empty() || state.index.library_folders.contains(f))
            .unwrap_or_default();
        SaveIdeaDialog { ideas, name, folder, new_folder: None, filter: String::new() }
    }
}

/// Naming the copy a project's Duplicate makes.
#[derive(Clone, Debug, PartialEq)]
pub struct DuplicateDialog {
    /// The project, relative to `projects/`.
    pub path: PathBuf,
    pub name: String,
}

/// The Generate form's **Presets** window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresetsPanel {
    pub view: PresetsView,
    pub search: String,
}

/// Prompt enhance: the server's state and what it suggested.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeState {
    /// The chat server in use: the open preset's, or the one picked to
    /// continue without sd-servers.
    pub server: Option<increart_core::pe::PeServer>,
    /// More chat servers sharing the asks with `server` (the preset's
    /// [`chat_pool`](increart_core::config::ServerPreset::chat_pool)).
    pub more: Vec<increart_core::pe::PeServer>,
    pub status: PeStatus,
    /// Of `server` and `more`: how many are ready (when more than one).
    pub ready: usize,
    /// Blanks asked to reroll: their answer replaces the values offered,
    /// rather than adding to them.
    pub rerolls: BTreeSet<BlankKey>,
    /// Blanks asked avoiding the values offered before: their answer drops
    /// any the server offers again.
    pub avoiding: BTreeSet<BlankKey>,
    /// The **Suggest…** dialog, for one blank.
    pub suggest: Option<SuggestDialog>,
    /// The **Suggest for blanks…** dialog, for the shared blanks ticked.
    pub batch_suggest: Option<BatchSuggest>,
    /// The **Prompt enhance settings** window.
    pub editor: Option<PeEditor>,
    /// The **Enhance style prompts…** dialog: its settings and the styles
    /// ticked.
    pub style_dialog: Option<increart_core::prompts::EnhanceAsk>,
    /// Styles waiting for an answer: how asked, what was sent to improve,
    /// and how many times asked again after an empty or unchanged answer.
    pub style_tries: BTreeMap<increart_core::prompts::PresetKey, StyleTry>,
    /// Styles whose asks failed even when asked again, and why (until asked
    /// again).
    pub style_failed: BTreeMap<increart_core::prompts::PresetKey, String>,
    /// The prompt built for each style asked, when asked: what its answer
    /// was made from.
    pub style_from: BTreeMap<increart_core::prompts::PresetKey, String>,
    /// Styles asked for since none were waiting (for "4 of 6").
    pub style_batch: usize,
    /// Blanks prompt enhance added to styles' prompts this batch (names
    /// without braces), asked about when the batch ends.
    pub added_blanks: Vec<(increart_core::prompts::PresetKey, Vec<String>)>,
    /// The batch ended with blanks added: keep them, or make them words?
    pub added_blanks_ask: bool,
    /// The GPUs listed for it, once asked, and the runtime that listed them.
    pub gpus: Option<Result<Vec<(u32, String)>, String>>,
    pub gpus_runtime: increart_core::pe::GpuRuntime,
    /// The asks not answered yet, in the order they're worked on (the
    /// first is the one being worked on, unless it's still queued).
    pub jobs: Vec<PeJobState>,
    /// The rewrite / describe window.
    pub text: Option<PeText>,
    /// Counts asks: each has its own number.
    pub asks: u64,
    /// Blanks asked for since none were waiting (for "5 of 13").
    pub blank_batch: usize,
    /// Blanks waiting for an answer: the settings asked with, and how many
    /// times asked again after an answer with nothing new.
    pub blank_tries: BTreeMap<BlankKey, BlankTry>,
    /// Blanks that got nothing new even when asked again, and why (until
    /// asked again).
    pub blank_failed: BTreeMap<BlankKey, String>,
    /// When the server started starting (`now`).
    pub starting_since: f64,
    /// How long the model took to load, last time.
    pub last_load: Option<f64>,
    /// How long the server took to read an ask before writing, last time,
    /// by the number of images it had.
    pub read_times: BTreeMap<usize, f64>,
}

impl PeState {
    /// The ask for a blank, if it's waiting.
    pub fn blank_job(&self, key: &BlankKey) -> Option<&PeJobState> {
        self.jobs.iter().find(|j| matches!(&j.key, PeKey::Blank(k) if k == key))
    }

    pub fn job(&self, id: u64) -> Option<&PeJobState> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// How many asks are ahead of this one.
    pub fn ahead(&self, id: u64) -> usize {
        self.jobs.iter().take_while(|j| j.id != id).filter(|j| matches!(j.step, PeStep::Queued)).count()
    }

    /// How many PE servers share the asks.
    pub fn pool_size(&self) -> usize {
        self.server.iter().count() + self.more.len()
    }

    /// The ask to show as "the one being worked on": the first answered,
    /// else the first.
    pub fn working(&self) -> Option<&PeJobState> {
        self.jobs.iter().find(|j| !matches!(j.step, PeStep::Queued)).or(self.jobs.first())
    }

    /// How many times a blank waiting has been asked again.
    pub fn blank_retry(&self, key: &BlankKey) -> u8 {
        self.blank_tries.get(key).map_or(0, |t| t.retry)
    }

    pub fn blanks_waiting(&self) -> usize {
        self.jobs.iter().filter(|j| matches!(j.key, PeKey::Blank(_))).count()
    }

    /// The ask for a style's prompt, if it's waiting.
    pub fn style_job(&self, key: &increart_core::prompts::PresetKey) -> Option<&PeJobState> {
        self.jobs.iter().find(|j| matches!(&j.key, PeKey::Style(k) if k == key))
    }

    pub fn styles_waiting(&self) -> usize {
        self.jobs.iter().filter(|j| matches!(j.key, PeKey::Style(_))).count()
    }

    /// The asks waiting, a batch per kind (Jobs): suggesting values for
    /// blanks, enhancing styles' prompts, the rewrite window's ask; in the
    /// order their first ask is worked on.
    pub fn batches(&self) -> Vec<PeBatch> {
        let mut out: Vec<PeBatch> = vec![];
        for j in &self.jobs {
            let kind = match &j.key {
                PeKey::Blank(_) => PeBatchKind::Blanks,
                PeKey::Style(_) => PeBatchKind::Styles,
                PeKey::Text(_) => match &self.text {
                    Some(t) => PeBatchKind::Text(t.target.clone()),
                    None => continue,
                },
            };
            match out.iter_mut().find(|b| b.kind == kind) {
                Some(b) => b.ids.push(j.id),
                None => out.push(PeBatch { kind, ids: vec![j.id], total: 0 }),
            }
        }
        for b in &mut out {
            let asked = match b.kind {
                PeBatchKind::Blanks => self.blank_batch,
                PeBatchKind::Styles => self.style_batch,
                PeBatchKind::Text(_) => 0,
            };
            b.total = asked.max(b.ids.len());
        }
        out
    }
}

/// What a batch of prompt-enhance asks is for.
#[derive(Clone, Debug, PartialEq)]
pub enum PeBatchKind {
    /// Values for the Generate form's blanks (its project's).
    Blanks,
    /// The Generate form's sweep styles' prompts.
    Styles,
    /// The rewrite / describe window's ask.
    Text(PeTarget),
}

/// Asks of one kind waiting: `ids` in the order they're worked on, `total`
/// asked since none of the kind were waiting ("4 of 41": the first is
/// number `total - ids.len() + 1`).
#[derive(Clone, Debug, PartialEq)]
pub struct PeBatch {
    pub kind: PeBatchKind,
    pub ids: Vec<u64>,
    pub total: usize,
}

impl PeBatch {
    /// The number of the ask being worked on, from 1.
    pub fn at(&self) -> usize {
        self.total - self.ids.len() + 1
    }
}

/// An ask waiting for its answer.
#[derive(Clone, Debug, PartialEq)]
pub struct PeJobState {
    pub id: u64,
    pub key: PeKey,
    pub step: PeStep,
    /// When it was asked (`now`).
    pub asked: f64,
    /// When its step began.
    pub since: f64,
    /// Tokens a second while writing, and when worked out: kept for half a
    /// second so the figure can be read.
    pub rate: Option<(f64, f64)>,
    /// The chat server answering it (once taken).
    pub on: Option<String>,
}

/// Where an ask is.
#[derive(Clone, Debug, PartialEq)]
pub enum PeStep {
    /// Waiting behind others (or for the server to start).
    Queued,
    /// Scaling its images.
    Preparing,
    /// Sent: the server reads the prompt and images before writing.
    Reading { images: usize },
    /// The answer so far (`pieces`: about one per token).
    Writing { pieces: u32, text: String },
}

/// What a PE answer is for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PeKey {
    Blank(BlankKey),
    /// A sweep style's prompt, improved.
    Style(increart_core::prompts::PresetKey),
    /// The rewrite / describe window's ask, by number.
    Text(u64),
}

/// The text a rewrite is of, or the image described.
#[derive(Clone, Debug, PartialEq)]
pub enum PeTarget {
    /// The Generate prompt.
    Prompt,
    /// The New idea form's text.
    NewIdea,
    /// An idea's text (Edit idea).
    Idea(Id),
    Describe(ImageRef),
}

/// The rewrite / describe window: the original, the answer to look over
/// (and edit), and instructions for another try.
#[derive(Clone, Debug, PartialEq)]
pub struct PeText {
    pub target: PeTarget,
    /// The text rewritten (empty when describing).
    pub original: String,
    /// The user's instructions ("more cinematic, keep it short").
    pub how: String,
    /// The answer, as edited; empty until it comes.
    pub text: String,
    /// The answer as it came, to tell an edit from it.
    pub answer: String,
    /// The ask waiting for its answer (its number).
    pub waiting: Option<u64>,
    pub error: Option<String>,
    /// How many images went with the ask.
    pub images: usize,
}

impl PeText {
    /// The server and system prompt the answer came from, for a variant.
    pub fn origin(
        server: &increart_core::pe::PeServer,
        pe: &increart_core::pe::PeConfig,
        describe: bool,
    ) -> increart_core::model::VariantOrigin {
        use increart_core::pe::{DESCRIBE, REWRITE, prompt_hash};
        let job = if describe { DESCRIBE } else { REWRITE };
        increart_core::model::VariantOrigin::Rewritten {
            model: server.name.clone(),
            system_prompt_hash: prompt_hash(pe.system_prompt(job)),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum PeStatus {
    /// Not started (or not set up).
    #[default]
    Off,
    /// Starting the server (loading the model).
    Starting,
    Ready,
    Failed(String),
}

impl AppState {
    /// The values prompt enhance offered for a blank of the Generate sweep,
    /// to pick from (saved with the project's draft).
    pub fn blank_shown(&self, key: &BlankKey) -> Option<&Vec<String>> {
        let v = self.generate.sweep.blank_values(key.style.as_ref(), &key.slot)?;
        Some(&v.shown).filter(|s| !s.is_empty())
    }

    /// **Suggest for blanks…**'s **Tick only** sets among the shared blanks
    /// `names`: (fewer than `count` suggestions showing, no value, failed).
    pub fn batch_picks(&self, names: &[String], count: usize) -> [Vec<String>; 3] {
        let key = |n: &String| BlankKey { style: None, slot: n.clone() };
        let pick = |f: &dyn Fn(&String) -> bool| names.iter().filter(|n| f(n)).cloned().collect::<Vec<_>>();
        [
            pick(&|n| self.blank_shown(&key(n)).map_or(0, |v| v.len()) < count),
            pick(&|n| self.generate.sweep.slots.get(n).is_none_or(|v| v.trim().is_empty())),
            pick(&|n| self.pe.blank_failed.contains_key(&key(n))),
        ]
    }

    /// Every value offered for a blank (rerolled ones too): asks avoid them.
    pub fn blank_offered(&self, key: &BlankKey) -> &[String] {
        self.generate.sweep.blank_values(key.style.as_ref(), &key.slot).map_or(&[], |v| v.offered.as_slice())
    }
}

/// A sweep blank: its shared value (`style` None), or a style's own.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlankKey {
    pub style: Option<increart_core::prompts::PresetKey>,
    pub slot: String,
}

/// **Prompt enhance settings**: the server dialog's draft's chat server,
/// being edited or made.
#[derive(Clone, Debug, PartialEq)]
pub struct PeEditor {
    /// Its place in the draft's servers; None: a new one.
    pub index: Option<usize>,
    pub draft: increart_core::pe::PeServer,
    /// Why it can't be saved as it is.
    pub error: Option<String>,
}

/// A style's ask: its settings, the prompt sent to improve, and how many
/// times asked again.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleTry {
    pub ask: increart_core::prompts::EnhanceAsk,
    pub sent: String,
    pub retry: u8,
}

/// The **Suggest…** dialog: asking for a blank's values with instructions
/// and the asks' settings at hand.
#[derive(Clone, Debug, PartialEq)]
pub struct SuggestDialog {
    pub key: BlankKey,
    pub form: SuggestForm,
}

/// The **Suggest for blanks…** dialog: one ask for each shared blank
/// ticked, each with the same settings.
#[derive(Clone, Debug, PartialEq)]
pub struct BatchSuggest {
    /// Ticked, by slot.
    pub blanks: Vec<String>,
    pub form: SuggestForm,
}

/// A blank's ask: with a Suggest dialog's settings (None: the quick ask's),
/// and how many times asked again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlankTry {
    pub form: Option<SuggestForm>,
    pub retry: u8,
}

/// A Suggest dialog's settings, as the asks get them.
#[derive(Clone, Debug, PartialEq)]
pub struct SuggestForm {
    /// Instructions sent with the ask ("only stones").
    pub how: String,
    pub count: usize,
    pub sampling: increart_core::pe::Sampling,
    /// Attach the reference and start images (with a vision server).
    pub images: bool,
    /// Name the values offered before, not to offer them again.
    pub avoid: bool,
    /// The answer replaces the values offered, rather than adding to them.
    pub replace: bool,
}

impl SuggestForm {
    /// The settings remembered, the sampling `server`'s unless they set
    /// their own.
    pub fn from_ask(a: &SuggestAsk, server: increart_core::pe::Sampling) -> SuggestForm {
        SuggestForm {
            how: a.how.clone(),
            count: a.count,
            sampling: a.sampling.unwrap_or(server),
            images: a.images,
            avoid: a.avoid,
            replace: a.replace,
        }
    }

    /// To remember: the sampling only if not `server`'s, so it follows
    /// Settings until changed here.
    pub fn to_ask(&self, server: increart_core::pe::Sampling) -> SuggestAsk {
        SuggestAsk {
            how: self.how.clone(),
            count: self.count,
            sampling: Some(self.sampling).filter(|s| *s != server),
            images: self.images,
            avoid: self.avoid,
            replace: self.replace,
        }
    }
}

/// The sweep's **Choose styles** window. Its ticks are the sweep's own
/// (`generate.sweep.selected`); Cancel puts back those it opened with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StylePicker {
    pub view: StyleView,
    pub search: String,
    pub filter: StyleFilter,
    /// The sweep as it was on opening.
    pub before: Sweep,
    /// What every style has made so far, as of opening.
    pub stats: HashMap<increart_core::prompts::PresetKey, StyleStats>,
}

/// What the Choose styles window lists (when not searching).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum StyleView {
    #[default]
    Ticked,
    Favourites,
    All,
    Task(String),
    /// A group within a task.
    Group(String, String),
}

/// Choose styles' filters, each narrowing the list.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StyleFilter {
    pub favourites: bool,
    /// Needs no more reference images than Generate has.
    pub usable: bool,
    /// Never used in the selected project.
    pub untried_here: bool,
    /// Has a Good image in some project.
    pub has_good: bool,
}

impl StyleFilter {
    pub fn count(&self) -> usize {
        [self.favourites, self.usable, self.untried_here, self.has_good].iter().filter(|&&x| x).count()
    }
}

/// A style's images (in every project), by verdict.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleStats {
    pub good: usize,
    pub neutral: usize,
    pub bad: usize,
    pub unrated: usize,
    /// Its newest run.
    pub last: Option<Id>,
    /// Its newest Good image, else its newest image.
    pub sample: Option<ImageRef>,
    /// Runs of it in the selected project.
    pub here: usize,
}

impl StyleStats {
    pub fn images(&self) -> usize {
        self.good + self.neutral + self.bad + self.unrated
    }
}

/// Whether the search `q` (lower case) finds a preset.
pub fn preset_matches(p: &PromptPreset, q: &str) -> bool {
    [&p.name, &p.description, &p.prompt, &p.group].iter().any(|s| s.to_lowercase().contains(q))
}

/// How many reference images a prompt needs: its highest `<imageN>`.
pub fn needs_refs(prompt: &str) -> usize {
    image_tags(prompt).into_iter().map(|(_, n)| n).max().unwrap_or(0)
}

/// What the Presets window lists (when not searching).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PresetsView {
    #[default]
    Favourites,
    Task(usize),
    /// One group (heading) of a task.
    Group(usize, String),
}

impl PresetsView {
    pub fn task(&self) -> Option<usize> {
        match self {
            PresetsView::Task(t) | PresetsView::Group(t, _) => Some(*t),
            _ => None,
        }
    }

    pub fn group(&self) -> Option<&str> {
        match self {
            PresetsView::Group(_, g) => Some(g),
            _ => None,
        }
    }
}

/// **Edit presets…**: a working copy, saved or dropped as a whole.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetsEditor {
    pub list: PromptPresets,
    pub task: usize,
    pub preset: Option<usize>,
    pub error: Option<String>,
}

/// **Save as preset…**: the Generate prompt as a new preset.
#[derive(Clone, Debug, PartialEq)]
pub struct SavePresetDialog {
    pub task: usize,
    pub preset: PromptPreset,
    pub error: Option<String>,
}

/// Switching to another library from the main window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LibrarySwitch {
    /// The engine was stopped for it (restarted on Cancel).
    pub stopped: bool,
}

/// Where an output image is used (Review / Output library actions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseTarget {
    /// The selected project's Generate form.
    CurrentProject,
    /// `E`: a fresh form in the image's own project.
    Edit,
    /// `N`: a new project next to the image's project.
    NewProject,
    /// `D`: a copy of the image's project, starting from the image's run.
    DuplicateProject,
}

/// Max reference images Qwen-Image 2.1 is meant to take (more are sent,
/// with a warning).
pub const MAX_REFS: usize = 9;

impl GenerateForm {
    pub fn new(config: &Config) -> GenerateForm {
        let (width, height) = config.default_resolution().map_or((1024, 1024), |r| (r.width, r.height));
        let mut f = GenerateForm {
            label: String::new(),
            prompt: String::new(),
            negative: String::new(),
            width,
            height,
            preset: 0,
            steps: 20,
            cfg: 2.0,
            flow_shift: None,
            seeds: 4,
            first_seed: 0,
            images: vec![],
            strength: increart_core::model::default_strength(),
            source_idea: None,
            sweep: Sweep::default(),
        };
        if let Some(p) = config.recipes.first() {
            f.apply_preset(0, p);
        }
        f
    }

    pub fn to_draft(&self, config: &Config) -> GenerateDraft {
        GenerateDraft {
            label: self.label.clone(),
            prompt: self.prompt.clone(),
            negative_prompt: self.negative.clone(),
            width: self.width,
            height: self.height,
            preset: config.recipes.get(self.preset).map(|r| r.name.clone()).unwrap_or_default(),
            steps: self.steps,
            cfg: self.cfg,
            flow_shift: self.flow_shift,
            seeds: self.seeds,
            first_seed: self.first_seed,
            // transparency is asked for in the prompt; never wrapped
            alpha: false,
            refs: self.refs(),
            init: self.init(),
            images: self.images.clone(),
            strength: self.strength,
            source_idea: self.source_idea.clone(),
            sweep: self.sweep.clone(),
        }
    }

    /// The form from a saved draft (the model preset by name, else the first).
    pub fn from_draft(d: &GenerateDraft, config: &Config) -> GenerateForm {
        GenerateForm {
            label: d.label.clone(),
            prompt: d.prompt.clone(),
            negative: d.negative_prompt.clone(),
            width: d.width,
            height: d.height,
            preset: config.recipes.iter().position(|r| r.name == d.preset).unwrap_or(0),
            steps: d.steps,
            cfg: d.cfg,
            flow_shift: d.flow_shift,
            seeds: d.seeds,
            first_seed: d.first_seed,
            images: if d.images.is_empty() { images_of(&d.refs, d.init.as_ref()) } else { d.images.clone() },
            strength: d.strength,
            source_idea: d.source_idea.clone(),
            // ticks from before Modify style was split, in the style tasks
            sweep: d.sweep.current(&increart_core::prompts::builtin()),
        }
    }

    /// The form a run was made with (for Duplicate with an image): prompt,
    /// sizes, sampling, transparency and refs; label and seeds start afresh.
    pub fn from_run(run: &increart_core::model::Run, config: &Config) -> GenerateForm {
        let mut f = GenerateForm::new(config);
        f.prompt = run.prompt.clone();
        f.negative = run.negative_prompt.clone();
        (f.width, f.height) = (run.width, run.height);
        f.steps = run.sample.steps;
        f.cfg = run.sample.cfg;
        f.flow_shift = run.sample.flow_shift;
        let init = match &run.mode {
            increart_core::model::Mode::Edit { init, strength } => {
                f.strength = *strength;
                init.clone()
            }
            increart_core::model::Mode::Generate => None,
        };
        f.images = images_of(&run.refs, init.as_ref());
        f.source_idea = run.source_idea.clone();
        f.preset = config.recipes.iter().position(|r| r.recipe == run.model.recipe).unwrap_or(0);
        f
    }

    /// The references, in order (`<image1>` first).
    pub fn refs(&self) -> Vec<ImageRef> {
        self.images.iter().filter(|i| i.reference).map(|i| i.image.clone()).collect()
    }

    /// The img2img start image, if any.
    pub fn init(&self) -> Option<ImageRef> {
        self.images.iter().find(|i| i.start).map(|i| i.image.clone())
    }

    /// The `<imageN>` number of the list's `i`th image, if it's a reference.
    pub fn tag_of(&self, i: usize) -> Option<usize> {
        self.images.get(i).filter(|x| x.reference).map(|_| self.images[..i].iter().filter(|x| x.reference).count() + 1)
    }

    /// Adds an image as the next reference (or makes it one, if it's in the
    /// list already).
    pub fn add_reference(&mut self, image: ImageRef) {
        match self.images.iter_mut().find(|i| i.image == image) {
            Some(i) => i.reference = true,
            None => self.images.push(GenerateImage { image, reference: true, start: false }),
        }
    }

    /// Makes an image the start image (at most one); returns the one it
    /// replaced.
    pub fn set_start(&mut self, image: ImageRef) -> Option<ImageRef> {
        let old = self.init().filter(|o| *o != image);
        for i in &mut self.images {
            i.start = i.image == image;
        }
        if !self.images.iter().any(|i| i.start) {
            self.images.push(GenerateImage { image, reference: false, start: true });
        }
        old
    }

    pub fn apply_preset(&mut self, i: usize, p: &RecipePreset) {
        self.preset = i;
        self.steps = p.steps;
        self.cfg = p.cfg;
        self.flow_shift = p.flow_shift;
    }
}

/// A library archive being written (Export library) or unpacked (Open a
/// library archive): the archive, and bytes done of how many.
#[derive(Clone, Debug, PartialEq)]
pub struct Archiving {
    pub archive: PathBuf,
    pub export: bool,
    pub done: u64,
    pub total: u64,
}

/// What a file picker is for; its result comes back as [`crate::Msg::Picked`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    /// A library folder (existing, or empty for a new library).
    LibraryFolder,
    /// Images and text files to import as new ideas.
    ImportFiles,
    /// Images to attach to the New idea draft.
    IdeaImages,
    /// Images to add as references in the Generate form.
    GenerateRefs,
    /// The Export dialog's destination folder.
    ExportDir,
    /// Where Tile… saves its image, in this format.
    TileFile(increart_core::config::TileFormat),
    SdServerBinary,
    KoboldBinary,
    /// A model directory (its index; one past the last adds one).
    ModelDir(usize),
    /// A file for the PE server being edited.
    Pe(PeFile),
    /// A file for the preset editor's KoboldCpp entry (its index).
    Kobold(usize, PeFile),
    /// The KoboldCpp wizard's binary.
    WizardBinary,
    /// The KoboldCpp wizard's model directory.
    WizardModels,
    /// Where Export library saves its archive, in this format.
    LibraryArchiveOut(increart_core::archive::ArchiveFormat),
    /// A library archive to unpack and open.
    LibraryArchiveIn,
    /// The folder to unpack [`AppState::unpack_from`] into.
    LibraryUnpackInto,
    /// A file of the model presets editor's selected preset: its slot (an
    /// index into [`increart_core::config::MODEL_SLOTS`]).
    RecipeFile(usize),
    /// A LoRA of the model presets editor's selected preset (its index).
    RecipeLora(usize),
    /// Where **Save for a bug report…** saves its zip.
    BugReport,
}

/// Which of a KoboldCpp PE server's files is picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeFile {
    Binary,
    Model,
    Mmproj,
    /// A `.kcpps` settings file.
    Config,
}

/// A server's **Test** (DESIGN.md, Testing a server): its steps so far.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerTest {
    /// Numbers tests; steps from an older one are ignored.
    pub run: u32,
    pub name: String,
    pub steps: Vec<increart_core::servertest::Step>,
    pub done: bool,
}

/// **Validate servers…** (DESIGN.md, Validating servers): the suite,
/// `increart-apitest` run as a child, over the config's server presets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApiTest {
    /// Numbers lists and runs; events from an older one are ignored.
    pub run: u32,
    /// Asking the suite for its cases.
    pub listing: bool,
    pub running: bool,
    /// Stop asked, the run not over yet.
    pub stopping: bool,
    pub cases: Vec<ApiCase>,
    /// The server preset shown (its cases only); None: all of them.
    pub preset: Option<String>,
    /// The case shown on the right; None: the run's own log.
    pub selected: Option<usize>,
    /// The case shown follows the one running (until one is clicked).
    pub follow: bool,
    /// The run's log: between cases, and its end.
    pub log: Vec<String>,
    /// The last run's counts, report folder and zip.
    pub done: Option<(increart_core::apitest::Counts, Option<PathBuf>, Option<PathBuf>)>,
    /// It couldn't list or run (the suite missing, a bad config).
    pub error: Option<String>,
    /// Run the basic cases of the preset shown once they're listed
    /// (**Validate now**).
    pub basic_pending: bool,
    /// The cases the last run ran, for its summary.
    pub ran: Vec<usize>,
    /// Stop was asked during the last run (it didn't finish).
    pub stopped: bool,
    /// When the last run started and ended (`AppState::now`).
    pub started: f64,
    pub ended: Option<f64>,
}

/// A case of the suite, in [`ApiTest`].
#[derive(Clone, Debug, PartialEq)]
pub struct ApiCase {
    pub case: increart_core::apitest::Case,
    pub checked: bool,
    /// Of the last run that ran it.
    pub outcome: Option<increart_core::apitest::Outcome>,
    pub running: bool,
    pub summary: String,
    pub secs: f64,
    pub log: Vec<String>,
    pub traffic: Vec<String>,
}

/// Which cases **Run** runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiRun {
    Checked,
    All,
    /// Those that failed or were skipped last time.
    Failed,
}

impl ApiTest {
    pub fn index(&self, id: &str) -> Option<usize> {
        self.cases.iter().position(|c| c.case.id == id)
    }

    /// Whether case `i` is shown: of the preset shown, if one is.
    pub fn shown(&self, i: usize) -> bool {
        self.cases.get(i).is_some_and(|c| self.preset.as_ref().is_none_or(|p| &c.case.preset == p))
    }

    /// The cases shown.
    pub fn shown_cases(&self) -> Vec<usize> {
        (0..self.cases.len()).filter(|&i| self.shown(i)).collect()
    }

    /// The server presets, in the order listed.
    pub fn presets(&self) -> Vec<&str> {
        let mut v: Vec<&str> = vec![];
        for c in &self.cases {
            if !v.contains(&c.case.preset.as_str()) {
                v.push(&c.case.preset);
            }
        }
        v
    }

    /// Shows `preset`'s cases only, checking those for its setup (not the
    /// n/a ones) and no other's; None shows all, the checks as they are.
    pub fn show_preset(&mut self, preset: Option<String>) {
        let preset = preset.filter(|p| self.cases.iter().any(|c| &c.case.preset == p));
        if let Some(p) = &preset {
            for c in &mut self.cases {
                c.checked = &c.case.preset == p && c.case.not_applicable.is_none();
            }
        }
        self.preset = preset;
        if self.selected.is_some_and(|i| !self.shown(i)) {
            self.selected = None;
        }
    }

    /// Checks the basic cases shown (those for its setup) and no others.
    pub fn check_basic(&mut self) {
        for i in 0..self.cases.len() {
            let on = self.shown(i) && self.cases[i].case.basic && self.cases[i].case.not_applicable.is_none();
            self.cases[i].checked = on;
        }
    }

    /// The cases `which` runs, of the cases shown.
    pub fn picked(&self, which: ApiRun) -> Vec<usize> {
        use increart_core::apitest::Outcome;
        let pick = |c: &ApiCase| match which {
            ApiRun::Checked => c.checked,
            ApiRun::All => true,
            ApiRun::Failed => matches!(c.outcome, Some(Outcome::Fail | Outcome::Skipped)),
        };
        self.shown_cases().into_iter().filter(|&i| pick(&self.cases[i])).collect()
    }

    /// The IDs `which` runs (empty for all), of the cases shown.
    pub fn ids(&self, which: ApiRun) -> Vec<String> {
        if which == ApiRun::All && self.preset.is_none() {
            return vec![];
        }
        self.picked(which).into_iter().map(|i| self.cases[i].case.id.clone()).collect()
    }

    /// The basic cases of the preset shown, for its setup.
    pub fn basic_cases(&self) -> Vec<usize> {
        self.shown_cases()
            .into_iter()
            .filter(|&i| self.cases[i].case.basic && self.cases[i].case.not_applicable.is_none())
            .collect()
    }

    /// The last run is over and ran (some of) the preset shown.
    pub fn finished(&self) -> bool {
        !self.running && self.ended.is_some() && self.ran.iter().any(|&i| self.shown(i))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServerStatus {
    /// What it was opened as.
    pub config: Option<ServerConfig>,
    pub name: String,
    pub device: String,
    pub state: String,
    pub model: Option<String>,
    /// Since when it's been ready with no job (`AppState::now`).
    pub idle_since: Option<f64>,
    /// Has had a job since it was last idle (its queue clearing is news).
    pub worked: bool,
}

/// The notification alerts' state (DESIGN.md, Notifications).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Alerts {
    /// When each alert last sounded; `None` while its condition doesn't hold.
    pub idle_played: Option<f64>,
    pub backlog_played: Option<f64>,
    /// What holds now (for the bell), e.g. "GPU 0 idle for 7 min".
    pub idle_text: Option<String>,
    pub backlog_text: Option<String>,
    /// Servers whose queue just cleared, for the next check.
    pub cleared: Vec<usize>,
    /// The runs each server has had jobs of since it was last idle.
    pub server_runs: HashMap<usize, BTreeSet<Id>>,
    /// Each server's work since it was last idle: since when
    /// (`AppState::now`), and images made (for "… in 41 min").
    pub busy: HashMap<usize, (f64, u32)>,
    /// Runs stopped with Stop now: a server that clears from one of them
    /// was stopped by the user, who saw it, so there's no alert.
    pub stopped_now: BTreeSet<Id>,
    /// The last queue clearing, e.g. "GPU 0 finished its queue", until the
    /// window has the focus or a GPU gets work again.
    pub cleared_text: Option<String>,
    /// No sound until then.
    pub snooze_until: Option<f64>,
    /// No sound until the app restarts.
    pub muted: bool,
    /// The app's window has the focus.
    pub focused: bool,
    /// For the window: ask the window manager for attention (taken there).
    pub attention: bool,
    /// The last sound that couldn't play.
    pub sound_error: Option<String>,
    /// The Test button's last result.
    pub test_result: Option<Result<String, String>>,
    /// When the conditions were last checked (once a second).
    pub checked: f64,
    /// When prompt enhance was last seen asking.
    pub pe_busy_at: Option<f64>,
}

/// The longest snooze for a time typed in (longer: Until restart).
pub const SNOOZE_MAX_MIN: u32 = 24 * 60;

/// Minutes, with or without a unit ("45", "45m", "45 min").
fn minutes_of(t: &str) -> Option<f64> {
    let m = t.trim_end_matches(|c: char| c.is_alphabetic()).trim();
    let unit = t[m.len()..].trim();
    let ok = ["", "m", "min", "mins", "minute", "minutes"].contains(&unit);
    m.parse::<f64>().ok().filter(|v| ok && v.is_finite() && *v >= 0.0)
}

/// A snooze typed in, in minutes: `45` (minutes), `45m`, `45 min`, `2h`,
/// `1.5h`, `1h30`, `1h 30m`, `1:30` (hours:minutes); 1 min to 24 h.
pub fn parse_snooze(text: &str) -> Result<u32, String> {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return Err("Type a time, e.g. 45, 45m, 1h30 or 1:30".into());
    }
    let bad = || format!("\"{}\" isn't a time: try 45, 45m, 1h30 or 1:30", text.trim());
    let num = |s: &str| -> Result<f64, String> {
        let s = s.trim();
        s.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).ok_or_else(bad)
    };
    let minutes = if let Some((h, m)) = t.split_once(':') {
        let m = num(m)?;
        if m >= 60.0 {
            return Err("Minutes after the colon go up to 59".into());
        }
        num(h)? * 60.0 + m
    } else if let Some(at) = t.find('h') {
        // hours, then minutes if any ("1h30", "1h 30m", "2 hours")
        let (h, rest) = t.split_at(at);
        let after = rest.trim_start_matches(|c: char| c.is_alphabetic());
        if !["h", "hr", "hrs", "hour", "hours"].contains(&&rest[..rest.len() - after.len()]) {
            return Err(bad());
        }
        let after = after.trim();
        num(h)? * 60.0 + if after.is_empty() { 0.0 } else { minutes_of(after).ok_or_else(bad)? }
    } else {
        minutes_of(&t).ok_or_else(bad)?
    };
    let minutes = minutes.round();
    if minutes < 1.0 {
        return Err("At least 1 minute".into());
    }
    if minutes > SNOOZE_MAX_MIN as f64 {
        return Err("At most 24 hours (longer: Until restart)".into());
    }
    Ok(minutes as u32)
}

impl Alerts {
    pub fn snoozed(&self, now: f64) -> bool {
        self.muted || self.snooze_until.is_some_and(|t| now < t)
    }

    /// No sound now: snoozed, or focused when it should sound only while not.
    pub fn held(&self, now: f64, only_unfocused: bool) -> bool {
        self.snoozed(now) || (only_unfocused && self.focused)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub text: String,
    pub error: bool,
    /// A notification alert's reason: shown with Snooze and Settings links.
    pub alert: bool,
    /// The log file (in the log folder) of the server it is about, when the
    /// app started that server: shown with a button opening it.
    pub log: Option<String>,
}

/// An inline rename in a tree: which node, and the edited name.
#[derive(Clone, Debug, PartialEq)]
pub struct Renaming {
    pub tree: Tree,
    pub path: PathBuf,
    pub name: String,
    /// New project: nothing made yet; `path` is where it would go, and
    /// Cancel leaves no project behind.
    pub creating: bool,
}

/// Where the user is, for the top-left back / forward: the tabs and what
/// is selected in their side panels.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub tab: Tab,
    pub inbox_tab: InboxTab,
    pub lib_tab: LibTab,
    pub lib_folder: Option<PathBuf>,
    pub projects_tab: ProjectsTab,
    pub project: Option<Id>,
    pub proj_folder: Option<PathBuf>,
    pub review_scope: Option<Id>,
    pub review_folder: Option<PathBuf>,
    pub review_view: ReviewView,
    pub output_tab: OutputTab,
    pub output_source: OutputSource,
}

/// The places visited, around the current one.
#[derive(Clone, Debug, Default)]
pub struct NavHistory {
    pub back: Vec<Place>,
    pub forward: Vec<Place>,
    pub current: Option<Place>,
    /// Nesting of `update` (only a whole message makes a step).
    pub(crate) depth: u32,
    /// Going back or forward: its own steps aren't recorded.
    pub(crate) restoring: bool,
}

/// Seconds the finished usage check stays in the status bar.
pub const USAGE_CHECKED_SHOWN: f64 = 5.0;

/// Where Review's **Generate ›** goes ([`AppState::review_generate`]).
pub struct ReviewGenerate {
    pub project: Id,
    pub name: String,
    pub image: Option<ImageRef>,
    pub run: Option<Id>,
}

/// Review's Follow, as it goes.
#[derive(Clone, Debug, Default)]
pub struct Follow {
    /// The image being shown, and seconds it has been on (not counting
    /// paused, held or away).
    pub shown: Option<ImageRef>,
    pub elapsed: f64,
    /// When `elapsed` was last brought up to date.
    pub at: f64,
    /// Paused from its button; held while the pointer is over the image
    /// (with `pause_on_hover`).
    pub paused: bool,
    pub held: bool,
    /// When images came, over the last [`Follow::WINDOW`] seconds.
    pub arrivals: Vec<f64>,
    /// The newest image seen, for telling the new ones.
    pub newest: Option<Id>,
    /// The last move on (when, and from which image): a rating just after
    /// it is for the image that was there.
    pub moved: Option<(f64, ImageRef)>,
}

impl Follow {
    pub const WINDOW: f64 = 600.0;
    /// A rating this soon after moving on rates the image before.
    pub const GRACE: f64 = 0.4;

    /// Seconds between images lately, if two or more came.
    pub fn interval(&self, now: f64) -> Option<f64> {
        let (first, n) = (self.arrivals.first()?, self.arrivals.len());
        (n >= 2).then(|| (now - first).max(0.0) / (n - 1) as f64)
    }
}

pub struct FollowCountdown {
    /// Seconds to the next image, of `secs` an image.
    pub left: f64,
    pub secs: f64,
    /// Images after the one shown.
    pub ahead: usize,
    pub paused: bool,
    /// Held by the pointer, or not on Review's queue.
    pub held: bool,
}

/// The Settings window's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    Notifications,
    /// This library's own settings (its image storage).
    Library,
    /// How long the servers may go quiet (`Config::timeouts`).
    Timeouts,
}

/// The open library's store format and what its stores hold now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreInfo {
    pub format: increart_core::model::StoreFormat,
    pub counts: increart_core::library::StoreCounts,
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub config: Config,
    /// The GPU runtime new KoboldCpp entries and the wizard start with
    /// (CUDA when an NVIDIA driver is found, else Vulkan).
    pub gpu_runtime: increart_core::pe::GpuRuntime,
    /// Where `config` is saved (`None`: not saved, e.g. in tests).
    pub config_path: Option<PathBuf>,
    pub phase: Phase,
    pub library: Option<PathBuf>,
    pub index: Index,
    pub tab: Tab,
    pub inbox_tab: InboxTab,
    pub projects_tab: ProjectsTab,
    pub draft: IdeaDraft,
    /// Selected card in the Idea inbox grid (index into `inbox_queue`).
    pub inbox_cursor: usize,
    /// The Idea inbox Grid's picks (Ctrl/Shift+click, the rubber band), in
    /// pick order: what the picks bar and their menu act on.
    pub inbox_picked: Vec<Id>,
    pub inbox_filter: InboxFilter,
    /// Details panels hidden this session (all are shown at start).
    pub info_hidden: BTreeSet<InfoPanel>,
    /// Input library folder shown (`None`: all ideas).
    pub lib_folder: Option<PathBuf>,
    pub lib_selected: BTreeSet<Id>,
    /// The idea in the Input library's side panel.
    pub lib_info: Option<Id>,
    pub project: Option<Id>,
    /// Projects tree selection when a folder is selected (relative to `projects/`).
    pub proj_folder: Option<PathBuf>,
    /// The grid's folder kept when a project was selected without narrowing
    /// it (see [`Self::project_scope`]).
    pub proj_scope_kept: Option<PathBuf>,
    pub generate: GenerateForm,
    pub servers: Vec<ServerStatus>,
    /// What each server can load, by name (asked once it's open).
    pub server_models: BTreeMap<String, increart_core::sdserver::Loadable>,
    /// The shared sd-server settings `servers` were opened with.
    pub servers_opened_with: Option<increart_core::config::SdServerConfig>,
    pub engine_running: bool,
    /// The Test window of a server the app connects to.
    pub server_test: Option<ServerTest>,
    /// Tests started, numbering them.
    pub server_tests: u32,
    /// The **Validate servers…** window.
    pub api_test: Option<ApiTest>,
    /// It shows its advanced view (the cases as a tree, each one's log),
    /// not the basic one (a preset's validation, step by step, and its
    /// summary); kept for the session.
    pub api_test_advanced: bool,
    /// Which server presets passed the suite's basic cases (the stamps
    /// file at `validations_path`).
    pub validations: increart_core::validation::Validations,
    /// The stamps file ([`increart_core::validation::path`]); None (as in
    /// tests): nothing read or noted, and opening a preset doesn't ask.
    pub validations_path: Option<PathBuf>,
    /// `increart-apitest` is there to run (in `tools/` beside the app, beside it, or on PATH).
    pub suite_found: bool,
    /// Numbers server-opening attempts; reports from older ones are ignored.
    pub open_attempt: u32,
    /// Servers still opening after **Continue in background**.
    pub background: Option<Vec<ServerProgress>>,
    /// Per starting server: its log's latest line ("loading tensors …").
    pub start_log: Vec<String>,
    /// Servers being stopped (until [`Msg::ServersStopped`](crate::Msg)).
    pub stopping: Vec<String>,
    /// Runs with seeds in the engine.
    pub active_runs: BTreeSet<Id>,
    /// Active runs stopped with Finish & stop: their generating images are
    /// still coming.
    pub finishing: BTreeSet<Id>,
    /// (run, seed) → (step, total) of seeds being generated.
    pub progress: HashMap<(Id, u64), (u32, u32)>,
    /// When their steps were seen, for the live seconds per step.
    pub sampling_times: HashMap<(Id, u64), crate::timing::Sampling>,
    /// Each server's latest images this session (by its name), for its
    /// speed in the server list.
    pub server_speeds: HashMap<String, std::collections::VecDeque<crate::timing::Recent>>,
    /// Seconds per image from the library's timed images (rebuilt with
    /// the index), for the forecast.
    pub rates: crate::forecast::Rates,
    /// When each run in the engine starts and finishes (DESIGN.md,
    /// Timing), remade every second.
    pub forecast: crate::forecast::Forecast,
    /// How long each server took to load each model (the file at
    /// `load_times_path`; None, as in tests: not saved).
    pub load_times: increart_core::loadtimes::LoadTimes,
    pub load_times_path: Option<PathBuf>,
    /// Review scope: `None` all projects (or `review_folder`), else one project.
    pub review_scope: Option<Id>,
    /// Review scope: every project under this folder (relative to `projects/`).
    pub review_folder: Option<PathBuf>,
    /// Review scope: one sweep of `review_scope`'s project (by name).
    pub review_sweep: Option<String>,
    /// Review scope: one run of `review_scope`'s project (not a sweep's).
    pub review_run: Option<Id>,
    /// Review's scope panel lists every project's runs, not only the
    /// selected project's.
    pub review_all_runs: bool,
    /// Review's scope panel: the tree's search text (project and folder names).
    pub review_search: String,
    /// The contact sheet's selected cell (row, column).
    pub sheet_cell: (usize, usize),
    /// How many rows the contact sheet shows side by side (what fits the
    /// window; set as it is drawn): row r is on line r / bands, in band
    /// r % bands.
    pub sheet_bands: usize,
    /// The contact sheet's name filter: rows whose style (or run label)
    /// has every word of it.
    pub sheet_filter: String,
    /// A new sweep asked for, waiting for its project's prompt history (the
    /// form it was queued with).
    pub new_sweep: Option<NewSweepAsk>,
    pub review_cursor: usize,
    /// Review moves on through the unreviewed images by itself, paced by
    /// [`FollowPace`](increart_core::config::FollowPace), up to the newest
    /// (Single, Grid, and Compare's B); going back turns it off.
    pub review_follow: bool,
    pub follow: Follow,
    pub review_view: ReviewView,
    /// A/B compare: the pinned image (A).
    pub review_pinned: Option<ImageRef>,
    /// A/B compare (Review's and the Output library's) with the pinned A
    /// on the left and B, the image the arrows change, on the right (Swap
    /// sides); B is on the left by default.
    pub compare_flipped: bool,
    pub output_tab: OutputTab,
    pub output_source: OutputSource,
    /// Set by Show outputs: the Grid warns that filters hide images, or that
    /// there are none, until the source changes or it is dismissed.
    pub output_notice: bool,
    pub confirm: Option<Confirm>,
    /// The server status window (the top bar's **Servers…**).
    pub server_status: bool,
    /// "No server is connected" showing, after Queue.
    pub no_server_warning: bool,
    /// The log viewer (**Logs…**), when open.
    pub logs: Option<LogsView>,
    /// A bug report zip being written.
    pub saving_bug_report: bool,
    /// That warning was shown this session (it shows once per start).
    pub no_server_warned: bool,
    /// The runs and sweeps being saved (project, label): Queue again for
    /// one before it's saved (a double-click) would save it twice, as the
    /// label only moves on once it is.
    pub queueing: Vec<(Id, String)>,
    /// Queueing a sweep while the jobs go one at a time: asking whether to
    /// spread them evenly (holds the sweep's "big one confirmed").
    pub sweep_order_ask: Option<bool>,
    /// Answered: the queueing it held goes on without asking again.
    pub sweep_order_asked: bool,
    /// Projects → Folder.
    /// The folder view of the tab showing; the other three wait in
    /// `parked_folder_views`, so each keeps its own location and history.
    pub folder_view: FolderView,
    pub parked_folder_views: Vec<FolderView>,
    pub fv_clipboard: Option<FvClipboard>,
    pub lib_tab: LibTab,
    pub lib_filter: LibFilter,
    pub output_filter: OutputFilter,
    /// The Output library's search box searches project names (narrowing
    /// the "by project" tree) instead of the images' titles and prompts.
    pub output_search_projects: bool,
    /// That project-name search.
    pub output_project_search: String,
    /// Set by Show outputs: the sources panel opens the way to the source
    /// in "by project" and scrolls to it, once (then `OutputRevealed`).
    pub output_reveal: bool,
    /// Bumped by each Show outputs: the "by project" tree's open folders
    /// start over (only the way to the source open).
    pub output_reveals: u32,
    /// The Grid's preset tab last clicked (the filters may differ since).
    pub output_preset: OutputPreset,
    /// The top-left back / forward.
    pub nav: NavHistory,
    /// Projects tab: the tree's search text.
    pub project_search: String,
    /// The command line's servers choice, until the library opens.
    pub startup_servers: Option<StartupServers>,
    /// Output library image whose lineage and recipe are shown.
    pub output_selected: Option<ImageRef>,
    /// The Output library's Grid comparing against this (A), the selected
    /// image being B.
    pub output_compare: Option<ImageRef>,
    pub export: Option<ExportDialog>,
    /// Images picked in Review or the Output library's Grid, in the order
    /// picked: rated together, or tiled (DESIGN.md, Tiling).
    pub picked: Vec<ImageRef>,
    pub tile: Option<TileDialog>,
    pub markup: Option<Markup>,
    pub mark_tools: MarkTools,
    /// A composed image for the GUI to put on the clipboard.
    pub clipboard_image: Option<crate::tile::TileImage>,
    pub history_view: Option<HistoryView>,
    pub history_last: HashMap<HistoryKey, LastEntry>,
    pub idea_edit: Option<IdeaEdit>,
    /// When the Edit idea text was last changed, while not yet recorded.
    pub idea_edit_dirty: Option<f64>,
    /// The resolution editor's working copy.
    pub res_editor: Option<Vec<Resolution>>,
    /// The model presets editor, while open.
    pub recipe_editor: Option<RecipeEditor>,
    pub scale: Option<ScaleDialog>,
    /// An image shown large over everything (double-click a reference or
    /// the start image in Generate).
    pub preview: Option<ImageRef>,
    /// The title dialog of an output image, while open.
    pub title_edit: Option<TitleEdit>,
    /// Generate's From library… picker, while open.
    pub ref_picker: Option<RefPicker>,
    /// Where the picker was last (it opens there again).
    pub ref_picker_source: Option<RefSource>,
    /// The job details window (a run in Jobs or the jobs popover clicked).
    pub job_info: Option<Id>,
    /// Its generation log (read when it opens, added to as the engine logs).
    pub job_log: Vec<String>,
    /// The runs each server (by index) has a job of now.
    pub server_jobs: HashMap<usize, Vec<Id>>,
    /// The Settings window.
    pub settings_open: bool,
    pub settings_tab: SettingsTab,
    /// For Settings ▸ Library, asked for when it shows.
    pub store: Option<StoreInfo>,
    /// A store conversion going on: images done, of how many.
    pub converting: Option<(usize, usize)>,
    /// A library archive being written or unpacked.
    pub archiving: Option<Archiving>,
    /// The archive picked to unpack, while its destination is picked.
    pub unpack_from: Option<PathBuf>,
    pub alerts: Alerts,
    /// How the library is watched for outside changes (Import tab).
    pub watch_status: String,
    /// Naming the project New project / Duplicate with an image makes.
    pub new_project: Option<NewProjectDialog>,
    /// Home's Start a new project dialog.
    pub start_project: Option<StartProjectDialog>,
    /// Naming a project's plain Duplicate.
    pub duplicate: Option<DuplicateDialog>,
    /// Naming the project for New project from ideas.
    pub from_ideas: Option<FromIdeasDialog>,
    /// Where the last New project from ideas put its project (this session), the next
    /// one's first guess.
    pub from_ideas_last: Option<PathBuf>,
    /// Saving inbox ideas to the input library.
    pub save_idea: Option<SaveIdeaDialog>,
    /// Where the last Save to input library went (this session), the next
    /// one's first folder.
    pub save_idea_last: Option<PathBuf>,
    /// The prompt presets (shared by every library).
    pub prompt_presets: PromptPresets,
    /// The `prompt-presets` folder they are saved in (`None`: not saved,
    /// e.g. in tests).
    pub presets_path: Option<PathBuf>,
    pub presets_panel: Option<PresetsPanel>,
    pub styles: Option<StylePicker>,
    pub pe: PeState,
    pub presets_editor: Option<PresetsEditor>,
    pub save_preset: Option<SavePresetDialog>,
    /// The Generate prompt's characters to select, once (a preset's first
    /// `{slot}`, or where an inserted preset ends).
    pub prompt_select: Option<std::ops::Range<usize>>,
    /// The library screen shown to switch libraries (it has a Cancel).
    pub switching: Option<LibrarySwitch>,
    /// The prompt's `<imageN>` tags to renumber, waiting for confirmation.
    pub retag: Option<Retag>,
    /// Recipes of output images, loaded on demand.
    pub recipes: HashMap<Id, Option<Recipe>>,
    /// Input images' stored metadata, loaded on demand (`None`: loading).
    pub input_meta: HashMap<Id, Option<InputMeta>>,
    /// How often each image was used as a ref or start image (from the
    /// index's runs; see [`AppState::rebuild_usage`]).
    pub usage: HashMap<ImageRef, Usage>,
    /// Import tab: input images with what their original was, newest first.
    pub imports: Option<Vec<(Id, SourceInfo)>>,
    /// Review: checkerboard behind transparent images (else a flat colour).
    pub checkerboard: bool,
    /// (link path now, verdict before) of review actions, for `Ctrl+Z`:
    /// one entry per action (rating the picked rates them all at once).
    pub review_undo: Vec<Vec<(PathBuf, Option<Verdict>)>>,
    /// Output links sent to be rated, until the index shows them moved (or
    /// no library op is in flight): they leave Review's queue at once, so a
    /// quick next rating goes to the next image, not to this one by the
    /// path it no longer has.
    pub reviewing: HashSet<PathBuf>,
    pub renaming: Option<Renaming>,
    /// Library operations in flight.
    pub busy: usize,
    /// The usage check after opening, while it runs: images done, of how many.
    pub usage_check: Option<(usize, usize)>,
    /// Once it's done: how many images it checked, and when (`now`), shown
    /// for [`USAGE_CHECKED_SHOWN`] seconds.
    pub usage_checked: Option<(usize, f64)>,
    /// Seconds, from `Msg::Tick` (the GUI's clock).
    pub now: f64,
    /// When the Generate form was last edited, while not yet autosaved.
    pub generate_dirty: Option<f64>,
    /// When the New idea draft was last edited, while not yet autosaved.
    pub idea_dirty: Option<f64>,
    /// When a draft was last saved (for the "draft saved" note).
    pub draft_saved: Option<f64>,
    /// Draft saves sent and not yet confirmed.
    pub saving: usize,
    pub messages: Vec<Message>,
}

impl AppState {
    /// What the open servers can load (those that have said).
    fn open_loadable(&self) -> impl Iterator<Item = &increart_core::sdserver::Loadable> {
        self.servers.iter().filter_map(|s| self.server_models.get(&s.name))
    }

    /// The open servers that can run these files.
    pub fn servers_running(&self, files: &BTreeMap<String, String>) -> Vec<&str> {
        self.servers
            .iter()
            .filter(|s| self.server_models.get(&s.name).is_some_and(|l| l.runs(files)))
            .map(|s| s.name.as_str())
            .collect()
    }

    /// Why no open server can run these files, one line per server
    /// ("kcpp-gpu1: …"); `None` when one can, or none has said.
    pub fn why_no_server_runs(&self, files: &BTreeMap<String, String>) -> Option<Vec<String>> {
        let known: Vec<_> =
            self.servers.iter().filter_map(|s| self.server_models.get(&s.name).map(|l| (&s.name, l))).collect();
        if known.is_empty() {
            return None;
        }
        let why: Vec<String> =
            known.iter().map(|(name, l)| l.why_not(files).map(|w| format!("{name}: {w}"))).collect::<Option<_>>()?;
        Some(why)
    }

    /// Every model file the open servers list (KoboldCpp started elsewhere
    /// lists none).
    pub fn server_files(&self) -> BTreeSet<&str> {
        use increart_core::sdserver::Loadable;
        self.open_loadable()
            .flat_map(|l| match l {
                Loadable::Files { ids, .. } => ids.iter().map(String::as_str).collect(),
                Loadable::Only(_) => vec![],
            })
            .collect()
    }

    /// Why no open server lists a model file, for its row in Model presets:
    /// each server that has it but can't load it says why
    /// ("kcpp-gpu1: is in sd-server's int8 format, …"), else whether it's
    /// in the model directories at all.
    pub fn why_file_unlisted(&self, id: &str) -> String {
        use increart_core::sdserver::Loadable;
        let refused: Vec<String> = self
            .servers
            .iter()
            .filter_map(|s| match self.server_models.get(&s.name)? {
                Loadable::Files { refused, .. } => refused.get(id).map(|why| format!("{}: {why}", s.name)),
                Loadable::Only(_) => None,
            })
            .collect();
        if !refused.is_empty() {
            return refused.join("\n");
        }
        if increart_core::config::model_file_on_disk(id, &self.config.programs.sd_server.model_dirs).is_none() {
            "Not in the model directories, so no open server lists it".into()
        } else {
            "No open server lists it (a server you connect to lists its own model directories)".into()
        }
    }

    /// For a model preset no open server can run, one they can with the
    /// same sd-server recipe (sd-server's int8 files → KoboldCpp's GGUFs):
    /// the same model, so a draft's settings still fit it. None when it
    /// runs, or no server has said.
    pub fn runnable_alternative(&self, preset: usize) -> Option<usize> {
        let runnable = self.runnable_recipes();
        if self.open_loadable().next().is_none() || runnable.contains(&preset) {
            return None;
        }
        let recipe = &self.config.recipes.get(preset)?.recipe;
        runnable.into_iter().find(|&i| self.config.recipes[i].recipe == *recipe)
    }

    /// The model presets (indices into `config.recipes`) an open server
    /// can run; all of them until one has said what it can load.
    pub fn runnable_recipes(&self) -> Vec<usize> {
        let all = 0..self.config.recipes.len();
        if self.open_loadable().next().is_none() {
            return all.collect();
        }
        all.filter(|&i| self.open_loadable().any(|l| l.runs(&self.config.recipes[i].files))).collect()
    }

    /// Image models a KoboldCpp started elsewhere runs that no model preset
    /// uses: (server, file name).
    pub fn models_without_preset(&self) -> Vec<(&str, &str)> {
        use increart_core::sdserver::Loadable;
        self.servers
            .iter()
            .filter_map(|s| match self.server_models.get(&s.name) {
                Some(l @ Loadable::Only(Some(file))) if !self.config.recipes.iter().any(|r| l.runs(&r.files)) => {
                    Some((s.name.as_str(), file.as_str()))
                }
                _ => None,
            })
            .collect()
    }

    /// Fresh state: asks for a library (a recent one, or another folder).
    pub fn new(config: Config, config_path: Option<PathBuf>) -> AppState {
        AppState {
            gpu_runtime: Default::default(),
            generate: GenerateForm::new(&config),
            config,
            config_path,
            phase: Phase::Library(LibraryDialog::default()),
            library: None,
            index: Index::default(),
            tab: Tab::Home,
            inbox_tab: InboxTab::NewIdea,
            projects_tab: ProjectsTab::Grid,
            draft: IdeaDraft::default(),
            inbox_cursor: 0,
            inbox_filter: InboxFilter::default(),
            info_hidden: BTreeSet::new(),
            lib_folder: None,
            lib_selected: BTreeSet::new(),
            inbox_picked: vec![],
            lib_info: None,
            project: None,
            proj_folder: None,
            proj_scope_kept: None,
            servers: vec![],
            server_models: BTreeMap::new(),
            servers_opened_with: None,
            engine_running: false,
            server_test: None,
            server_tests: 0,
            api_test: None,
            api_test_advanced: false,
            validations: Default::default(),
            validations_path: None,
            suite_found: false,
            open_attempt: 0,
            background: None,
            start_log: vec![],
            stopping: vec![],
            active_runs: BTreeSet::new(),
            finishing: BTreeSet::new(),
            progress: HashMap::new(),
            sampling_times: HashMap::new(),
            server_speeds: HashMap::new(),
            rates: Default::default(),
            forecast: Default::default(),
            load_times: Default::default(),
            load_times_path: None,
            review_scope: None,
            review_folder: None,
            review_sweep: None,
            review_run: None,
            review_all_runs: false,
            review_search: String::new(),
            sheet_cell: (0, 0),
            sheet_bands: 1,
            sheet_filter: String::new(),
            new_sweep: None,
            review_cursor: 0,
            review_follow: false,
            follow: Follow::default(),
            review_view: ReviewView::Single,
            review_pinned: None,
            compare_flipped: false,
            output_tab: OutputTab::Grid,
            output_source: OutputSource::AllProjects,
            output_notice: false,
            confirm: None,
            server_status: false,
            no_server_warning: false,
            logs: None,
            saving_bug_report: false,
            no_server_warned: false,
            queueing: vec![],
            folder_view: FolderView::new(FvPlace::Projects),
            parked_folder_views: FvPlace::ALL[1..].iter().map(|p| FolderView::new(*p)).collect(),
            fv_clipboard: None,
            lib_tab: LibTab::Grid,
            lib_filter: LibFilter::default(),
            output_filter: OutputFilter::default(),
            output_search_projects: false,
            output_project_search: String::new(),
            output_reveal: false,
            output_reveals: 0,
            output_preset: OutputPreset::Kept,
            nav: NavHistory::default(),
            project_search: String::new(),
            startup_servers: None,
            output_selected: None,
            output_compare: None,
            export: None,
            picked: vec![],
            tile: None,
            markup: None,
            mark_tools: MarkTools::default(),
            clipboard_image: None,
            history_view: None,
            history_last: HashMap::new(),
            idea_edit: None,
            idea_edit_dirty: None,
            res_editor: None,
            recipe_editor: None,
            scale: None,
            preview: None,
            ref_picker: None,
            ref_picker_source: None,
            title_edit: None,
            from_ideas: None,
            from_ideas_last: None,
            save_idea: None,
            save_idea_last: None,
            job_info: None,
            job_log: vec![],
            server_jobs: HashMap::new(),
            settings_open: false,
            settings_tab: SettingsTab::Notifications,
            store: None,
            converting: None,
            archiving: None,
            unpack_from: None,
            alerts: Alerts { focused: true, ..Default::default() },
            retag: None,
            new_project: None,
            start_project: None,
            sweep_order_ask: None,
            sweep_order_asked: false,
            duplicate: None,
            prompt_presets: increart_core::prompts::builtin(),
            presets_path: None,
            presets_panel: None,
            styles: None,
            pe: PeState::default(),
            presets_editor: None,
            save_preset: None,
            prompt_select: None,
            switching: None,
            watch_status: String::new(),
            recipes: HashMap::new(),
            input_meta: HashMap::new(),
            usage: HashMap::new(),
            imports: None,
            checkerboard: true,
            review_undo: vec![],
            reviewing: HashSet::new(),
            renaming: None,
            busy: 0,
            usage_check: None,
            usage_checked: None,
            now: 0.0,
            generate_dirty: None,
            idea_dirty: None,
            draft_saved: None,
            saving: 0,
            messages: vec![],
        }
    }

    // ---------------------------------------------------------- idea lists

    /// The Idea inbox, newest first.
    pub fn unreviewed_ideas(&self) -> Vec<&IdeaEntry> {
        let mut v: Vec<_> = self.index.ideas.iter().filter(|e| e.loc == IdeaLoc::Unreviewed).collect();
        v.sort_by(|a, b| b.idea.id.cmp(&a.idea.id));
        v
    }

    /// The Idea inbox as the Grid shows it: searched, filtered and sorted.
    pub fn inbox_queue(&self) -> Vec<&IdeaEntry> {
        let f = &self.inbox_filter;
        let now = increart_core::id::now();
        let mut v = self.unreviewed_ideas();
        v.retain(|e| f.matches(&e.idea, now));
        if f.oldest_first {
            v.reverse();
        }
        v
    }

    /// Discarded ideas, newest first.
    pub fn discarded_ideas(&self) -> Vec<&IdeaEntry> {
        let mut v: Vec<_> = self.index.ideas.iter().filter(|e| e.loc == IdeaLoc::Discarded).collect();
        v.sort_by(|a, b| b.idea.id.cmp(&a.idea.id));
        v
    }

    /// Kept ideas in the selected folder and its subfolders, newest first.
    pub fn library_ideas(&self) -> Vec<&IdeaEntry> {
        let mut v: Vec<_> = self
            .index
            .ideas
            .iter()
            .filter(|e| match (&e.loc, &self.lib_folder) {
                (IdeaLoc::Library(_), None) => true,
                (IdeaLoc::Library(f), Some(sel)) => f.starts_with(sel),
                (IdeaLoc::Edited, None) => self.lib_filter.show_hidden,
                _ => false,
            })
            .collect();
        let f = &self.lib_filter;
        let q = f.text.to_lowercase();
        v.retain(|e| {
            let i = &e.idea;
            (q.is_empty()
                || i.title.to_lowercase().contains(&q)
                || i.text.to_lowercase().contains(&q)
                || i.tags.iter().any(|t| t.to_lowercase().contains(&q)))
                && (!f.starred_only || i.starred)
                && (!f.with_images_only || !i.images.is_empty())
                && f.used_in_project.is_none_or(|p| p == !i.projects.is_empty())
        });
        v.sort_by(|a, b| b.idea.id.cmp(&a.idea.id));
        if f.most_used {
            v.sort_by_key(|e| std::cmp::Reverse(self.idea_usage(e).runs()));
        }
        v
    }

    /// Counts every image's uses from the runs in the index.
    pub fn rebuild_usage(&mut self) {
        let mut made: HashMap<&Id, usize> = HashMap::new();
        for o in self.index.projects.iter().flat_map(|p| &p.outputs) {
            *made.entry(&o.name.run).or_default() += 1;
        }
        let mut usage: HashMap<ImageRef, Usage> = HashMap::new();
        // as the images' increart:usage counts: a run counts once it generates
        for r in self.index.projects.iter().flat_map(|p| &p.runs).filter(|r| !r.revisions.is_empty()) {
            let images = made.get(&r.id).copied().unwrap_or(0);
            for i in r.refs.iter().collect::<std::collections::HashSet<_>>() {
                usage.entry(i.clone()).or_default().add(Usage { ref_runs: 1, start_runs: 0, images });
            }
            if let increart_core::model::Mode::Edit { init: Some(i), .. } = &r.mode {
                usage.entry(i.clone()).or_default().add(Usage { ref_runs: 0, start_runs: 1, images });
            }
        }
        self.usage = usage;
    }

    pub fn usage(&self, image: &ImageRef) -> Usage {
        self.usage.get(image).copied().unwrap_or_default()
    }

    /// An idea's images' uses, added up.
    pub fn idea_usage(&self, e: &IdeaEntry) -> Usage {
        let mut u = Usage::default();
        for id in &e.idea.images {
            u.add(self.usage(&ImageRef::input(id.clone())));
        }
        u
    }

    pub fn idea(&self, id: &Id) -> Option<&IdeaEntry> {
        self.index.idea(id)
    }

    // ---------------------------------------------------------- projects

    /// Whether `name` is taken next to the project at `path` (relative to
    /// `projects/`), by a project or a folder, ignoring case.
    pub fn project_name_taken(&self, path: &Path, name: &str) -> bool {
        let parent = path.parent().unwrap_or(Path::new(""));
        let projects = self.index.projects.iter().filter_map(|p| p.dir.strip_prefix("projects").ok());
        projects.chain(self.index.project_folders.iter().map(PathBuf::as_path)).any(|p| {
            p.parent() == Some(parent)
                && p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == name.to_lowercase())
        })
    }

    /// What keeps Start servers from starting: no sd-server binary, or a
    /// missing one or model directory. A bare binary name is looked up on PATH
    /// and not checked.
    pub fn sdserver_problem(&self) -> Option<String> {
        let c = &self.config.programs.sd_server;
        if c.binary.is_empty() {
            return Some("No sd-server binary set".into());
        }
        if c.binary.contains('/') && !Path::new(&c.binary).is_file() {
            return Some(format!("sd-server not found at {}", c.binary));
        }
        c.model_dirs
            .iter()
            .find(|m| !m.trim().is_empty() && !Path::new(m).is_dir())
            .map(|m| format!("Model directory not found: {m}"))
    }

    pub fn place(&self) -> Place {
        Place {
            tab: self.tab,
            inbox_tab: self.inbox_tab,
            lib_tab: self.lib_tab,
            lib_folder: self.lib_folder.clone(),
            projects_tab: self.projects_tab,
            project: self.project.clone(),
            proj_folder: self.proj_folder.clone(),
            review_scope: self.review_scope.clone(),
            review_folder: self.review_folder.clone(),
            review_view: self.review_view,
            output_tab: self.output_tab,
            output_source: self.output_source.clone(),
        }
    }

    /// Whether a folder view is on screen (it has its own back / forward).
    pub fn folder_view_shown(&self) -> bool {
        match self.tab {
            Tab::IdeaInbox => self.inbox_tab == InboxTab::Folder,
            Tab::InputLibrary => self.lib_tab == LibTab::Folder,
            Tab::Projects => self.projects_tab == ProjectsTab::Folder,
            Tab::Home | Tab::Review => false,
            Tab::OutputLibrary => self.output_tab == OutputTab::Folder,
        }
    }

    pub fn project_entry(&self) -> Option<&ProjectEntry> {
        self.index.project(self.project.as_ref()?)
    }

    /// The project folder the grid shows (relative to `projects/`): the
    /// selected folder; else, with a project selected, the folder kept on
    /// selecting it, or [`Self::project_home`].
    pub fn project_scope(&self) -> PathBuf {
        match (&self.proj_folder, self.project_entry()) {
            (Some(f), _) => f.clone(),
            (None, Some(p)) => self.proj_scope_kept.clone().unwrap_or_else(|| self.project_home(p)),
            (None, None) => PathBuf::new(),
        }
    }

    /// A project's folder when it holds more than this one project (here
    /// and below), else the folder above it.
    pub fn project_home(&self, p: &ProjectEntry) -> PathBuf {
        let mut at = p.dir.parent().unwrap_or(Path::new("projects"));
        if !self.has_siblings(p) {
            at = at.parent().filter(|a| a.starts_with("projects")).unwrap_or(at);
        }
        at.strip_prefix("projects").map(Path::to_path_buf).unwrap_or_default()
    }

    /// Whether a project's folder (here and below) holds other projects.
    pub fn has_siblings(&self, p: &ProjectEntry) -> bool {
        let Some(dir) = p.dir.parent() else { return false };
        self.index.projects.iter().any(|q| q.dir != p.dir && q.dir.starts_with(dir))
    }

    /// Projects in [`Self::project_scope`] and all its subfolders, in
    /// [`Self::sort_projects`] order.
    pub fn projects_in_scope(&self) -> Vec<&ProjectEntry> {
        let scope = Path::new("projects").join(self.project_scope());
        let mut v: Vec<_> = self.index.projects.iter().filter(|p| p.dir.starts_with(&scope)).collect();
        self.sort_projects(&mut v);
        v
    }

    /// The subfolders of a project-tree folder (relative to `projects/`,
    /// empty = the top): by name (any case), or newest created / last run
    /// first, by the newest project anywhere in them (empty folders last,
    /// by name).
    pub fn project_folder_children(&self, parent: &Path) -> Vec<&PathBuf> {
        let mut v: Vec<&PathBuf> = self.index.project_folders.iter().filter(|f| f.parent() == Some(parent)).collect();
        self.sort_folders(&mut v, self.config.folder_order.0);
        v
    }

    /// Folders (relative to `projects/`) by name (any case), or newest
    /// created / last run first by the newest project anywhere in them
    /// (empty folders last, by name).
    fn sort_folders(&self, v: &mut [&PathBuf], order: TreeOrder) {
        v.sort_by_cached_key(|f| f.to_string_lossy().to_lowercase());
        let last_run = match order {
            TreeOrder::Name => return,
            o => o == TreeOrder::LastRun,
        };
        v.sort_by_cached_key(|f| {
            let dir = Path::new("projects").join(f);
            let newest = self
                .index
                .projects
                .iter()
                .filter(|p| p.dir.starts_with(&dir))
                .map(|p| (last_run.then(|| p.last_run()), p.project.id.clone()))
                .max();
            std::cmp::Reverse(newest)
        });
    }

    /// Unreviewed images in a project folder (relative to `projects/`) and
    /// all its subfolders.
    pub fn unreviewed_under(&self, folder: &Path) -> usize {
        let dir = Path::new("projects").join(folder);
        self.index.projects.iter().filter(|p| p.dir.starts_with(&dir)).map(|p| counts(p)[3]).sum()
    }

    /// Review's scope tree at a folder (relative to `projects/`): its
    /// subfolders and its projects in [`ReviewOrder`] order, and, with **hide
    /// reviewed**, without those that have none (unless selected or
    /// holding the selection).
    pub fn review_tree_children(&self, parent: &Path) -> (Vec<&PathBuf>, Vec<&ProjectEntry>) {
        let hide = !self.config.review_show_reviewed;
        let selected_dir = match (&self.review_folder, &self.review_scope) {
            (Some(f), _) => Some(Path::new("projects").join(f)),
            (None, Some(id)) => self.index.project(id).map(|p| p.dir.clone()),
            _ => None,
        };
        let holds_selection = |dir: &Path| selected_dir.as_ref().is_some_and(|s| s.starts_with(dir));
        let mut folders: Vec<&PathBuf> = self
            .index
            .project_folders
            .iter()
            .filter(|f| f.parent() == Some(parent))
            .filter(|f| !hide || self.unreviewed_under(f) > 0 || holds_selection(&Path::new("projects").join(f)))
            .collect();
        let dir = Path::new("projects").join(parent);
        let mut projects: Vec<&ProjectEntry> = self
            .index
            .projects
            .iter()
            .filter(|p| p.dir.parent() == Some(dir.as_path()))
            .filter(|p| !hide || counts(p)[3] > 0 || holds_selection(&p.dir))
            .collect();
        if let Some(order) = self.config.review_order.tree() {
            self.sort_folders(&mut folders, order);
            sort_projects(&mut projects, order);
            return (folders, projects);
        }
        projects.sort_by(|a, b| {
            let key = |p: &ProjectEntry| (counts(p)[3] == 0, p.project.id.clone());
            key(a).cmp(&key(b))
        });
        folders.sort_by_cached_key(|f| {
            let dir = Path::new("projects").join(f);
            let oldest = self
                .index
                .projects
                .iter()
                .filter(|p| p.dir.starts_with(&dir) && counts(p)[3] > 0)
                .map(|p| p.project.id.clone())
                .min();
            (oldest.is_none(), oldest, f.to_string_lossy().to_lowercase())
        });
        (folders, projects)
    }

    /// [`sort_projects`] by the project tree's order.
    pub fn sort_projects(&self, v: &mut [&ProjectEntry]) {
        sort_projects(v, self.config.project_order)
    }

    /// One past the highest seed any run of the selected project has used
    /// (generated, pending, cancelled or failed): where the Generate panel's
    /// first seed continues from.
    pub fn next_seed(&self) -> u64 {
        self.project_entry().map_or(0, |p| p.runs.iter().map(|r| r.next_seed()).max().unwrap_or(0))
    }

    /// Whether a project-tree folder is collapsed (in this library).
    pub fn folder_collapsed(&self, f: &Path) -> bool {
        let Some(lib) = &self.library else { return false };
        self.config.collapsed_folders.get(&*lib.to_string_lossy()).is_some_and(|c| c.iter().any(|g| g == f))
    }

    // ---------------------------------------------------------- review

    /// What the reference picker shows (its source, text and ratings),
    /// each image once.
    pub fn ref_candidates(&self) -> Vec<RefCandidate> {
        let Some(p) = &self.ref_picker else { return vec![] };
        let q = p.text.trim().to_lowercase();
        let has = |s: &str| s.to_lowercase().contains(&q);
        let mut seen = std::collections::HashSet::new();
        let v: Vec<RefCandidate> = match &p.source {
            RefSource::TileInputs | RefSource::ImageInputs(_) | RefSource::ProjectInputs(_) => {
                let images: Vec<ImageRef> = match &p.source {
                    RefSource::ImageInputs(i) => self.image_inputs(i),
                    RefSource::ProjectInputs(id) => self.project_inputs(id),
                    _ => self.tile.iter().flat_map(|t| t.layout.images()).flat_map(|i| self.image_inputs(i)).collect(),
                };
                images
                    .into_iter()
                    .map(|image| {
                        let link = self.output_link(&image).and_then(|o| self.index.run(&o.name.run).map(|r| (o, r)));
                        let (title, text) = match link {
                            Some((o, (proj, run))) => {
                                (format!("{} · {} · seed {}", proj.name(), run.label, o.name.seed), run.prompt.clone())
                            }
                            None => {
                                let idea = self.index.ideas.iter().find(|e| e.idea.images.contains(&image.id));
                                let title = idea.map(|e| e.idea.title.clone()).filter(|t| !t.is_empty());
                                (
                                    title.unwrap_or_else(|| "(untitled)".into()),
                                    idea.map(|e| e.idea.text.clone()).unwrap_or_default(),
                                )
                            }
                        };
                        RefCandidate { image, title, text, verdict: None }
                    })
                    .filter(|c| q.is_empty() || has(&c.title) || has(&c.text))
                    .collect()
            }
            RefSource::Inputs(_) | RefSource::Inbox | RefSource::Edited => {
                let mut ideas: Vec<&IdeaEntry> = self
                    .index
                    .ideas
                    .iter()
                    .filter(|e| match (&e.loc, &p.source) {
                        (IdeaLoc::Unreviewed, RefSource::Inbox) | (IdeaLoc::Edited, RefSource::Edited) => true,
                        (IdeaLoc::Library(_), RefSource::Inputs(None)) => true,
                        (IdeaLoc::Library(f), RefSource::Inputs(Some(sel))) => f.starts_with(sel),
                        _ => false,
                    })
                    .filter(|e| {
                        let i = &e.idea;
                        q.is_empty() || has(&i.title) || has(&i.text) || i.tags.iter().any(|t| has(t))
                    })
                    .collect();
                ideas.sort_by(|a, b| b.idea.id.cmp(&a.idea.id));
                ideas
                    .into_iter()
                    .flat_map(|e| {
                        e.idea.images.iter().map(|id| RefCandidate {
                            image: ImageRef::input(id.clone()),
                            title: if e.idea.title.is_empty() { "(untitled)".into() } else { e.idea.title.clone() },
                            text: e.idea.text.clone(),
                            verdict: None,
                        })
                    })
                    .collect()
            }
            RefSource::Outputs(src) => self
                .source_images_in(src, p.ratings)
                .into_iter()
                .filter_map(|o| {
                    let (proj, run) = self.index.run(&o.name.run)?;
                    let title = proj.project.titles.get(&o.image.id);
                    (q.is_empty() || has(&run.prompt) || title.is_some_and(|t| has(t))).then(|| RefCandidate {
                        image: o.image.clone(),
                        title: match title {
                            Some(t) => format!("{t}\n{} · {} · seed {}", proj.name(), run.label, o.name.seed),
                            None => format!("{} · {} · seed {}", proj.name(), run.label, o.name.seed),
                        },
                        text: run.prompt.clone(),
                        verdict: Some(o.verdict),
                    })
                })
                .collect(),
        };
        v.into_iter().filter(|c| seen.insert(c.image.clone())).collect()
    }

    /// The images the large preview steps through with ← / →: Review's
    /// queue (single and grid), the contact sheet's images (row by row) or
    /// the Output library's grid; `None` elsewhere.
    pub fn preview_list(&self) -> Option<Vec<&OutputLink>> {
        match self.tab {
            Tab::Review if matches!(self.review_view, ReviewView::Single | ReviewView::Grid) => {
                Some(self.review_queue())
            }
            Tab::Review if self.review_view == ReviewView::Sheet => Some(
                self.sheet()?
                    .rows
                    .into_iter()
                    .flat_map(|r| r.cells)
                    .filter_map(|c| match c {
                        SheetCell::Image(o) => Some(o),
                        _ => None,
                    })
                    .collect(),
            ),
            Tab::OutputLibrary if self.output_tab == OutputTab::Grid => Some(self.output_source_images()),
            _ => None,
        }
    }

    /// Where the preview is in [`Self::preview_list`]: (index, count).
    pub fn preview_position(&self) -> Option<(usize, usize)> {
        let p = self.preview.as_ref()?;
        let list = self.preview_list()?;
        list.iter().position(|o| &o.image == p).map(|i| (i, list.len()))
    }

    /// Follow's countdown on the image shown, while it is on.
    pub fn follow_countdown(&self) -> Option<FollowCountdown> {
        if !self.review_follow {
            return None;
        }
        let ahead = self.review_queue().len().saturating_sub(self.review_cursor + 1);
        let secs = self.config.review_follow.secs(ahead, self.follow.interval(self.now));
        Some(FollowCountdown {
            left: (secs - self.follow.elapsed).max(0.0),
            secs,
            ahead,
            paused: self.follow.paused,
            held: self.follow.held || self.tab != Tab::Review || self.review_view == ReviewView::Sheet,
        })
    }

    /// Unreviewed outputs in the review scope, oldest first.
    pub fn review_queue(&self) -> Vec<&OutputLink> {
        let mut v: Vec<_> = self
            .index
            .projects
            .iter()
            .filter(|p| self.review_scope.as_ref().is_none_or(|s| &p.project.id == s))
            .filter(|p| self.review_folder.as_ref().is_none_or(|f| p.dir.starts_with(Path::new("projects").join(f))))
            .flat_map(|p| p.outputs.iter().map(move |o| (p, o)))
            .filter(|(p, o)| self.review_sweep.as_ref().is_none_or(|s| in_sweep(p, &o.name.run, s)))
            .filter(|(_, o)| self.review_run.as_ref().is_none_or(|r| &o.name.run == r))
            .map(|(_, o)| o)
            .filter(|o| o.verdict.is_none() && !self.reviewing.contains(&o.path))
            .collect();
        v.sort_by(|a, b| a.image.id.cmp(&b.image.id));
        v
    }

    /// What can be picked where we are, in order: Review's queue or contact
    /// sheet (row by row), or the Output library's Grid.
    pub fn pickable(&self) -> Vec<ImageRef> {
        match self.tab {
            Tab::Review if self.review_view == ReviewView::Sheet => self
                .sheet()
                .map(|sh| {
                    sh.rows
                        .iter()
                        .flat_map(|r| &r.cells)
                        .filter_map(|c| match c {
                            SheetCell::Image(o) => Some(o.image.clone()),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Tab::Review => self.review_queue().iter().map(|o| o.image.clone()).collect(),
            Tab::OutputLibrary if self.output_tab == OutputTab::Grid => {
                self.output_source_images().iter().map(|o| o.image.clone()).collect()
            }
            _ => vec![],
        }
    }

    /// The image the cursor is on (where picking with Ctrl or Shift starts).
    pub fn current_image(&self) -> Option<ImageRef> {
        match self.tab {
            Tab::Review if self.review_view == ReviewView::Sheet => {
                let sh = self.sheet()?;
                match sh.rows.get(self.sheet_cell.0)?.cells.get(self.sheet_cell.1)? {
                    SheetCell::Image(o) => Some(o.image.clone()),
                    _ => None,
                }
            }
            Tab::Review => self.review_queue().get(self.review_cursor).map(|o| o.image.clone()),
            Tab::OutputLibrary => self.output_selected.clone(),
            _ => None,
        }
    }

    /// The (column, row) of each image on the contact sheet, as shown
    /// (rows folded into bands side by side).
    pub fn sheet_cells(&self) -> HashMap<ImageRef, (u32, u32)> {
        let Some(sh) = self.sheet() else { return HashMap::new() };
        let (k, seeds) = (self.sheet_bands.max(1), sh.seeds.len());
        let mut m = HashMap::new();
        for (r, row) in sh.rows.iter().enumerate() {
            for (c, cell) in row.cells.iter().enumerate() {
                if let SheetCell::Image(o) = cell {
                    m.insert(o.image.clone(), (((r % k) * seeds + c) as u32, (r / k) as u32));
                }
            }
        }
        m
    }

    /// An output image's caption.
    pub fn caption(&self, image: &ImageRef, fill: CaptionFill) -> String {
        // an input: its title, else "input"
        let Some(o) = self.output_link(image) else {
            return match fill {
                CaptionFill::Clear => String::new(),
                CaptionFill::Title => self.input_title(image).unwrap_or_default().to_string(),
                _ => self.input_kind(image).into(),
            };
        };
        let run = self.index.run(&o.name.run).map(|(_, r)| r);
        match fill {
            CaptionFill::Clear => String::new(),
            CaptionFill::Style => {
                run.map(|r| r.sweep.as_ref().map_or(r.label.clone(), |s| s.row())).unwrap_or_default()
            }
            CaptionFill::Seed => format!("seed {}", o.name.seed),
            CaptionFill::Title => self.image_title(image).unwrap_or_default().to_string(),
            CaptionFill::Run => run.map(|r| r.label.clone()).unwrap_or_default(),
            CaptionFill::Prompt => run.map(|r| r.prompt.clone()).unwrap_or_default(),
            CaptionFill::Settings => run.map(settings_line).unwrap_or_default(),
            CaptionFill::StyleSeed => {
                format!("{} · seed {}", self.caption(image, CaptionFill::Style), o.name.seed)
            }
        }
    }

    /// A caption as shown: its `{placeholders}` (`CAPTION_FIELDS`) filled
    /// for the `n`th tile's image; unknown ones stay as typed.
    pub fn caption_text(&self, template: &str, image: Option<&ImageRef>, n: usize) -> String {
        if !template.contains('{') {
            return template.to_string();
        }
        let o = image.and_then(|i| self.output_link(i));
        let found = o.and_then(|o| self.index.run(&o.name.run));
        let fmt = crate::update::fmt_value;
        let field = |key: &str| -> Option<String> {
            let run = found.map(|(_, r)| r);
            Some(match key {
                "n" => n.to_string(),
                "style" => image.map(|i| self.caption(i, CaptionFill::Style)).unwrap_or_default(),
                "seed" => o.map(|o| o.name.seed.to_string()).unwrap_or_default(),
                "settings" => run.map(settings_line).unwrap_or_default(),
                "steps" => run.map(|r| r.sample.steps.to_string()).unwrap_or_default(),
                "cfg" => run.map(|r| fmt(r.sample.cfg)).unwrap_or_default(),
                "shift" => run.and_then(|r| r.sample.flow_shift).map(fmt).unwrap_or_default(),
                "strength" => match run.map(|r| &r.mode) {
                    Some(increart_core::model::Mode::Edit { strength, init: Some(_) }) => fmt(*strength),
                    _ => String::new(),
                },
                "title" => {
                    image.and_then(|i| self.image_title(i).or(self.input_title(i))).unwrap_or_default().to_string()
                }
                "kind" => match image {
                    None => "pasted",
                    Some(i) if self.input_kind(i) == "start" => "start",
                    Some(_) if o.is_some() => "output",
                    Some(_) => "input",
                }
                .to_string(),
                "run" => run.map(|r| r.label.clone()).unwrap_or_default(),
                "prompt" => run.map(|r| r.prompt.clone()).unwrap_or_default(),
                "project" => found.map(|(p, _)| p.name().to_string()).unwrap_or_default(),
                _ => return None,
            })
        };
        let mut out = String::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            match after.find('}').and_then(|close| field(&after[..close]).map(|v| (close, v))) {
                Some((close, v)) => {
                    out.push_str(&v);
                    rest = &after[close + 1..];
                }
                None => {
                    out.push('{');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// An output image's size, as its run asked for.
    pub fn output_size(&self, image: &ImageRef) -> Option<(u32, u32)> {
        let o = self.output_link(image)?;
        self.index.run(&o.name.run).map(|(_, r)| (r.width, r.height))
    }

    /// A project's runs for Review's scope panel, newest first: a single run
    /// on its own, a sweep once (at its newest run).
    pub fn review_runs<'a>(&self, p: &'a ProjectEntry) -> Vec<ReviewRun<'a>> {
        let mut v = vec![];
        let mut runs: Vec<_> = p.runs.iter().collect();
        runs.sort_by(|a, b| b.id.cmp(&a.id));
        for r in runs {
            match &r.sweep {
                None => v.push(ReviewRun::Single(r)),
                Some(s) if !v.contains(&ReviewRun::Sweep(&s.name)) => v.push(ReviewRun::Sweep(&s.name)),
                Some(_) => {}
            }
        }
        v
    }

    /// A project's runs with kept images, newest first, and how many.
    pub fn kept_runs<'a>(&self, p: &'a ProjectEntry) -> Vec<(&'a increart_core::model::Run, usize)> {
        let kept = self.kept_outputs();
        let mut v: Vec<_> = p
            .runs
            .iter()
            .map(|r| (r, kept.iter().filter(|o| o.name.run == r.id).count()))
            .filter(|(_, n)| *n > 0)
            .collect();
        v.sort_by(|a, b| b.0.id.cmp(&a.0.id));
        v
    }

    /// A sweep's newest run.
    pub fn sweep_newest<'a>(&self, p: &'a ProjectEntry, sweep: &str) -> Option<&'a increart_core::model::Run> {
        p.runs.iter().filter(|r| r.sweep.as_ref().is_some_and(|s| s.name == sweep)).max_by(|a, b| a.id.cmp(&b.id))
    }

    /// Unreviewed images of a run.
    pub fn run_unreviewed(&self, p: &ProjectEntry, run: &Id) -> usize {
        p.outputs.iter().filter(|o| o.verdict.is_none() && &o.name.run == run).count()
    }

    /// Unreviewed images of a sweep.
    pub fn sweep_unreviewed(&self, p: &ProjectEntry, sweep: &str) -> usize {
        p.outputs.iter().filter(|o| o.verdict.is_none() && in_sweep(p, &o.name.run, sweep)).count()
    }

    /// The interface zoom in percent.
    pub fn ui_zoom(&self) -> u32 {
        match self.config.ui_zoom {
            0 => 100,
            n => n.clamp(UI_ZOOM_MIN, UI_ZOOM_MAX),
        }
    }

    /// An image grid's tile size in points.
    pub fn grid_cell_size(&self, grid: ZoomGrid) -> u32 {
        let n = match grid {
            ZoomGrid::Review => self.config.review_grid_cell,
            ZoomGrid::Output => self.config.output_grid_cell,
            ZoomGrid::Library => self.config.library_grid_cell,
            ZoomGrid::Inbox => self.config.inbox_grid_cell,
        };
        if n == 0 { grid.default_size() } else { n.clamp(GRID_CELL_MIN, GRID_CELL_MAX) }
    }

    /// The contact sheet's cell size in points.
    pub fn sheet_cell_size(&self) -> u32 {
        match self.config.sheet_cell {
            0 => SHEET_CELL_DEFAULT,
            n => n.clamp(SHEET_CELL_MIN, SHEET_CELL_MAX),
        }
    }

    /// What Review's **Generate ›** opens: the project of the image shown
    /// (the cursor's; on the contact sheet, its row's), else the scope's
    /// project; with that image and its run, when there is one.
    pub fn review_generate(&self) -> Option<ReviewGenerate> {
        let (image, run) = if self.review_view == ReviewView::Sheet {
            let sheet = self.sheet();
            let row = sheet.as_ref().and_then(|sh| sh.rows.get(self.sheet_cell.0));
            let image = row.and_then(|r| match r.cells.get(self.sheet_cell.1) {
                Some(SheetCell::Image(o)) => Some(o.image.clone()),
                _ => None,
            });
            (image, row.map(|r| r.run.id.clone()))
        } else {
            let o = self.review_queue().get(self.review_cursor).map(|o| (o.image.clone(), o.name.run.clone()));
            (o.as_ref().map(|o| o.0.clone()), o.map(|o| o.1))
        };
        let project = match run.as_ref().and_then(|r| self.index.run(r)) {
            Some((p, _)) => p.project.id.clone(),
            None => self.review_scope.clone()?,
        };
        let name = self.index.project(&project)?.name();
        Some(ReviewGenerate { project, name, image, run })
    }

    /// The Review scope's sweep as a contact sheet.
    pub fn sheet(&self) -> Option<Sheet<'_>> {
        let name = self.review_sweep.as_ref()?;
        let p = self.index.project(self.review_scope.as_ref()?)?;
        let mut runs: Vec<_> = p.runs.iter().filter(|r| r.sweep.as_ref().is_some_and(|s| &s.name == name)).collect();
        runs.sort_by(|a, b| a.id.cmp(&b.id));
        // the seeds are the whole sweep's, so columns stay put while filtering
        let mut seeds: Vec<u64> = runs.iter().flat_map(|r| r.all_seeds().map(|(_, s)| s)).collect();
        seeds.sort();
        seeds.dedup();
        let words: Vec<String> = self.sheet_filter.to_lowercase().split_whitespace().map(String::from).collect();
        let total = runs.len();
        runs.retain(|r| {
            let name = format!("{} {}", r.sweep.as_ref().map_or(String::new(), |s| s.row()), r.label).to_lowercase();
            words.iter().all(|w| name.contains(w))
        });
        let hidden = total - runs.len();
        let rows = runs
            .into_iter()
            .map(|run| {
                let cells = seeds
                    .iter()
                    .map(|seed| {
                        let newest = p
                            .outputs
                            .iter()
                            .filter(|o| o.name.run == run.id && o.name.seed == *seed)
                            .max_by_key(|o| o.name.rev);
                        let asked = run.all_seeds().any(|(_, s)| s == *seed);
                        let dropped = run.failed.iter().any(|f| f.seed == *seed)
                            || run.cancelled.iter().any(|c| (c.first..c.first + c.count).contains(seed));
                        match newest {
                            Some(o) => SheetCell::Image(o),
                            None if asked && !dropped => SheetCell::Pending,
                            None => SheetCell::Empty,
                        }
                    })
                    .collect();
                SheetRow { run, cells }
            })
            .collect();
        Some(Sheet { seeds, rows, hidden })
    }

    pub fn unreviewed_outputs(&self) -> usize {
        self.index.projects.iter().flat_map(|p| &p.outputs).filter(|o| o.verdict.is_none()).count()
    }

    /// The project holding an output link.
    pub fn project_of_link(&self, link: &OutputLink) -> Option<&ProjectEntry> {
        self.index.projects.iter().find(|p| link.path.starts_with(&p.dir))
    }

    /// An output image's title (project.rsn `titles`), if it has one.
    /// What an output was made from: its start image, then its references
    /// (none for other images).
    pub fn image_inputs(&self, image: &ImageRef) -> Vec<ImageRef> {
        let Some((_, run)) = self.output_link(image).and_then(|o| self.index.run(&o.name.run)) else { return vec![] };
        let mut v = vec![];
        run_inputs(run, &mut v);
        v
    }

    /// What `input` was to the output `image`'s run: `Some(None)` its start
    /// image, `Some(Some(n))` its reference `<imageN>`.
    pub fn input_role(&self, image: &ImageRef, input: &ImageRef) -> Option<Option<usize>> {
        let (_, run) = self.output_link(image).and_then(|o| self.index.run(&o.name.run))?;
        if let Some(n) = run.refs.iter().position(|r| r == input) {
            return Some(Some(n + 1));
        }
        match &run.mode {
            increart_core::model::Mode::Edit { init: Some(i), .. } if i == input => Some(None),
            _ => None,
        }
    }

    /// What a project's runs were made from, the newest run's first, each
    /// image once.
    pub fn project_inputs(&self, project: &Id) -> Vec<ImageRef> {
        let mut v = vec![];
        if let Some(p) = self.index.projects.iter().find(|p| &p.project.id == project) {
            for run in p.runs.iter().rev() {
                run_inputs(run, &mut v);
            }
        }
        v
    }

    /// What Compare with… compares against: Review's image, or the Output
    /// library's selected one.
    pub fn compare_b(&self) -> Option<ImageRef> {
        if self.tab == Tab::OutputLibrary {
            return self.output_selected.clone();
        }
        let queue = self.review_queue();
        queue.get(self.review_cursor.min(queue.len().saturating_sub(1))).map(|o| o.image.clone())
    }

    /// A tiled input's `{kind}`: Generate's start image, else "input".
    fn input_kind(&self, image: &ImageRef) -> &'static str {
        if self.tile.as_ref().and_then(|t| t.start.as_ref()) == Some(image) { "start" } else { "input" }
    }

    /// An input image's idea's title.
    /// What an edited copy (Crop / mark…) was made from, and how.
    pub fn edited_source(&self, image: &ImageRef) -> Option<(&ImageRef, &ImageEdits)> {
        self.index.ideas.iter().filter(|e| e.loc == IdeaLoc::Edited && e.idea.images.contains(&image.id)).find_map(
            |e| match &e.idea.source {
                IdeaSource::Edited { image, edits } => Some((image, edits)),
                _ => None,
            },
        )
    }

    /// The edited copies made from an image, newest first.
    pub fn edited_copies(&self, image: &ImageRef) -> Vec<&IdeaEntry> {
        let mut v: Vec<_> = self
            .index
            .ideas
            .iter()
            .filter(|e| e.loc == IdeaLoc::Edited)
            .filter(|e| matches!(&e.idea.source, IdeaSource::Edited { image: i, .. } if i == image))
            .collect();
        v.sort_by(|a, b| b.idea.id.cmp(&a.idea.id));
        v
    }

    pub fn input_title(&self, image: &ImageRef) -> Option<&str> {
        self.index.ideas.iter().find(|e| e.idea.images.contains(&image.id)).map(|e| e.idea.title.as_str())
    }

    pub fn image_title(&self, image: &ImageRef) -> Option<&str> {
        let p = self.project_of_link(self.output_link(image)?)?;
        p.project.titles.get(&image.id).map(String::as_str)
    }

    /// What each style (a sweep's prompt preset) has made, in every project.
    pub fn style_stats(&self) -> HashMap<increart_core::prompts::PresetKey, StyleStats> {
        let mut m: HashMap<increart_core::prompts::PresetKey, StyleStats> = HashMap::new();
        for p in &self.index.projects {
            let here = self.project.as_ref() == Some(&p.project.id);
            let mut of_run = HashMap::new();
            for r in &p.runs {
                let Some(tag) = r.sweep.as_ref().filter(|t| !t.preset.is_empty()) else { continue };
                let key = increart_core::prompts::PresetKey { task: tag.task.clone(), name: tag.preset.clone() };
                let s = m.entry(key.clone()).or_default();
                s.here += here as usize;
                if s.last.as_ref().is_none_or(|l| *l < r.id) {
                    s.last = Some(r.id.clone());
                }
                of_run.insert(&r.id, key);
            }
            // newest last, so the sample ends up the newest
            let mut outs: Vec<_> = p.outputs.iter().filter(|o| of_run.contains_key(&o.name.run)).collect();
            outs.sort_by(|a, b| (&a.name.run, a.name.seed).cmp(&(&b.name.run, b.name.seed)));
            for o in outs {
                let Some(s) = of_run.get(&o.name.run).and_then(|k| m.get_mut(k)) else { continue };
                let good = o.verdict == Some(Verdict::Good);
                match o.verdict {
                    Some(Verdict::Good) => s.good += 1,
                    Some(Verdict::Neutral) => s.neutral += 1,
                    Some(Verdict::Bad) => s.bad += 1,
                    None => s.unrated += 1,
                }
                if good || s.good == 0 {
                    s.sample = Some(o.image.clone());
                }
            }
        }
        m
    }

    /// The presets (task, preset) Choose styles lists: those the search
    /// finds in every task, else the view's; filtered, then sorted.
    pub fn style_rows(&self, picker: &StylePicker) -> Vec<(usize, usize)> {
        use increart_core::config::StyleSort;
        let list = &self.prompt_presets;
        let ticked = &self.generate.sweep.selected;
        let refs = self.generate.refs().len();
        let q = picker.search.to_lowercase();
        let none = StyleStats::default();
        let stats = |t: &str, p: &PromptPreset| {
            picker
                .stats
                .get(&increart_core::prompts::PresetKey { task: t.into(), name: p.name.clone() })
                .unwrap_or(&none)
        };
        let f = picker.filter;
        let mut v = vec![];
        for (ti, t) in list.tasks.iter().enumerate() {
            for (pi, p) in t.presets.iter().enumerate() {
                let in_view = if !q.is_empty() {
                    preset_matches(p, &q)
                } else {
                    match &picker.view {
                        StyleView::Ticked => ticked.iter().any(|k| k.task == t.name && k.name == p.name),
                        StyleView::Favourites => p.favourite,
                        StyleView::All => true,
                        StyleView::Task(x) => *x == t.name,
                        StyleView::Group(x, g) => *x == t.name && *g == p.group,
                    }
                };
                let s = stats(&t.name, p);
                let keep = in_view
                    && (!f.favourites || p.favourite)
                    && (!f.usable || needs_refs(&p.prompt) <= refs)
                    && (!f.untried_here || s.here == 0)
                    && (!f.has_good || s.good > 0);
                if keep {
                    v.push((ti, pi));
                }
            }
        }
        let at = |&(t, p): &(usize, usize)| (&list.tasks[t], &list.tasks[t].presets[p]);
        match self.config.style_sort {
            StyleSort::Listed => {}
            StyleSort::Name => v.sort_by_cached_key(|x| at(x).1.name.to_lowercase()),
            StyleSort::MostGood => v.sort_by_key(|x| {
                let (t, p) = at(x);
                std::cmp::Reverse(stats(&t.name, p).good)
            }),
            StyleSort::Recent => v.sort_by_key(|x| {
                let (t, p) = at(x);
                std::cmp::Reverse(stats(&t.name, p).last.clone())
            }),
            StyleSort::Untried => v.sort_by_key(|x| {
                let (t, p) = at(x);
                stats(&t.name, p).last.clone()
            }),
        }
        v
    }

    /// The link of a store image (the first project link to it), any verdict.
    pub fn output_link(&self, image: &ImageRef) -> Option<&OutputLink> {
        self.index.projects.iter().flat_map(|p| &p.outputs).find(|o| &o.image == image)
    }

    /// The Output library Grid's images for the chosen source, newest first.
    pub fn output_source_images(&self) -> Vec<&OutputLink> {
        let f = &self.output_filter;
        let mut v = self.source_images_in(&self.output_source, f.ratings);
        let q = f.text.to_lowercase();
        v.retain(|o| {
            let run = self.index.run(&o.name.run).map(|(_, r)| r);
            let title = || self.image_title(&o.image).is_some_and(|t| t.to_lowercase().contains(&q));
            (q.is_empty() || run.is_some_and(|r| r.prompt.to_lowercase().contains(&q)) || title())
                && (!f.has_refs || run.is_some_and(|r| !r.refs.is_empty()))
                && (!f.transparent || self.index.transparent(&o.image))
        });
        v
    }

    /// Unreviewed images of a project source (0 for the other sources).
    pub fn source_unreviewed(&self, source: &OutputSource) -> usize {
        let dir = match source {
            OutputSource::Project(id) => match self.index.project(id) {
                Some(p) => p.dir.clone(),
                None => return 0,
            },
            OutputSource::ProjectFolder(f) => Path::new("projects").join(f),
            _ => return 0,
        };
        self.index.projects.iter().filter(|p| p.dir.starts_with(&dir)).map(|p| counts(p)[3]).sum()
    }

    /// A source's kept images (no filters), newest first.
    pub fn source_images(&self, source: &OutputSource) -> Vec<&OutputLink> {
        self.source_images_in(source, Ratings::KEPT)
    }

    /// Whether the filters differ from the preset tab last clicked.
    pub fn output_preset_modified(&self) -> bool {
        self.output_filter != self.output_preset.filter()
    }

    /// A source's images with these ratings (no other filters), newest first.
    pub fn source_images_in(&self, source: &OutputSource, ratings: Ratings) -> Vec<&OutputLink> {
        let kept: Vec<&OutputLink> = self.all_outputs().into_iter().filter(|o| ratings.allows(o.verdict)).collect();
        let v: Vec<&OutputLink> = match source {
            OutputSource::AllProjects => kept,
            OutputSource::Project(id) => {
                let dir = self.index.project(id).map(|p| p.dir.clone());
                kept.into_iter().filter(|o| dir.as_ref().is_some_and(|d| o.path.starts_with(d))).collect()
            }
            OutputSource::ProjectFolder(f) => {
                let dir = Path::new("projects").join(f);
                kept.into_iter().filter(|o| o.path.starts_with(&dir)).collect()
            }
            OutputSource::Smart(Smart::Day(day)) => {
                kept.into_iter().filter(|o| o.image.id.as_str().starts_with(day.as_str())).collect()
            }
            OutputSource::Smart(Smart::Idea(idea)) => kept
                .into_iter()
                .filter(|o| self.index.run(&o.name.run).is_some_and(|(_, r)| r.source_idea.as_ref() == Some(idea)))
                .collect(),
            OutputSource::Smart(Smart::Transparent) => {
                kept.into_iter().filter(|o| self.index.transparent(&o.image)).collect()
            }
            OutputSource::Run(id) => kept.into_iter().filter(|o| &o.name.run == id).collect(),
            OutputSource::Collection(folder) => {
                let mut seen = std::collections::HashSet::new();
                let images: Vec<ImageRef> = self
                    .index
                    .collection_links
                    .iter()
                    .filter(|l| l.folder.starts_with(folder))
                    .map(|l| l.image.clone())
                    .filter(|i| seen.insert(i.clone()))
                    .collect();
                // Any rating: a collection holds what you put in it.
                images.iter().filter_map(|i| self.output_link(i)).collect()
            }
        };
        v
    }

    /// A smart idea folder's name: "title · id".
    fn smart_idea_name(&self, id: &Id) -> String {
        let title = self.idea(id).map(|e| e.idea.title.clone()).filter(|t| !t.is_empty());
        title.map_or_else(|| id.to_string(), |t| format!("{t} · {id}"))
    }

    /// A folder of the project tree (relative to `projects/`, empty = the
    /// top): its subfolders, then its projects, that have kept images.
    pub fn kept_project_children(&self, folder: &Path) -> (Vec<&PathBuf>, Vec<&ProjectEntry>) {
        let kept = self.kept_outputs();
        let has_kept = |d: &Path| kept.iter().any(|o| o.path.starts_with(d));
        let mut folders: Vec<&PathBuf> = self
            .index
            .project_folders
            .iter()
            .filter(|f| f.parent() == Some(folder) && has_kept(&Path::new("projects").join(f)))
            .collect();
        folders.sort_by_key(|f| f.to_string_lossy().to_lowercase());
        let dir = Path::new("projects").join(folder);
        let mut projects: Vec<&ProjectEntry> =
            self.index.projects.iter().filter(|p| p.dir.parent() == Some(dir.as_path()) && has_kept(&p.dir)).collect();
        projects.sort_by_key(|p| p.name().to_lowercase());
        (folders, projects)
    }

    /// Whether the "by project" tree shows `source` with the project-name
    /// search typed: a project shows when its name or a folder above it
    /// has the text, a folder also when something inside it has.
    pub fn project_search_shows(&self, source: &OutputSource) -> bool {
        let q = self.output_project_search.trim().to_lowercase();
        if !self.output_search_projects || q.is_empty() {
            return true;
        }
        let has = |p: &Path| p.iter().any(|c| c.to_string_lossy().to_lowercase().contains(&q));
        let rel = |e: &ProjectEntry| e.dir.strip_prefix("projects").map(Path::to_path_buf).unwrap_or_default();
        match source {
            OutputSource::Project(id) => {
                self.index.projects.iter().find(|p| &p.project.id == id).is_none_or(|p| has(&rel(p)))
            }
            OutputSource::Run(id) => self.index.run(id).is_none_or(|(p, _)| has(&rel(p))),
            OutputSource::ProjectFolder(f) => {
                has(f)
                    || self.index.project_folders.iter().any(|d| d.starts_with(f) && has(d))
                    || self.index.projects.iter().any(|p| {
                        let r = rel(p);
                        r.starts_with(f) && has(&r)
                    })
            }
            _ => true,
        }
    }

    /// The project at a path relative to `projects/`.
    fn project_at(&self, rel: &Path) -> Option<&ProjectEntry> {
        let dir = Path::new("projects").join(rel);
        self.index.projects.iter().find(|p| p.dir == dir)
    }

    /// The subfolders of a folder of the virtual `smart/` tree (`rel` is
    /// the path under it), if it has subfolders rather than images.
    pub fn smart_children(&self, rel: &Path) -> Option<Vec<String>> {
        let parts: Vec<String> = rel.iter().map(|c| c.to_string_lossy().into_owned()).collect();
        Some(match parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            [] => ["by date", "by source idea", "by project", "transparent"].map(String::from).to_vec(),
            ["by date"] => self.kept_days().into_iter().map(|(d, _)| d).collect(),
            ["by source idea"] => self.kept_source_ideas().iter().map(|(id, _)| self.smart_idea_name(id)).collect(),
            // the project tree, folders and all
            ["by project", rest @ ..] => {
                let rel: PathBuf = rest.iter().collect();
                if self.project_at(&rel).is_some() {
                    return None;
                }
                let (folders, projects) = self.kept_project_children(&rel);
                let name = |p: &Path| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                folders.into_iter().map(|f| name(f)).chain(projects.into_iter().map(|p| p.name())).collect()
            }
            _ => return None,
        })
    }

    /// The source a folder of the virtual `smart/` tree shows, if it is one
    /// that holds images.
    pub fn smart_source(&self, rel: &Path) -> Option<OutputSource> {
        let parts: Vec<String> = rel.iter().map(|c| c.to_string_lossy().into_owned()).collect();
        Some(match parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["by date", day] => OutputSource::Smart(Smart::Day(day.to_string())),
            ["by source idea", name] => {
                let id = self
                    .kept_source_ideas()
                    .into_iter()
                    .map(|(id, _)| id)
                    .find(|id| self.smart_idea_name(id) == *name)?;
                OutputSource::Smart(Smart::Idea(id))
            }
            ["by project", rest @ ..] if !rest.is_empty() => {
                let rel: PathBuf = rest.iter().collect();
                match self.project_at(&rel) {
                    Some(p) => OutputSource::Project(p.project.id.clone()),
                    None if self.index.project_folders.contains(&rel) => OutputSource::ProjectFolder(rel),
                    None => return None,
                }
            }
            ["transparent"] => OutputSource::Smart(Smart::Transparent),
            _ => return None,
        })
    }

    /// A folder of the virtual `smart/` tree as folder-view entries.
    pub fn smart_listing(&self, rel: &Path) -> Vec<FvEntry> {
        if let Some(children) = self.smart_children(rel) {
            return children
                .into_iter()
                .map(|name| {
                    let n = self.smart_images(&rel.join(&name)).len();
                    FvEntry {
                        info: format!("{n} image(s)"),
                        name,
                        kind: EntryKind::Smart,
                        size: n as u64,
                        modified: None,
                    }
                })
                .collect();
        }
        self.smart_images(rel)
            .into_iter()
            .map(|o| FvEntry {
                name: o.path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                kind: EntryKind::Image(o.image.clone()),
                size: 0,
                modified: None,
                info: match o.verdict {
                    Some(Verdict::Good) => "good",
                    Some(Verdict::Neutral) => "neutral",
                    Some(Verdict::Bad) => "bad",
                    None => "unreviewed",
                }
                .to_string(),
            })
            .collect()
    }

    /// Every image in a folder of the `smart/` tree and its subfolders.
    pub fn smart_images(&self, rel: &Path) -> Vec<&OutputLink> {
        if let Some(src) = self.smart_source(rel) {
            return self.source_images(&src);
        }
        match self.smart_children(rel) {
            Some(_) if rel.as_os_str().is_empty() || rel == Path::new("by date") || rel == Path::new("by project") => {
                self.kept_outputs()
            }
            Some(children) => {
                let mut seen = std::collections::HashSet::new();
                let mut v: Vec<&OutputLink> = children
                    .iter()
                    .flat_map(|c| self.smart_images(&rel.join(c)))
                    .filter(|o| seen.insert(o.path.clone()))
                    .collect();
                v.sort_by(|a, b| b.image.id.cmp(&a.image.id));
                v
            }
            None => vec![],
        }
    }

    /// The image a folder-view path (relative to the library root) shows:
    /// a project's link, a collection's link or a smart folder's image.
    pub fn image_at(&self, path: &Path) -> Option<ImageRef> {
        if is_smart(path) {
            let rel = path.parent()?.strip_prefix(SMART_DIR).ok()?;
            let name = path.file_name()?;
            return self
                .smart_images(rel)
                .into_iter()
                .find(|o| o.path.file_name() == Some(name))
                .map(|o| o.image.clone());
        }
        let in_collections = self.index.collection_links.iter().find(|l| l.path == path).map(|l| l.image.clone());
        in_collections.or_else(|| {
            self.index.projects.iter().flat_map(|p| &p.outputs).find(|o| o.path == path).map(|o| o.image.clone())
        })
    }

    /// The library images a path names (absolute, or relative to the
    /// root): a store file, an image link ([`image_at`](Self::image_at)),
    /// else every image under it as a folder (a project's or a collection's
    /// links, ideas' images), once each. None for anything else.
    pub fn images_at(&self, path: &Path) -> Vec<ImageRef> {
        let rel = match (path.is_absolute(), &self.library) {
            (false, _) => path,
            (true, Some(root)) => match path.strip_prefix(root) {
                Ok(rel) => rel,
                Err(_) => return vec![],
            },
            (true, None) => return vec![],
        };
        // the root itself: not the whole library
        if rel.as_os_str().is_empty() {
            return vec![];
        }
        let store = rel.parent().and_then(|d| d.to_str()).and_then(increart_core::model::Store::from_dir);
        let id = rel.file_stem().and_then(|s| s.to_str()?.parse::<Id>().ok());
        if let (Some(store), Some(id)) = (store, id) {
            return vec![ImageRef { id, store }];
        }
        if let Some(image) = self.image_at(rel) {
            return vec![image];
        }
        let outputs = self.index.projects.iter().flat_map(|p| &p.outputs).map(|o| (&o.path, &o.image));
        let collected = self.index.collection_links.iter().map(|l| (&l.path, &l.image));
        let mut images: Vec<ImageRef> =
            outputs.chain(collected).filter(|(p, _)| p.starts_with(rel)).map(|(_, i)| i.clone()).collect();
        let ideas = self.index.ideas.iter().filter(|e| e.dir.starts_with(rel));
        images.extend(ideas.flat_map(|e| e.idea.images.iter().cloned().map(ImageRef::input)));
        let mut seen = BTreeSet::new();
        images.retain(|i| seen.insert(i.clone()));
        images
    }

    /// Days with kept images (`YYYY.MM.DD`), newest first.
    pub fn kept_days(&self) -> Vec<(String, usize)> {
        let mut days: std::collections::BTreeMap<String, usize> = Default::default();
        for o in self.kept_outputs() {
            *days.entry(o.image.id.as_str()[..10].to_string()).or_default() += 1;
        }
        days.into_iter().rev().collect()
    }

    /// Source ideas of kept images, with counts.
    pub fn kept_source_ideas(&self) -> Vec<(Id, usize)> {
        let mut ideas: std::collections::BTreeMap<Id, usize> = Default::default();
        for o in self.kept_outputs() {
            if let Some(id) = self.index.run(&o.name.run).and_then(|(_, r)| r.source_idea.clone()) {
                *ideas.entry(id).or_default() += 1;
            }
        }
        ideas.into_iter().collect()
    }

    /// Lineage of an output: the image it was edited from (its recipe's
    /// parent) and the outputs edited from it.
    pub fn lineage(&self, image: &ImageRef) -> (Option<ImageRef>, Vec<ImageRef>) {
        let parent = self.recipes.get(&image.id).cloned().flatten().and_then(|r| r.parent);
        let children = self
            .index
            .projects
            .iter()
            .flat_map(|p| &p.outputs)
            .filter(|o| {
                self.index.run(&o.name.run).is_some_and(|(_, r)| {
                    matches!(&r.mode, increart_core::model::Mode::Edit { init: Some(i), .. } if i == image)
                        || r.refs.contains(image)
                })
            })
            .map(|o| o.image.clone())
            .collect();
        (parent, children)
    }

    /// What deleting a tree node removes, for the confirmation:
    /// "2 folders, 3 projects, 41 images; 12 pending seeds will be cancelled".
    pub fn delete_summary(&self, tree: increart_core::Tree, path: &Path) -> String {
        use increart_core::Tree;
        match tree {
            Tree::Projects => {
                let base = Path::new("projects").join(path);
                let ps: Vec<_> = self.index.projects.iter().filter(|p| p.dir.starts_with(&base)).collect();
                let folders =
                    self.index.project_folders.iter().filter(|f| f.starts_with(path) && f.as_path() != path).count();
                let images: usize = ps.iter().map(|p| p.outputs.len()).sum();
                let pending: u64 = ps
                    .iter()
                    .flat_map(|p| &p.runs)
                    .map(|r| r.live_seeds().count() as u64)
                    .sum::<u64>()
                    .saturating_sub(images as u64);
                let mut s = format!(
                    "{folders} folder(s); {} project(s) with {images} image link(s) will be discarded (restorable)",
                    ps.len()
                );
                if pending > 0 {
                    s.push_str(&format!("; {pending} pending seed(s) won't be generated unless restored"));
                }
                s
            }
            Tree::InputLibrary => {
                let n = self
                    .index
                    .ideas
                    .iter()
                    .filter(|e| matches!(&e.loc, IdeaLoc::Library(f) if f.starts_with(path)))
                    .count();
                format!("{n} idea(s) inside will be discarded (restorable)")
            }
            Tree::Collections => {
                let n = self.index.collection_links.iter().filter(|l| l.folder.starts_with(path)).count();
                format!("{n} link(s); the images stay")
            }
        }
    }

    /// Discarded output images (every project's `discarded/`), newest
    /// first.
    pub fn discarded_outputs(&self) -> Vec<&OutputLink> {
        let mut v: Vec<_> = self.index.projects.iter().flat_map(|p| &p.discarded).collect();
        v.sort_by(|a, b| b.image.id.cmp(&a.image.id));
        v
    }

    /// Every output image, any rating or none, newest first.
    pub fn all_outputs(&self) -> Vec<&OutputLink> {
        let mut v: Vec<_> = self.index.projects.iter().flat_map(|p| &p.outputs).collect();
        v.sort_by(|a, b| b.image.id.cmp(&a.image.id));
        v
    }

    /// Kept (good + neutral) images, newest first.
    pub fn kept_outputs(&self) -> Vec<&OutputLink> {
        let mut v: Vec<_> = self
            .index
            .projects
            .iter()
            .flat_map(|p| &p.outputs)
            .filter(|o| matches!(o.verdict, Some(Verdict::Good | Verdict::Neutral)))
            .collect();
        v.sort_by(|a, b| b.image.id.cmp(&a.image.id));
        v
    }

    pub fn messages_error(&mut self, text: impl Into<String>) {
        let text = text.into();
        increart_core::applog::line("error", &text);
        self.messages.push(Message { text, error: true, alert: false, log: None });
    }

    pub fn messages_info(&mut self, text: impl Into<String>) {
        let text = text.into();
        increart_core::applog::line("info", &text);
        self.messages.push(Message { text, error: false, alert: false, log: None });
    }

    /// A notification alert's reason (its message, when so set).
    pub fn messages_alert(&mut self, text: impl Into<String>) {
        let text = text.into();
        increart_core::applog::line("alert", &text);
        self.messages.push(Message { text, error: false, alert: true, log: None });
    }

    /// An error about a server (`"<name>: …"`, as the engine words them),
    /// with that server's log when the app started it.
    pub fn messages_server_error(&mut self, text: impl Into<String>) {
        let text = text.into();
        increart_core::applog::line("error", &text);
        let log = self
            .servers
            .iter()
            .filter(|s| text.strip_prefix(s.name.as_str()).is_some_and(|rest| rest.starts_with(':')))
            .find_map(|s| match &s.config {
                Some(c @ (ServerConfig::Start { .. } | ServerConfig::Kobold(_))) => {
                    let path = increart_core::launcher::log_path(Path::new(""), c);
                    Some(path.display().to_string())
                }
                _ => None,
            });
        self.messages.push(Message { text, error: true, alert: false, log });
    }
}

/// Progress of one run that has seeds in the engine.
#[derive(Clone, Debug, PartialEq)]
pub struct RunProgress {
    pub run: Id,
    pub project_id: Id,
    pub project: String,
    /// Its sweep's label (`r05`) and its row there (the style, "cfg 3").
    pub sweep: Option<(String, String)>,
    pub label: String,
    pub prompt: String,
    /// Seeds with an image.
    pub done: u64,
    /// Seeds to generate in all (cancelled and failed ones excluded).
    pub total: u64,
    /// (seed, step, steps) of seeds being sampled now.
    pub sampling: Vec<(u64, u32, u32)>,
    /// Its images so far (DESIGN.md, Timing).
    pub timing: Option<crate::timing::Timing>,
    /// Seconds per step of the seeds being sampled now (mean).
    pub step_s: Option<f64>,
    /// Steps per image.
    pub steps: u32,
}

impl RunProgress {
    /// 0..1, counting partly sampled seeds by their steps.
    pub fn fraction(&self) -> f32 {
        let partial: f32 = self.sampling.iter().map(|(_, s, n)| *s as f32 / (*n).max(1) as f32).sum();
        ((self.done as f32 + partial) / self.total.max(1) as f32).min(1.0)
    }

    /// Seconds per image: its images' mean so far, else from the live
    /// seconds per step.
    pub fn image_s(&self) -> Option<f64> {
        self.timing.as_ref().map(|t| t.per_image).or_else(|| self.step_s.map(|s| s * self.steps as f64))
    }

    /// Time left, roughly: the images still to make (partly sampled ones by
    /// their steps) at [`Self::image_s`], as many at once as are sampling now.
    pub fn eta_s(&self) -> Option<f64> {
        let partial: f64 = self.sampling.iter().map(|(_, s, n)| *s as f64 / (*n).max(1) as f64).sum();
        let left = (self.total.saturating_sub(self.done) as f64 - partial).max(0.0);
        Some(left * self.image_s()? / self.sampling.len().max(1) as f64)
    }
}

/// **New sweep from these**: a sweep's runs to sweep again.
#[derive(Clone, Debug, PartialEq)]
pub struct NewSweepAsk {
    pub project: Id,
    /// The sweep's label (`r05`).
    pub sweep: String,
    pub runs: Vec<Id>,
}

/// A project's runs in the engine (Jobs).
#[derive(Clone, Debug, PartialEq)]
pub struct RunGroup {
    pub project: Id,
    pub name: String,
    pub items: Vec<RunItem>,
}

impl RunGroup {
    pub fn runs(&self) -> Vec<&RunProgress> {
        self.items.iter().flat_map(RunItem::runs).collect()
    }
}

/// A run on its own, or a sweep's runs in the engine.
#[derive(Clone, Debug, PartialEq)]
pub enum RunItem {
    Run(Box<RunProgress>),
    Sweep { name: String, runs: Vec<RunProgress> },
}

impl RunItem {
    pub fn runs(&self) -> Vec<&RunProgress> {
        match self {
            RunItem::Run(r) => vec![r.as_ref()],
            RunItem::Sweep { runs, .. } => runs.iter().collect(),
        }
    }
}

/// Several runs together: seeds done and to do, 0..1 (partly sampled seeds
/// by their steps), and roughly the time left: the images still to make at
/// each run's time per image (else the others' mean), `servers` at once.
pub fn runs_progress(runs: &[&RunProgress], servers: usize) -> (u64, u64, f32, Option<f64>) {
    let done: u64 = runs.iter().map(|r| r.done).sum();
    let total: u64 = runs.iter().map(|r| r.total).sum();
    let weighted: f32 = runs.iter().map(|r| r.fraction() * r.total as f32).sum();
    let fraction = if total == 0 { 0.0 } else { (weighted / total as f32).min(1.0) };
    let known: Vec<f64> = runs.iter().filter_map(|r| r.image_s()).collect();
    let eta = (!known.is_empty()).then(|| {
        let mean = known.iter().sum::<f64>() / known.len() as f64;
        let left: f64 =
            runs.iter().map(|r| (1.0 - r.fraction() as f64) * r.total as f64 * r.image_s().unwrap_or(mean)).sum();
        left / servers.max(1) as f64
    });
    (done, total, fraction, eta)
}

/// What a time per image is from ([`AppState::image_s_for`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// Images like it (model, size, steps, references, CFG) on these
    /// servers' devices.
    Like,
    /// Other images, scaled by steps and pixels.
    Scaled,
    /// None timed: the rough formula for one R9700.
    Rough,
}

impl Basis {
    pub fn text(self) -> &'static str {
        match self {
            Basis::Like => "from images like these (model, size, steps, references, CFG) on these servers",
            Basis::Scaled => "scaled by steps and pixels from other images timed on these servers",
            Basis::Rough => "on one R9700, a rough estimate (no images like these timed yet)",
        }
    }
}

/// How long runs not queued yet would take ([`AppState::estimate_new`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NewEstimate {
    /// On the servers working, alone.
    pub alone_s: f64,
    /// Until they'd be done with the runs in the engine (sharing the
    /// servers as the order has it); `None` with none there.
    pub with_queue_s: Option<f64>,
    pub servers: usize,
}

/// All runs in the engine together, for the status bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobSummary {
    pub runs: usize,
    pub done: u64,
    pub total: u64,
    pub fraction: f32,
    /// Roughly the time left, every run together ([`runs_progress`]).
    pub eta_s: Option<f64>,
    /// Images an hour, the servers working together at their latest speed.
    pub per_hour: f64,
}

impl AppState {
    pub fn run_progress(&self, run: &Id) -> Option<RunProgress> {
        let (p, r) = self.index.run(run)?;
        let live: BTreeSet<u64> = r.live_seeds().map(|(_, s)| s).collect();
        let done = p.outputs.iter().filter(|o| &o.name.run == run && live.contains(&o.name.seed)).count() as u64;
        let mut sampling: Vec<_> =
            self.progress.iter().filter(|((r2, _), _)| r2 == run).map(|((_, seed), (s, n))| (*seed, *s, *n)).collect();
        sampling.sort();
        let live_steps: Vec<f64> =
            self.sampling_times.iter().filter(|((r2, _), _)| r2 == run).filter_map(|(_, t)| t.step_s()).collect();
        Some(RunProgress {
            run: run.clone(),
            project_id: p.project.id.clone(),
            project: p.name(),
            sweep: r.sweep.as_ref().map(|s| (s.name.clone(), s.row())),
            label: r.label.clone(),
            prompt: r.prompt.clone(),
            done,
            total: live.len() as u64,
            sampling,
            timing: crate::timing::Timing::of_run(r),
            step_s: (!live_steps.is_empty()).then(|| live_steps.iter().sum::<f64>() / live_steps.len() as f64),
            steps: r.sample.steps,
        })
    }

    /// A project's most recent run (by ID: creation order).
    pub fn latest_run<'a>(&self, p: &'a ProjectEntry) -> Option<&'a increart_core::model::Run> {
        p.runs.iter().max_by(|a, b| a.id.cmp(&b.id))
    }

    /// Runs in the engine, in queue order (oldest first).
    pub fn jobs(&self) -> Vec<RunProgress> {
        self.active_runs.iter().filter_map(|r| self.run_progress(r)).collect()
    }

    /// The runs in the engine by project, a sweep's together (Jobs):
    /// projects in the order their first run is in the queue, and in each,
    /// runs and sweeps in the order of their first run.
    pub fn run_groups(&self) -> Vec<RunGroup> {
        let mut groups: Vec<RunGroup> = vec![];
        for j in self.jobs() {
            if !groups.iter().any(|g| g.project == j.project_id) {
                groups.push(RunGroup { project: j.project_id.clone(), name: j.project.clone(), items: vec![] });
            }
            let Some(g) = groups.iter_mut().find(|g| g.project == j.project_id) else { continue };
            let sweep = j.sweep.as_ref().map(|(s, _)| s.clone());
            match sweep {
                Some(name) => {
                    match g.items.iter_mut().find(|i| matches!(i, RunItem::Sweep { name: n, .. } if *n == name)) {
                        Some(RunItem::Sweep { runs, .. }) => runs.push(j),
                        _ => g.items.push(RunItem::Sweep { name, runs: vec![j] }),
                    }
                }
                None => g.items.push(RunItem::Run(Box::new(j))),
            }
        }
        groups
    }

    pub fn job_summary(&self) -> JobSummary {
        let jobs = self.jobs();
        let (done, total, fraction, eta_s) = runs_progress(&jobs.iter().collect::<Vec<_>>(), self.working_servers());
        let eta_s = self.left_of(jobs.iter().map(|j| &j.run)).or(eta_s);
        let per_hour = self
            .servers
            .iter()
            .filter(|s| server_working(&s.state))
            .filter_map(|s| self.server_speed(&s.name))
            .map(|s| s.per_hour())
            .fold(0.0, |a, b| a + b);
        JobSummary { runs: jobs.len(), done, total, fraction, eta_s, per_hour }
    }

    /// The servers that are working or about to (not connecting or
    /// unreachable), at least 1: how many images are made at once.
    pub fn working_servers(&self) -> usize {
        self.servers.iter().filter(|s| server_working(&s.state)).count().max(1)
    }

    /// Seconds per image of this shape on each server (by its device): a
    /// run's own images there, else images like it there, else the run's
    /// own anywhere (or its live seconds per step), else the library's
    /// nearest, else [`crate::forecast::Shape::rough_image_s`].
    fn image_s_on(
        &self,
        shape: &crate::forecast::Shape,
        run: Option<&increart_core::model::Run>,
        live: Option<f64>,
    ) -> Vec<f64> {
        let mean = |v: Vec<f64>| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
        self.servers
            .iter()
            .map(|s| {
                let own = run.and_then(|r| {
                    mean(r.timings.iter().filter(|t| t.device == s.device).map(|t| t.elapsed_s as f64).collect())
                });
                let like = self.rates.image_s(shape, &s.device);
                own.or(like.filter(|(_, exact)| *exact).map(|(e, _)| e))
                    .or(live)
                    .or(like.map(|(e, _)| e))
                    .unwrap_or_else(|| shape.rough_image_s())
            })
            .collect()
    }

    /// The runs in the engine as the simulation takes them.
    fn sim_runs(&self) -> Vec<crate::forecast::SimRun> {
        let mut runs = vec![];
        for j in self.jobs() {
            let Some((_, run)) = self.index.run(&j.run) else { continue };
            let group = match &run.sweep {
                Some(s) => format!("{}/{}", j.project_id, s.name),
                None => j.run.to_string(),
            };
            runs.push(crate::forecast::SimRun {
                id: j.run.clone(),
                group,
                recipe: run.model.recipe.clone(),
                seeds: (j.total.saturating_sub(j.done) as usize).saturating_sub(j.sampling.len()),
                sampling: j.sampling.iter().map(|(_, s, n)| *s as f64 / (*n).max(1) as f64).collect(),
                image_s: self.image_s_on(&crate::forecast::Shape::of(run), Some(run), j.image_s()),
            });
        }
        runs
    }

    fn sim_servers(&self, runs: &[crate::forecast::SimRun]) -> Vec<crate::forecast::SimServer> {
        let recipes: BTreeSet<&str> = runs.iter().map(|r| r.recipe.as_str()).collect();
        self.servers
            .iter()
            .map(|s| crate::forecast::SimServer {
                working: server_working(&s.state),
                recipe: s.model.clone(),
                load_s: recipes
                    .iter()
                    .filter_map(|r| Some((r.to_string(), self.load_times.get(&s.name, r)? as f64)))
                    .collect(),
            })
            .collect()
    }

    /// The runs in the engine on a simulated clock (DESIGN.md, Timing).
    pub fn make_plan(&self) -> Option<crate::forecast::Plan> {
        let runs = self.sim_runs();
        Some(crate::forecast::simulate(&runs, &self.sim_servers(&runs), self.config.job_order))
    }

    /// Seconds per image of this shape on the servers working (their mean;
    /// else every server's, else any device's), and what that's from.
    pub fn image_s_for(&self, shape: &crate::forecast::Shape) -> (f64, Basis) {
        let mut devices: Vec<&str> =
            self.servers.iter().filter(|s| server_working(&s.state)).map(|s| s.device.as_str()).collect();
        if devices.is_empty() {
            devices = self.servers.iter().map(|s| s.device.as_str()).collect();
        }
        if devices.is_empty() {
            devices.push("");
        }
        let found: Vec<(f64, bool)> = devices.iter().filter_map(|d| self.rates.image_s(shape, d)).collect();
        if found.len() < devices.len() || found.is_empty() {
            return (shape.rough_image_s(), Basis::Rough);
        }
        let mean = found.iter().map(|(e, _)| e).sum::<f64>() / found.len() as f64;
        (mean, if found.iter().all(|(_, exact)| *exact) { Basis::Like } else { Basis::Scaled })
    }

    /// How long runs not queued yet would take (the Generate form): each
    /// (shape, seeds), a sweep's in one group. `None` with no server
    /// working.
    pub fn estimate_new(&self, new: &[(crate::forecast::Shape, u64)], sweep: bool) -> Option<NewEstimate> {
        use crate::forecast::{SimRun, simulate};
        let at = chrono::DateTime::from_timestamp(4_000_000_000, 0)?;
        let runs: Vec<SimRun> = new
            .iter()
            .enumerate()
            .map(|(i, (shape, seeds))| {
                let id = Id::new(at, i as u32);
                SimRun {
                    group: if sweep { "new sweep".into() } else { id.to_string() },
                    id,
                    recipe: shape.recipe.clone(),
                    seeds: *seeds as usize,
                    sampling: vec![],
                    image_s: self.image_s_on(shape, None, None),
                }
            })
            .collect();
        let servers = self.sim_servers(&runs);
        if !servers.iter().any(|s| s.working) {
            return None;
        }
        let ends = |plan: &crate::forecast::Plan| {
            runs.iter().map(|r| plan.runs.get(&r.id).and_then(|p| p.1)).try_fold(0.0f64, |m, f| f.map(|f| m.max(f)))
        };
        let alone = ends(&simulate(&runs, &servers, self.config.job_order))?;
        let mut all = self.sim_runs();
        let queued = !all.is_empty();
        all.extend(runs.iter().cloned());
        let after = if queued { ends(&simulate(&all, &self.sim_servers(&all), self.config.job_order)) } else { None };
        Some(NewEstimate { alone_s: alone, with_queue_s: after, servers: servers.iter().filter(|s| s.working).count() })
    }

    /// What the forecast is made of, hashed: the runs in the engine, their
    /// seeds done and whether they've a live speed yet, the servers' states
    /// and models, and the order.
    pub fn plan_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for j in self.jobs() {
            (j.run, j.done, j.total, j.step_s.is_some()).hash(&mut h);
        }
        for s in &self.servers {
            (&s.state, &s.model).hash(&mut h);
        }
        format!("{:?}", self.config.job_order).hash(&mut h);
        h.finish()
    }

    /// Seconds until a run in the engine is done, by the forecast.
    pub fn run_left(&self, run: &Id) -> Option<f64> {
        self.forecast.runs.get(run)?.1.map(|f| (f - self.now).max(0.0))
    }

    /// Seconds until a run in the engine starts generating, when it hasn't
    /// yet.
    pub fn run_starts_in(&self, run: &Id) -> Option<f64> {
        let p = self.run_progress(run)?;
        if p.done > 0 || !p.sampling.is_empty() {
            return None;
        }
        self.forecast.runs.get(run)?.0.map(|s| (s - self.now).max(0.0))
    }

    /// Seconds until the last of these runs is done; `None` when one
    /// isn't in the forecast.
    pub fn left_of<'a>(&self, runs: impl IntoIterator<Item = &'a Id>) -> Option<f64> {
        let left: Option<Vec<f64>> = runs.into_iter().map(|r| self.run_left(r)).collect();
        left.filter(|l| !l.is_empty()).map(|l| l.into_iter().fold(0.0, f64::max))
    }

    /// [`runs_progress`] of these runs, with the forecast's time left when
    /// it has one for each.
    pub fn group_progress(&self, runs: &[&RunProgress]) -> (u64, u64, f32, Option<f64>) {
        let (done, total, fraction, eta) = runs_progress(runs, self.working_servers());
        (done, total, fraction, self.left_of(runs.iter().map(|r| &r.run)).or(eta))
    }

    /// A server's speed over its latest images this session.
    pub fn server_speed(&self, name: &str) -> Option<crate::timing::Speed> {
        let recent: Vec<_> = self.server_speeds.get(name)?.iter().copied().collect();
        crate::timing::Speed::of(&recent)
    }
}

/// Whether a server in this state makes images or is about to (ready,
/// warming up, loading, draining).
fn server_working(state: &str) -> bool {
    !(state == "connecting" || state == "opening" || state.contains("unreachable") || state.contains("failed"))
}

/// Newest created first, last run first, or by name (any case); newest
/// created first among equals.
fn sort_projects(v: &mut [&ProjectEntry], order: TreeOrder) {
    v.sort_by(|a, b| b.project.id.cmp(&a.project.id));
    match order {
        TreeOrder::Created => {}
        TreeOrder::LastRun => v.sort_by_cached_key(|p| std::cmp::Reverse(p.last_run())),
        TreeOrder::Name => v.sort_by_cached_key(|p| p.name().to_lowercase()),
    }
}

/// (good, neutral, bad, unreviewed) of a project.
pub fn counts(p: &ProjectEntry) -> [usize; 4] {
    let mut c = [0; 4];
    for o in &p.outputs {
        c[match o.verdict {
            Some(Verdict::Good) => 0,
            Some(Verdict::Neutral) => 1,
            Some(Verdict::Bad) => 2,
            None => 3,
        }] += 1;
    }
    c
}

/// The images a project generates from (references and start images): its
/// Generate draft's list, then its runs', newest run first, each once.
pub fn project_inputs(p: &ProjectEntry) -> Vec<ImageRef> {
    let mut seen = std::collections::HashSet::new();
    let draft = p.draft.iter().flat_map(|d| {
        let listed = d.images.iter().map(|i| i.image.clone());
        listed.chain(d.refs.iter().cloned()).chain(d.init.clone())
    });
    let runs = p.runs.iter().rev().flat_map(|r| {
        let init = match &r.mode {
            increart_core::model::Mode::Edit { init, .. } => init.clone(),
            increart_core::model::Mode::Generate => None,
        };
        r.refs.iter().cloned().chain(init)
    });
    draft.chain(runs).filter(|i| seen.insert(i.clone())).collect()
}

/// A project's cover: the chosen cover, else its best image (good > neutral
/// > unreviewed > bad, most recent among equals).
pub fn cover(p: &ProjectEntry) -> Option<ImageRef> {
    if let Some(c) = &p.project.cover {
        return Some(c.clone());
    }
    let rank = |v: Option<Verdict>| match v {
        Some(Verdict::Good) => 3,
        Some(Verdict::Neutral) => 2,
        None => 1,
        Some(Verdict::Bad) => 0,
    };
    p.outputs
        .iter()
        .max_by(|a, b| (rank(a.verdict), &a.image.id).cmp(&(rank(b.verdict), &b.image.id)))
        .map(|o| o.image.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_snap_to_32() {
        assert_eq!([1000, 1008, 1024, 1039, 1040, 5, 0].map(round32), [992, 1024, 1024, 1024, 1056, 32, 32]);
        // the start image's aspect, within 256…2048, in multiples of 32
        assert_eq!(fit_size(1344, 896), (1344, 896));
        assert_eq!(fit_size(1000, 750), (992, 736));
        assert_eq!(fit_size(4000, 3000), (2048, 1536));
        assert_eq!(fit_size(100, 50), (512, 256));
        assert_eq!(fit_size(3000, 200), (2048, 256));
    }

    #[test]
    fn settings_changes_name_what_differs() {
        let mut config = Config::default();
        config.normalize();
        let t0 = increart_core::id::now();
        let img = |n: u32| GenerateImage { image: ImageRef::input(Id::new(t0, n)), reference: true, start: false };
        let mut a = GenerateForm::new(&config);
        a.images = vec![img(1), img(2)];
        let a = a.to_draft(&config);
        let mut b = a.clone();
        b.label = "r09".into();
        b.first_seed = 99;
        b.prompt = "other".into();
        assert!(settings_changes(&a, &b).is_empty(), "label, seed and prompt aren't settings");
        b.images = vec![img(2), img(1)];
        assert_eq!(settings_changes(&a, &b), ["image order"]);
        b.images = vec![img(1), img(3), img(4)];
        b.width = 512;
        assert_eq!(settings_changes(&a, &b), [format!("512×{}", a.height), "+2 images".into(), "−1 image".into()]);
    }

    #[test]
    fn history_rows_group_identical_texts_in_a_row() {
        let e = |kind, text: &str| HistoryEntry::text(increart_core::id::now(), kind, text.into(), "".into());
        let q = |l: &str| HistoryKind::Queued(l.into());
        let mut v = HistoryView {
            // newest first
            entries: vec![e(q("r03"), "fox"), e(q("r02"), "fox"), e(HistoryKind::Edit, "fox"), e(q("r01"), "cat")],
            selected: 1,
            ..HistoryView::new(HistoryKey::NewIdea)
        };
        let rows = v.shown();
        assert_eq!(rows.iter().map(|r| r.indexes.clone()).collect::<Vec<_>>(), [vec![0, 1, 2], vec![3]]);
        assert_eq!(rows[0].index, 0, "the newest entry is the row's");
        assert_eq!(rows[0].kinds, [&HistoryKind::Edit, &q("r02"), &q("r03")], "oldest first");
        v.every_entry = true;
        assert_eq!(v.shown().len(), 4);
    }

    #[test]
    fn scaling_keeps_the_aspect() {
        let g = (256, 2048);
        // 3:2 to 1 MP: 1224.7 × 816.5 → 1216 × 832 (the nearest multiples of 32)
        assert_eq!(scaled(1344, 896, ScaleBy::Megapixels, 1.0, g), (1216, 832));
        assert_eq!(scaled(1024, 1024, ScaleBy::Megapixels, 2.0, g), (1408, 1408));
        assert_eq!(scaled(1344, 896, ScaleBy::Width, 1920.0, g), (1920, 1280));
        assert_eq!(scaled(1344, 896, ScaleBy::Height, 1000.0, g), (1504, 992));
        // within the limits
        assert_eq!(scaled(1024, 1024, ScaleBy::Megapixels, 50.0, g), (2048, 2048));
        let d = ScaleDialog::new(ScaleTarget::Generate, 1344, 896);
        assert_eq!((d.megapixels, d.side, d.result()), (1.2, 1344, (1344, 896)));
    }
}

#[cfg(test)]
mod snooze_tests {
    use super::parse_snooze;

    #[test]
    fn snooze_times_typed_in() {
        for (text, min) in [
            ("45", 45),
            (" 45m ", 45),
            ("45 min", 45),
            ("2h", 120),
            ("1.5h", 90),
            ("1h30", 90),
            ("1h 30m", 90),
            ("2 hours", 120),
            ("1:30", 90),
            ("0:05", 5),
            ("24h", 1440),
        ] {
            assert_eq!(parse_snooze(text), Ok(min), "{text}");
        }
        for text in ["", "abc", "0", "0.2", "25h", "1:75", "5 sec", "-3", "1h x"] {
            assert!(parse_snooze(text).is_err(), "{text}");
        }
    }
}
