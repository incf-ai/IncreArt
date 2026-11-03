//! Images for the GUI, loaded off the UI thread: thumbnails from the library's
//! `thumbs/` cache (made on demand) and larger images for Review.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use increart_core::sync::LockExt;
use increart_core::{ImageRef, thumbs};

/// Longest side of the images shown in Review.
const FULL: u32 = 1600;

/// Quick tries of an image that fails to load, [`RETRY_AFTER`] apart;
/// then it's tried every [`RETRY_LATER`] (an image that can be read again
/// shows, one that can't costs a load now and then).
const TRIES: u32 = 3;

/// Wait before loading a failed image again, the first [`TRIES`] times.
const RETRY_AFTER: Duration = Duration::from_secs(2);

/// Wait between tries after those.
const RETRY_LATER: Duration = Duration::from_secs(30);

/// A load not back by then is asked for again (a read that hangs, a loader
/// that's gone); its answer, if it comes, is taken all the same.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    image: ImageRef,
    full: bool,
}

type Loaded = (Key, Result<ColorImage, String>);

/// Loads to do: full-size images first (the one on screen), then
/// thumbnails, each in the order asked.
#[derive(Default)]
struct Queue {
    jobs: Mutex<Jobs>,
    ready: Condvar,
}

#[derive(Default)]
struct Jobs {
    full: VecDeque<(Key, PathBuf)>,
    thumbs: VecDeque<(Key, PathBuf)>,
    closed: bool,
}

impl Queue {
    fn push(&self, key: Key, root: PathBuf) {
        let mut q = self.jobs.lock_or_recover();
        let jobs = if key.full { &mut q.full } else { &mut q.thumbs };
        jobs.push_back((key, root));
        self.ready.notify_one();
    }

