//! A whole-library check (`increart --check-library`): what a scan finds,
//! plus what it takes on trust: every link reaches its store image, every
//! image a run or idea names is there, and every store image decodes to the
//! pixels its `increart:pixels` names. Generated images whose output link
//! is gone are problems; seeds still to generate are counted.

use crate::vfs as fs;
use crate::vfs::PathExt;
use std::collections::BTreeSet;
use std::path::Path;

use crate::error::Result;
use crate::model::{ImageRef, Mode, Store, StoreFormat};
use crate::{Library, container, import, link, png};

/// What [`check`] counted and found wrong.
#[derive(Debug, Default)]
pub struct Report {
    pub ideas: usize,
    pub projects: usize,
    pub runs: usize,
    pub outputs: usize,
    pub store_images: usize,
    pub links: usize,
    /// Seeds still to generate (queued when the engine starts).
    pub pending: usize,
    pub problems: Vec<String>,
}

impl Report {
    /// One line: the counts, then the problems or "no problems".
    pub fn summary(&self) -> String {
        let mut counts = format!(
            "{} ideas, {} projects, {} runs, {} outputs, {} store images, {} links",
            self.ideas, self.projects, self.runs, self.outputs, self.store_images, self.links
        );
        if self.pending > 0 {
            counts += &format!(", {} seeds to generate", self.pending);
        }
        match self.problems.len() {
            0 => format!("{counts}: no problems"),
            n => format!("{counts}: {n} problem(s)"),
        }
    }
}

/// Checks the open library `lib` (reads every image; writes nothing, though
/// opening it may have).
pub fn check(lib: &Library) -> Result<Report> {
    let ix = lib.scan()?;
    let mut r = Report { problems: lib.problems().to_vec(), ..Default::default() };
    r.problems.extend(ix.problems.iter().cloned());
    r.problems.extend(ix.missing_outputs());
    r.pending = ix.pending_seeds();
    let mut named: BTreeSet<ImageRef> = BTreeSet::new();

    // the links: each resolves (a broken symlink can't be read) to the store
    // image it names
    let mut links: Vec<(std::path::PathBuf, Store)> = vec![];
    for e in &ix.ideas {
        r.ideas += 1;
        named.extend(e.idea.images.iter().cloned().map(ImageRef::input));
        if let Ok(rd) = fs::read_dir(lib.path(&e.dir)) {
            for f in rd.flatten() {
                if f.file_name().to_str().and_then(StoreFormat::of_name).is_some() {
                    links.push((f.path(), Store::Input));
                }
            }
        }
    }
    for p in &ix.projects {
        r.projects += 1;
        for run in &p.runs {
            r.runs += 1;
            named.extend(run.refs.iter().cloned());
            if let Mode::Edit { init: Some(i), .. } = &run.mode {
                named.insert(i.clone());
            }
        }
        for o in &p.outputs {
            r.outputs += 1;
            links.push((lib.path(&o.path), Store::Output));
            named.insert(o.image.clone());
        }
    }
    for c in &ix.collection_links {
        links.push((lib.path(&c.path), Store::Output));
    }
    for (path, store) in &links {
        r.links += 1;
        let shown = path.strip_prefix(lib.root()).unwrap_or(path).display().to_string();
        let to = match link::read_ref(path, *store) {
            Ok(Some(to)) => to,
            Ok(None) => {
                r.problems.push(format!("{shown}: not a link into a store"));
                continue;
            }
            Err(e) => {
                r.problems.push(format!("{shown}: {e}"));
                continue;
            }
        };
        if let Err(e) = fs::metadata(path) {
            r.problems.push(format!("{shown}: can't be read ({e})"));
        } else if !lib.image_path(&to).vfs_is_file() {
            r.problems.push(format!("{shown}: its image {} is missing", to.id));
        }
    }
    for i in &named {
        if !lib.image_path(i).vfs_is_file() {
            r.problems.push(format!("{}: named in the library but missing", i.rel_path_as(lib.store_format())));
        }
    }

    // every store image: its pixels as recorded
    for store in [Store::Input, Store::Output] {
        let dir = lib.path(store.dir());
        for f in fs::read_dir(&dir).map(|rd| rd.flatten().collect::<Vec<_>>()).unwrap_or_default() {
            if f.file_name().to_str().and_then(StoreFormat::of_name).is_none() {
                continue;
            }
            r.store_images += 1;
            store_image(&f.path(), &mut r.problems);
        }
    }
    Ok(r)
}

fn store_image(path: &Path, problems: &mut Vec<String>) {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => return problems.push(format!("{name}: {e}")),
    };
    let pixels = match import::decode_pixels(&bytes) {
        Ok(p) => p,
        Err(e) => return problems.push(format!("{name}: doesn't decode ({e})")),
    };
    match container::text(&bytes, png::KEY_PIXELS) {
        Ok(Some(h)) if h == pixels.hash() => {}
        Ok(Some(_)) => problems.push(format!("{name}: its pixels aren't the ones recorded")),
        Ok(None) => problems.push(format!("{name}: no {}", png::KEY_PIXELS)),
        Err(e) => problems.push(format!("{name}: {e}")),
    }
}
