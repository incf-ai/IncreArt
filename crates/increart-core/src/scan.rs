//! Reading the whole library into an [`Index`]. Problems with single files
//! (unparsable RSN, a stray file, a broken link) are collected, not fatal.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{PathParts, Result};
use crate::fsutil;
use crate::link;
use crate::model::{DiscardedProject, GenerateDraft, Idea, IdeaDraftFile, ImageRef, OutputName, Project, Run, Store};
use crate::rsnfmt;

pub const IDEA_FILE: &str = "idea.rsn";
pub const PROJECT_FILE: &str = "project.rsn";
pub const RUN_FILE: &str = "run.rsn";
/// A project's autosaved Generate form.
pub const DRAFT_FILE: &str = "draft.rsn";
/// The autosaved New idea form.
pub const IDEA_DRAFT: &str = "input/draft.rsn";

pub const INBOX_DROP: &str = "input/inbox_drop";
pub const IMPORTED: &str = "input/inbox_drop/imported";
pub const UNREVIEWED_IDEAS: &str = "input/unreviewed";
pub const INPUT_LIBRARY: &str = "input/library";
pub const DISCARDED_IDEAS: &str = "input/discarded";
/// Cropped and marked copies (Crop / mark…), out of the input library's
/// lists unless asked for.
pub const EDITED_IDEAS: &str = "input/edited";
pub const PROJECTS: &str = "projects";
/// Discarded projects, each with a [`DISCARDED_FILE`]; restorable.
pub const DISCARDED_PROJECTS: &str = "discarded_projects";
pub const DISCARDED_FILE: &str = "discarded.rsn";
/// A project's discarded output links, under their rating folders.
pub const DISCARDED_OUTPUTS: &str = "discarded";
pub const COLLECTIONS: &str = "collections";

/// Where an idea lives (the folder is the verdict).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IdeaLoc {
    /// The Idea inbox, `input/unreviewed/`.
    Unreviewed,
    /// A folder of the input library, relative to `input/library/` (empty =
    /// top level).
    Library(PathBuf),
    Discarded,
    /// A cropped and marked copy, `input/edited/`.
    Edited,
}

impl IdeaLoc {
    pub fn top_library() -> IdeaLoc {
        IdeaLoc::Library(PathBuf::new())
    }

    /// The directory holding idea folders, relative to the library root.
    pub fn dir(&self) -> PathBuf {
        match self {
            IdeaLoc::Unreviewed => PathBuf::from(UNREVIEWED_IDEAS),
            IdeaLoc::Library(folder) => Path::new(INPUT_LIBRARY).join(folder),
            IdeaLoc::Discarded => PathBuf::from(DISCARDED_IDEAS),
            IdeaLoc::Edited => PathBuf::from(EDITED_IDEAS),
        }
    }
}

/// An output review verdict, also the `reviewed/<verdict>/` folder name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Verdict {
    Good,
    Neutral,
    Bad,
}

impl Verdict {
    pub const ALL: [Verdict; 3] = [Verdict::Good, Verdict::Neutral, Verdict::Bad];