    /// The next load, waiting for one; None once closed.
    fn pop(&self) -> Option<(Key, PathBuf)> {
        let mut q = self.jobs.lock_or_recover();
        loop {
            if q.closed {
                return None;
            }
            if let Some(job) = q.full.pop_front().or_else(|| q.thumbs.pop_front()) {
                return Some(job);
            }
            q = self.ready.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn close(&self) {
        self.jobs.lock_or_recover().closed = true;
        self.ready.notify_all();
    }
}

/// A load that failed: how many times, and when to try again.
struct Failed {
    tries: u32,
    at: Instant,
}

pub struct Textures {
    loaded: HashMap<Key, TextureHandle>,
    failed: HashMap<Key, Failed>,
    /// Pixel sizes, from the PNG headers.
    dims: HashMap<ImageRef, Option<[u32; 2]>>,
    /// Loads asked for, since when.
    pending: HashMap<Key, Instant>,
    queue: Arc<Queue>,
    results: Receiver<Loaded>,
    /// [`RETRY_AFTER`], [`RETRY_LATER`], [`LOAD_TIMEOUT`] (shorter in tests).
    waits: [Duration; 3],
    ctx: egui::Context,
}

impl Textures {
    pub fn new(ctx: &egui::Context) -> Textures {
        let queue = Arc::new(Queue::default());
        let (done, results) = mpsc::channel::<Loaded>();
        for i in 0..3 {
            let (queue, done, ctx): (_, Sender<Loaded>, _) = (queue.clone(), done.clone(), ctx.clone());
            let spawned = std::thread::Builder::new().name(format!("textures {i}")).spawn(move || {
                while let Some((key, root)) = queue.pop() {
                    // a panic is that image's failure, not the loader's end
                    let img = std::panic::catch_unwind(|| load(&root, &key))
                        .unwrap_or_else(|_| Err("the image decoder panicked".into()));
                    if done.send((key, img)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
            // (the others load them; none, and images stay blank)
            if let Err(e) = spawned {
                increart_core::applog::line("error", &format!("can't start texture loader {i}: {e}"));
            }
        }
        Textures {
            loaded: HashMap::new(),
            failed: HashMap::new(),
            dims: HashMap::new(),
            pending: HashMap::new(),
            queue,
            results,
            waits: [RETRY_AFTER, RETRY_LATER, LOAD_TIMEOUT],
            ctx: ctx.clone(),
        }
    }

    /// Uploads finished loads; call once per frame. A failed load is logged
    /// and tried again: after [`RETRY_AFTER`] the first [`TRIES`] times, then
    /// every [`RETRY_LATER`] (logged once more, as it starts).
    pub fn poll(&mut self, ctx: &egui::Context) {
        for (key, img) in self.results.try_iter() {
            self.pending.remove(&key);
            match img {
                Ok(i) => {
                    let tex = ctx.load_texture(format!("{:?}{}", key.image, key.full), i, TextureOptions::LINEAR);
                    self.failed.remove(&key);
                    self.loaded.insert(key, tex);
                }
                Err(e) => {
                    let f = self.failed.entry(key.clone()).or_insert(Failed { tries: 0, at: Instant::now() });
                    f.tries += 1;
                    let wait = if f.tries < TRIES { self.waits[0] } else { self.waits[1] };
                    f.at = Instant::now() + wait;
                    let what = if key.full { "image" } else { "thumbnail of" };
                    let id = &key.image.id;
                    if f.tries < TRIES {
                        let text = format!("can't load {what} {id} (try {} of {TRIES}, trying again): {e}", f.tries);
                        increart_core::applog::line("error", &text);
                    } else if f.tries == TRIES {
                        let text =
                            format!("can't load {what} {id} ({TRIES} tries; trying every {}s): {e}", wait.as_secs());
                        increart_core::applog::line("error", &text);
                    }
                    // (a frame then, to ask again if it's still shown)
                    ctx.request_repaint_after(wait);
                }
            }
        }
    }

    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Drops everything (e.g. after another library was opened).
    pub fn clear(&mut self) {
        self.loaded.clear();
        self.failed.clear();
        self.dims.clear();
    }

    fn get(&mut self, root: &std::path::Path, image: &ImageRef, full: bool) -> Option<TextureHandle> {
        let key = Key { image: image.clone(), full };
        if let Some(t) = self.loaded.get(&key) {
            return Some(t.clone());
        }
        let now = Instant::now();
        if self.failed.get(&key).is_some_and(|f| now < f.at) {
            return None;
        }
        let timeout = self.waits[2];
        let asked = self.pending.get(&key).copied();
        if asked.is_none_or(|t| now >= t + timeout) {
            if asked.is_some() {
                let text = format!("loading {} took over {}s: asked for again", key.image.id, timeout.as_secs());
                increart_core::applog::line("slow", &text);
            }
            self.pending.insert(key.clone(), now);
            self.queue.push(key, root.to_path_buf());
            // (a frame then, to ask again if it isn't back)
            self.ctx.request_repaint_after(timeout);
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

    /// The full-size image, its thumbnail while that loads (or if it can't).
    pub fn full_or_thumb(&mut self, root: &std::path::Path, image: &ImageRef) -> Option<TextureHandle> {
        self.full(root, image).or_else(|| self.thumb(root, image))
    }
}

impl Drop for Textures {
    fn drop(&mut self) {
        self.queue.close();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn image(id: &str) -> ImageRef {
        ImageRef::output(id.parse().unwrap())
    }

    fn key(id: &str, full: bool) -> Key {
        Key { image: image(id), full }
    }

    #[test]
    fn full_images_load_before_thumbnails() {
        let q = Queue::default();
        q.push(key("2026.11.02-00.31-0001", false), PathBuf::new());
        q.push(key("2026.11.02-00.31-0002", true), PathBuf::new());
        q.push(key("2026.11.02-00.31-0003", false), PathBuf::new());
        q.push(key("2026.11.02-00.31-0004", true), PathBuf::new());
        let order: Vec<_> = (0..4).map(|_| q.pop().unwrap().0.image.id.to_string()).collect();
        assert_eq!(
            order,
            ["2026.11.02-00.31-0002", "2026.11.02-00.31-0004", "2026.11.02-00.31-0001", "2026.11.02-00.31-0003"]
        );
        q.close();
        assert!(q.pop().is_none());
    }

    /// Loads until nothing is pending (5 s at most).
    fn settle(tex: &mut Textures, ctx: &egui::Context) {
        let end = Instant::now() + Duration::from_secs(5);
        while tex.busy() {
            assert!(Instant::now() < end, "loads never finished");
            std::thread::sleep(Duration::from_millis(5));
            tex.poll(ctx);
        }
    }

    fn textures(ctx: &egui::Context) -> Textures {
        let mut tex = Textures::new(ctx);
        tex.waits = [Duration::from_millis(20), Duration::from_millis(150), Duration::from_millis(100)];
        tex
    }

    #[test]
    fn a_failed_load_is_tried_again_quickly_then_now_and_then() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("output_images")).unwrap();
        let ctx = egui::Context::default();
        let mut tex = textures(&ctx);
        // waits no slow runner sleeps through: the test ends them itself
        tex.waits[0] = Duration::from_secs(600);
        tex.waits[1] = Duration::from_secs(3600);
        let end_waits = |tex: &mut Textures| tex.failed.values_mut().for_each(|f| f.at = Instant::now());
        let img = image("2026.11.02-00.31-0000");
        let k = key("2026.11.02-00.31-0000", true);
        let long_wait = |tex: &Textures| tex.failed[&k].at > Instant::now() + Duration::from_secs(1800);

        // not there: tried TRIES times, each after the short wait
        for i in 0..TRIES {
            assert!(tex.full_or_thumb(root, &img).is_none());
            assert!(tex.busy(), "try {i} asked for");
            settle(&mut tex, &ctx);
            assert!(tex.full_or_thumb(root, &img).is_none());
            assert!(!tex.busy(), "waits before trying again");
            assert_eq!(long_wait(&tex), i + 1 == TRIES, "the long wait only after {TRIES} tries");
            if i + 1 < TRIES {
                end_waits(&mut tex);
            }
        }
        assert_eq!(tex.failed[&k].tries, TRIES);
        // then the long wait: not asked for again during it
        assert!(tex.full(root, &img).is_none());
        assert!(!tex.busy(), "waits longer after {TRIES} tries");

        // there now: once the long wait is over, it shows
        image::RgbaImage::new(4, 2).save(img.path(root)).unwrap();
        end_waits(&mut tex);
        assert!(tex.full(root, &img).is_none());
        settle(&mut tex, &ctx);
        assert_eq!(tex.full(root, &img).map(|t| t.size()), Some([4, 2]));
        assert!(!tex.failed.contains_key(&k));
    }

    #[test]
    fn a_load_not_back_in_time_is_asked_for_again() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("output_images")).unwrap();
        let ctx = egui::Context::default();
        let mut tex = textures(&ctx);
        // the loaders gone: nothing comes back
        tex.queue.close();
        std::thread::sleep(Duration::from_millis(20));
        let img = image("2026.11.02-00.31-0000");
        let k = key("2026.11.02-00.31-0000", true);

        assert!(tex.full(root, &img).is_none());
        let first = tex.pending[&k];
        assert!(tex.full(root, &img).is_none());
        assert_eq!(tex.pending[&k], first, "not asked again before the timeout");
        std::thread::sleep(Duration::from_millis(110));
        assert!(tex.full(root, &img).is_none());
        assert!(tex.pending[&k] > first, "asked again after it");

        // an answer, late or not, is taken
        image::RgbaImage::new(4, 2).save(img.path(root)).unwrap();
        let (done, results) = mpsc::channel();
        tex.results = results;
        done.send((k.clone(), load(root, &k))).unwrap();
        tex.poll(&ctx);
        assert_eq!(tex.full(root, &img).map(|t| t.size()), Some([4, 2]));
        assert!(!tex.busy());
    }
}
