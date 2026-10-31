//! Images for the GUI, loaded off the UI thread: thumbnails from the library's
//! `thumbs/` cache (made on demand) and larger images for Review.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use increart_core::sync::LockExt;
use increart_core::{ImageRef, thumbs};

/// Longest side of the images shown in Review.
const FULL: u32 = 1600;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    image: ImageRef,
    full: bool,
}

type Loaded = (Key, Result<ColorImage, String>);

pub struct Textures {
    loaded: HashMap<Key, Option<TextureHandle>>,
    /// Pixel sizes, from the PNG headers.
    dims: HashMap<ImageRef, Option<[u32; 2]>>,
    pending: HashSet<Key>,
    requests: Sender<(Key, PathBuf)>,
    results: Receiver<Loaded>,
}

impl Textures {
    pub fn new(ctx: &egui::Context) -> Textures {
        let (requests, jobs) = mpsc::channel::<(Key, PathBuf)>();
        let (done, results) = mpsc::channel::<Loaded>();
        let jobs = Arc::new(Mutex::new(jobs));
        for i in 0..3 {
            let (jobs, done, ctx) = (jobs.clone(), done.clone(), ctx.clone());
            std::thread::Builder::new()
                .name(format!("textures {i}"))
                .spawn(move || {
                    loop {
                        let Ok((key, root)) = jobs.lock_or_recover().recv() else { return };
                        let img = load(&root, &key);
                        if done.send((key, img)).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                })
                .expect("spawn texture loader");
        }
        Textures { loaded: HashMap::new(), dims: HashMap::new(), pending: HashSet::new(), requests, results }
    }

    /// Uploads finished loads; call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        for (key, img) in self.results.try_iter() {
            self.pending.remove(&key);
            let tex =
                img.ok().map(|i| ctx.load_texture(format!("{:?}{}", key.image, key.full), i, TextureOptions::LINEAR));
            self.loaded.insert(key, tex);
        }
    }

    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Drops everything (e.g. after another library was opened).
    pub fn clear(&mut self) {
        self.loaded.clear();
        self.dims.clear();
    }

    fn get(&mut self, root: &std::path::Path, image: &ImageRef, full: bool) -> Option<TextureHandle> {
        let key = Key { image: image.clone(), full };
        if let Some(t) = self.loaded.get(&key) {
            return t.clone();
        }
        if self.pending.insert(key.clone()) {
            let _ = self.requests.send((key, root.to_path_buf()));
        }
        None
    }

    pub fn thumb(&mut self, root: &std::path::Path, image: &ImageRef) -> Option<TextureHandle> {
        self.get(root, image, false)
    }

    /// The image's size in pixels (read from its header once, then cached).
    pub fn dims(&mut self, root: &std::path::Path, image: &ImageRef) -> Option<[u32; 2]> {
        *self
            .dims
            .entry(image.clone())
            .or_insert_with(|| image::image_dimensions(image.path(root)).ok().map(|(w, h)| [w, h]))
    }

    /// "W × H · aspect · N MP", or "" if the image can't be read.
    pub fn dims_text(&mut self, root: &std::path::Path, image: &ImageRef) -> String {
        self.dims(root, image).map(|[w, h]| crate::widgets::size_info(w, h)).unwrap_or_default()
    }

    pub fn full(&mut self, root: &std::path::Path, image: &ImageRef) -> Option<TextureHandle> {
        self.get(root, image, true)
    }
}

fn load(root: &std::path::Path, key: &Key) -> Result<ColorImage, String> {
    let img = if key.full {
        let img = image::open(key.image.path(root)).map_err(|e| e.to_string())?;
        if img.width().max(img.height()) > FULL { img.thumbnail(FULL, FULL) } else { img }
    } else {
        let path = thumbs::ensure(root, &key.image).map_err(|e| e.to_string())?;
        image::open(path).map_err(|e| e.to_string())?
    };
    let rgba = img.to_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}