    pub fn dir(self) -> &'static str {
        match self {
            Verdict::Good => "good",
            Verdict::Neutral => "neutral",
            Verdict::Bad => "bad",
        }
    }

    /// The folder for an output with this verdict (`None` = unreviewed),
    /// relative to the project.
    pub fn output_dir(v: Option<Verdict>) -> PathBuf {
        match v {
            None => PathBuf::from("unreviewed"),
            Some(v) => Path::new("reviewed").join(v.dir()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IdeaEntry {
    pub idea: Idea,
    pub loc: IdeaLoc,
    /// Relative to the library root.
    pub dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectEntry {
    pub project: Project,
    /// Relative to the library root (`projects/…`).
    pub dir: PathBuf,
    pub runs: Vec<Run>,
    pub outputs: Vec<OutputLink>,
    /// The autosaved Generate form, if any.
    pub draft: Option<GenerateDraft>,
    /// Its discarded output links (`discarded/`), each with the rating it
    /// had; not in `outputs`.
    pub discarded: Vec<OutputLink>,
}

/// A project in `discarded_projects/`.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscardedEntry {
    pub entry: ProjectEntry,
    pub discarded: DiscardedProject,
}

impl ProjectEntry {
    /// The project's name: its directory name.
    pub fn name(&self) -> String {
        self.dir.file_name().unwrap_or_default().to_string_lossy().into_owned()
    }

    /// When it last ran: its newest revision (seeds queued), else when it
    /// was created.
    pub fn last_run(&self) -> crate::id::Timestamp {
        let revs = self.runs.iter().flat_map(|r| &r.revisions).map(|r| r.created);
        revs.chain([self.project.created]).max().unwrap_or(self.project.created)
    }

    /// Seeds of one of its runs still to generate: live seeds with no output
    /// link and no timing (a generated seed whose link was deleted, or lost
    /// unzipping, isn't generated again). As
    /// [`crate::Library::pending_seeds`], from what was scanned (no reading
    /// the disk again).
    pub fn pending_seeds<'a>(&self, run: &'a Run) -> impl Iterator<Item = (u32, u64)> + 'a {
        let mut done: std::collections::HashSet<u64> =
            self.outputs.iter().filter(|o| o.name.run == run.id).map(|o| o.name.seed).collect();
        done.extend(run.timings.iter().map(|t| t.seed));
        run.live_seeds().filter(move |(_, s)| !done.contains(s))
    }

    /// Seeds of one of its runs generated, not deleted, and with no output
    /// link: links lost (an unzip that skipped symlinks, a file manager).
    pub fn missing_outputs<'a>(&self, run: &'a Run) -> impl Iterator<Item = (u32, u64)> + 'a {
        let linked: std::collections::HashSet<u64> =
            self.outputs.iter().filter(|o| o.name.run == run.id).map(|o| o.name.seed).collect();
        run.generated_seeds().filter(move |(_, s)| !linked.contains(s))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputLink {
    /// Relative to the library root.
    pub path: PathBuf,
    pub name: OutputName,
    pub image: ImageRef,
    pub verdict: Option<Verdict>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CollectionLink {
    /// Relative to the library root.
    pub path: PathBuf,
    /// Relative to `collections/`.
    pub folder: PathBuf,
    pub image: ImageRef,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Index {
    pub ideas: Vec<IdeaEntry>,
    pub projects: Vec<ProjectEntry>,
    /// Folders of the project tree, relative to `projects/`.
    pub project_folders: Vec<PathBuf>,
    /// Discarded projects (`discarded_projects/`), newest first; not in
    /// `projects`, so nothing generates, reviews or shows them elsewhere.
    pub discarded_projects: Vec<DiscardedEntry>,
    /// Folders of the input library, relative to `input/library/`.
    pub library_folders: Vec<PathBuf>,
    /// Collection folders, relative to `collections/`.
    pub collection_folders: Vec<PathBuf>,
    pub collection_links: Vec<CollectionLink>,
    /// Files waiting in `input/inbox_drop/` (relative to the library root).
    pub inbox_drop: Vec<PathBuf>,
    /// The autosaved New idea form, if any.
    pub idea_draft: Option<IdeaDraftFile>,
    /// Transparent store images, from the pixel index (filled by
    /// [`Library::scan`](crate::Library::scan)).
    pub transparent: BTreeSet<ImageRef>,
    pub problems: Vec<String>,
}

impl Index {
    /// A line per project whose generated images have lost output links
    /// ([`ProjectEntry::missing_outputs`]).
    pub fn missing_outputs(&self) -> Vec<String> {
        self.projects
            .iter()
            .filter_map(|p| {
                let n: usize = p.runs.iter().map(|r| p.missing_outputs(r).count()).sum();
                (n > 0).then(|| {
                    format!(
                        "{}: {n} generated image(s) have no output link: links lost unzipping? \
                         (unzip it again with a tool that keeps symlinks, or `unzip`)",
                        p.dir.display()
                    )
                })
            })
            .collect()
    }

    /// Seeds still to generate, in every run.
    pub fn pending_seeds(&self) -> usize {
        self.projects.iter().map(|p| p.runs.iter().map(|r| p.pending_seeds(r).count()).sum::<usize>()).sum()
    }

    pub fn idea(&self, id: &crate::Id) -> Option<&IdeaEntry> {
        self.ideas.iter().find(|e| &e.idea.id == id)
    }

    pub fn project(&self, id: &crate::Id) -> Option<&ProjectEntry> {
        self.projects.iter().find(|e| &e.project.id == id)
    }

    /// The project holding a run, and the run.
    pub fn run(&self, id: &crate::Id) -> Option<(&ProjectEntry, &Run)> {
        self.projects.iter().find_map(|p| p.runs.iter().find(|r| &r.id == id).map(|r| (p, r)))
    }

    pub fn transparent(&self, r: &ImageRef) -> bool {
        self.transparent.contains(r)
    }
}

pub fn scan(root: &Path) -> Result<Index> {
    let mut ix = Index::default();
    for loc in [IdeaLoc::Unreviewed, IdeaLoc::Discarded, IdeaLoc::Edited] {
        for dir in fsutil::subdirs(&root.join(loc.dir()))? {
            read_idea(root, &dir, loc.clone(), &mut ix);
        }
    }
    scan_library(root, Path::new(""), &mut ix)?;
    scan_projects(root, Path::new(""), &mut ix)?;
    scan_discarded_projects(root, &mut ix)?;
    scan_collections(root, &mut ix)?;
    let idea_draft = root.join(IDEA_DRAFT);
    if idea_draft.is_file() {
        match rsnfmt::read::<IdeaDraftFile>(&idea_draft) {
            Ok(d) => ix.idea_draft = Some(d),
            Err(e) => ix.problems.push(e.to_string()),
        }
    }
    let drop = root.join(INBOX_DROP);
    if drop.is_dir() {
        for e in fs::read_dir(&drop).map_err(|e| crate::Error::Io { path: drop.clone(), source: e })? {
            let Ok(e) = e else { continue };
            let p = e.path();
            if !p.is_dir() && !fsutil::hidden(&p) {
                ix.inbox_drop.push(rel(root, &p));
            }
        }
        ix.inbox_drop.sort();
    }
    Ok(ix)
}

fn rel(root: &Path, p: &Path) -> PathBuf {
    p.strip_prefix(root).unwrap_or(p).to_path_buf()
}

fn read_idea(root: &Path, dir: &Path, loc: IdeaLoc, ix: &mut Index) {
    let file = dir.join(IDEA_FILE);
    match rsnfmt::read::<Idea>(&file) {
        Ok(idea) => {
            if dir.file_name().is_some_and(|n| n.to_string_lossy() != idea.id.as_str()) {
                ix.problems.push(format!("{}: directory name is not the idea id {}", dir.display(), idea.id));
            }
            ix.ideas.push(IdeaEntry { idea, loc, dir: rel(root, dir) });
        }
        Err(e) => ix.problems.push(e.to_string()),
    }
}

fn scan_library(root: &Path, folder: &Path, ix: &mut Index) -> Result<()> {
    for dir in fsutil::subdirs(&root.join(INPUT_LIBRARY).join(folder))? {
        let name = dir.name()?;
        if dir.join(IDEA_FILE).exists() {
            read_idea(root, &dir, IdeaLoc::Library(folder.to_path_buf()), ix);
        } else {
            let sub = folder.join(name);
            ix.library_folders.push(sub.clone());
            scan_library(root, &sub, ix)?;
        }
    }
    Ok(())
}

fn scan_projects(root: &Path, folder: &Path, ix: &mut Index) -> Result<()> {
    for dir in fsutil::subdirs(&root.join(PROJECTS).join(folder))? {
        let name = dir.name()?;
        if dir.join(PROJECT_FILE).exists() {
            read_project(root, &dir, ix)?;
        } else {
            let sub = folder.join(name);
            ix.project_folders.push(sub.clone());
            scan_projects(root, &sub, ix)?;
        }
    }
    Ok(())
}

fn scan_discarded_projects(root: &Path, ix: &mut Index) -> Result<()> {
    for dir in fsutil::subdirs(&root.join(DISCARDED_PROJECTS))? {
        if !dir.join(PROJECT_FILE).exists() {
            continue;
        }
        // read as a project, then kept apart
        let mut one = Index::default();
        read_project(root, &dir, &mut one)?;
        ix.problems.append(&mut one.problems);
        let Some(entry) = one.projects.pop() else { continue };
        let discarded = rsnfmt::read::<DiscardedProject>(&dir.join(DISCARDED_FILE)).unwrap_or_else(|_| {
            DiscardedProject { name: entry.name(), from: PathBuf::new(), discarded: entry.project.created }
        });
        ix.discarded_projects.push(DiscardedEntry { entry, discarded });
    }
    ix.discarded_projects.sort_by_key(|d| std::cmp::Reverse(d.discarded.discarded));
    Ok(())
}

fn read_project(root: &Path, dir: &Path, ix: &mut Index) -> Result<()> {
    let project = match rsnfmt::read::<Project>(&dir.join(PROJECT_FILE)) {
        Ok(p) => p,
        Err(e) => {
            ix.problems.push(e.to_string());
            return Ok(());
        }
    };
    let mut runs = Vec::new();
    for run_dir in fsutil::subdirs(&dir.join("runs"))? {
        match rsnfmt::read::<Run>(&run_dir.join(RUN_FILE)) {
            Ok(r) => runs.push(r),
            Err(e) => ix.problems.push(e.to_string()),
        }
    }
    runs.sort_by(|a, b| a.id.cmp(&b.id));
    let mut outputs = Vec::new();
    let mut discarded = Vec::new();
    let verdicts = std::iter::once(None).chain(Verdict::ALL.map(Some));
    let places = verdicts.flat_map(|v| [(v, false), (v, true)]);
    for (verdict, gone) in places {
        let base = if gone { dir.join(DISCARDED_OUTPUTS) } else { dir.to_path_buf() };
        let folder = base.join(Verdict::output_dir(verdict));
        let outputs = if gone { &mut discarded } else { &mut outputs };
        let Ok(rd) = fs::read_dir(&folder) else { continue };
        let mut files: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        files.sort();
        for p in files {
            if fsutil::hidden(&p) || p.is_dir() {
                continue;
            }
            let file_name = p.name()?.to_string_lossy();
            let Some(name) = OutputName::parse(&file_name) else {
                ix.problems.push(format!("{}: not an output link name", p.display()));
                continue;
            };
            match link::read_ref(&p, Store::Output) {
                Ok(Some(image)) => outputs.push(OutputLink { path: rel(root, &p), name, image, verdict }),
                Ok(None) => ix.problems.push(format!("{}: not a link into a store", p.display())),
                Err(e) => ix.problems.push(e.to_string()),
            }
        }
    }
    let draft_file = dir.join(DRAFT_FILE);
    let draft = draft_file.is_file().then(|| rsnfmt::read::<GenerateDraft>(&draft_file)).and_then(|r| match r {
        Ok(d) => Some(d),
        Err(e) => {
            ix.problems.push(e.to_string());
            None
        }
    });
    ix.projects.push(ProjectEntry { project, dir: rel(root, dir), runs, outputs, draft, discarded });
    Ok(())
}

fn scan_collections(root: &Path, ix: &mut Index) -> Result<()> {
    let base = root.join(COLLECTIONS);
    for p in fsutil::walk(&base)? {
        let folder = p.parent_dir()?.relative_to(&base)?.to_path_buf();
        if fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
            ix.collection_folders.push(p.relative_to(&base)?.to_path_buf());
        } else if !fsutil::hidden(&p) {
            match link::read_ref(&p, Store::Output) {
                Ok(Some(image)) => ix.collection_links.push(CollectionLink { path: rel(root, &p), folder, image }),
                Ok(None) => ix.problems.push(format!("{}: not a link into a store", p.display())),
                Err(e) => ix.problems.push(e.to_string()),
            }
        }
    }
    Ok(())
}
