//! Links from folders into the image stores (DESIGN.md, Links): relative
//! symlinks, else hard links, else copies, fixed per library.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::container;
use crate::error::{Error, IoContext, PathParts, Result};
use crate::fsutil;
use crate::model::StoreFormat;
use crate::model::{ImageRef, LinkMode, Store};
use crate::png;

#[cfg(unix)]
pub fn symlink(target: &Path, at: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}

#[cfg(windows)]
pub fn symlink(target: &Path, at: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, at)
}

/// Picks the library's link mode by trying each kind in `dir` (inside the
/// library, so hard links stay on one volume). `INCREART_LINK_MODE`
/// (`symlink`, `hardlink` or `copy`) picks it instead: tests run every mode
/// this way, and it gives a library that copies cleanly to another drive.
pub fn probe(dir: &Path) -> Result<LinkMode> {
    if let Ok(m) = std::env::var("INCREART_LINK_MODE")
        && !m.is_empty()
    {
        return match m.to_ascii_lowercase().as_str() {
            "symlink" => Ok(LinkMode::Symlink),
            "hardlink" => Ok(LinkMode::Hardlink),
            "copy" => Ok(LinkMode::Copy),
            _ => Err(Error::Refused(format!("INCREART_LINK_MODE={m}: expected symlink, hardlink or copy"))),
        };
    }
    let probe = dir.join(".link-probe");
    fs::create_dir_all(&probe).at(&probe)?;
    let target = probe.join("target");
    fs::write(&target, b"probe").at(&target)?;
    let sym = probe.join("sym");
    let hard = probe.join("hard");
    let mode = if symlink(Path::new("target"), &sym).is_ok() && fs::read(&sym).is_ok_and(|b| b == b"probe") {
        LinkMode::Symlink
    } else if fs::hard_link(&target, &hard).is_ok() {
        LinkMode::Hardlink
    } else {
        LinkMode::Copy
    };
    fs::remove_dir_all(&probe).at(&probe)?;
    Ok(mode)
}

/// `target` as seen from `dir` (both absolute).
fn relative(target: &Path, dir: &Path) -> Result<PathBuf> {
    pathdiff::diff_paths(target, dir)
        .ok_or_else(|| Error::Path { path: target.to_path_buf(), what: "can't be linked to from a relative path" })
}

/// Creates a link at `at` to the store file `target` (absolute). Fails if
/// something exists at `at`.
pub fn create(mode: LinkMode, target: &Path, at: &Path) -> Result<()> {
    let dir = at.parent_dir()?;
    fs::create_dir_all(dir).at(dir)?;
    match mode {
        LinkMode::Symlink => {
            let rel = relative(target, dir)?;
            symlink(&rel, at).at(at)
        }
        LinkMode::Hardlink => fs::hard_link(target, at).at(at),
        LinkMode::Copy => {
            if fsutil::exists(at) {
                return Err(io::Error::from(io::ErrorKind::AlreadyExists)).at(at);
            }
            fs::copy(target, at).map(drop).at(at)
        }
    }
}

/// The store image a symlink points at, from its target text (the last two
/// components, `<store>_images/<id>.png` or `.webp`), without following it.
pub fn symlink_ref(at: &Path) -> Option<ImageRef> {
    let target = fs::read_link(at).ok()?;
    let mut comps = target.components().rev();
    let file = comps.next()?;
    let store = comps.next()?;
    let (Component::Normal(file), Component::Normal(store)) = (file, store) else {
        return None;
    };
    let id = StoreFormat::of_name(file.to_str()?)?.1.parse().ok()?;
    Some(ImageRef { id, store: Store::from_dir(store.to_str()?)? })
}

