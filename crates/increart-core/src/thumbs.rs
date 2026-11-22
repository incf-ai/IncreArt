//! Thumbnails: `thumbs/{input,output}/<image-id>.webp`, a deletable cache
//! (DESIGN.md, Caching). Lossless WebP with alpha.

use crate::vfs::PathExt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fsutil;
use crate::model::{ImageRef, Store};

/// Longest side of a thumbnail.
pub const SIZE: u32 = 320;

pub fn path(root: &Path, r: &ImageRef) -> PathBuf {
    let dir = match r.store {
        Store::Input => "input",
        Store::Output => "output",
    };
    root.join("thumbs").join(dir).join(format!("{}.webp", r.id))
}

/// The thumbnail of a store image, made first if missing.
pub fn ensure(root: &Path, r: &ImageRef) -> Result<PathBuf> {
    let thumb = path(root, r);
    if thumb.vfs_is_file() {
        return Ok(thumb);
    }
    let src = r.path(root);
    let img = crate::vfs::open_image(&src).map_err(|e| Error::image(&src, e))?;
    let small = img.thumbnail(SIZE, SIZE).to_rgba8();
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(small.as_raw(), small.width(), small.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| Error::image(&thumb, e))?;
    fsutil::write_atomic(&thumb, &out)?;
    Ok(thumb)
}