/// The store image a link refers to: from the target path for a symlink,
/// from `increart:id` for a hard link or copy (whose store is `store`, as
/// known from where the link lives). `None` for a file that isn't a link.
pub fn read_ref(at: &Path, store: Store) -> Result<Option<ImageRef>> {
    if fsutil::is_symlink(at) {
        return Ok(symlink_ref(at));
    }
    match container::header_text(at, png::KEY_ID) {
        Ok(Some(id)) => Ok(Some(ImageRef { id: id.parse()?, store })),
        Ok(None) | Err(Error::Format { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The store image a symlink's stand-in names: the small file git makes for
/// a symlink where it can't make one (Windows without Developer Mode),
/// holding just the target path, `../…/<store>_images/<id>.png`. `None`
/// for anything else, or when that image isn't in the store.
pub fn stand_in_ref(root: &Path, at: &Path) -> Option<ImageRef> {
    let meta = fs::symlink_metadata(at).ok()?;
    if !meta.is_file() || meta.len() > 512 {
        return None;
    }
    let text = fs::read_to_string(at).ok()?;
    let target = Path::new(text.trim_end());
    let mut comps = target.components();
    let (Some(Component::ParentDir), Some(Component::Normal(file)), Some(Component::Normal(store))) =
        (comps.next(), comps.next_back(), comps.next_back())
    else {
        return None;
    };
    if !comps.all(|c| c == Component::ParentDir) {
        return None;
    }
    let store = Store::from_dir(store.to_str()?)?;
    let id = StoreFormat::of_name(file.to_str()?)?.1.parse().ok()?;
    let real = root.join(store.dir()).join(file);
    // the path must land on the store from where the stand-in is
    let resolved = at.parent()?.join(target);
    let same = |a: &Path, b: &Path| fs::canonicalize(a).ok().is_some_and(|a| fs::canonicalize(b).is_ok_and(|b| a == b));
    same(&resolved, &real).then_some(ImageRef { id, store })
}

/// Replaces every symlink stand-in under `root` (outside the stores and
/// caches) with a link of kind `mode`. Returns how many.
pub fn replace_stand_ins(root: &Path, mode: LinkMode) -> Result<usize> {
    let mut n = 0;
    for top in fsutil::subdirs(root)? {
        let name = top.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if matches!(name, "input_images" | "output_images" | "thumbs" | "cache") {
            continue;
        }
        for p in fsutil::walk(&top)? {
            let is_image = p.file_name().and_then(|n| n.to_str()).and_then(StoreFormat::of_name).is_some();
            let Some(r) = is_image.then(|| stand_in_ref(root, &p)).flatten() else { continue };
            fs::remove_file(&p).at(&p)?;
            create(mode, &r.path(root), &p)?;
            debug_assert_eq!(read_ref(&p, r.store).ok().flatten(), Some(r));
            n += 1;
        }
    }
    Ok(n)
}

/// Recreates every symlink under `dir` after the directory was moved (their
/// relative targets are only right at the old depth). Other kinds of link
/// don't need it.
pub fn refresh_symlinks(root: &Path, dir: &Path) -> Result<()> {
    let paths = if fsutil::is_symlink(dir) { vec![dir.to_path_buf()] } else { fsutil::walk(dir)? };
    for p in paths {
        if !fsutil::is_symlink(&p) {
            continue;
        }
        if let Some(r) = symlink_ref(&p) {
            let target = r.path(root);
            let want = relative(&target, p.parent_dir()?)?;
            if fs::read_link(&p).at(&p)? != want {
                fs::remove_file(&p).at(&p)?;
                symlink(&want, &p).at(&p)?;
            }
        }
    }
    Ok(())
}

/// Moves a link or a directory holding links, then fixes symlink targets.
pub fn move_with_links(root: &Path, from: &Path, to: &Path) -> Result<()> {
    if fsutil::exists(to) {
        return Err(Error::Refused(format!("{} already exists", to.display())));
    }
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).at(dir)?;
    }
    fs::rename(from, to).at(from)?;
    refresh_symlinks(root, to)
}

/// Absolute form of a library path, for diffing.
pub fn absolute(p: &Path) -> Result<PathBuf> {
    std::path::absolute(p).at(p)
}
