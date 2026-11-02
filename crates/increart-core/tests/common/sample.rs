//! A small synthetic library following docs/DESIGN.md (Filesystem layout,
//! Project folders, Input library folders), written file by file, not
//! through the core: the core's tests open it, and `verify` checks it.
//!
//! Content mirrors the GUI prototype's mock data. Images are procedural
//! 1024x1024 placeholders; model file hashes are sha256 of the file id, not
//! of real weights. Output is deterministic (fixed RNG seed). By hand:
//! `cargo run -p increart-core --example sample_library [out_dir]`.

use std::collections::HashMap;
use std::f64::consts::TAU;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use sha2::{Digest, Sha256};

const SIZE: u32 = 1024;
const SDCPP_COMMIT: &str = "fd21ecf";
const PE_MODEL: &str = "qwen3.5-9b-qwen-image-2.1-pe-t2i";
const RGBA_WRAP: [&str; 2] =
    ["This is an RGBA image with transparency. ", ". The image has alpha channel and the background is transparent."];

type Time = DateTime<Utc>;

fn t0() -> Time {
    Utc.with_ymd_and_hms(2026, 10, 18, 9, 0, 0).unwrap()
}

fn ts(t: Time) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn sha(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// SplitMix64: fixed, so every sample is the same.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In [0, 1).
    fn random(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

// ------------------------------------------------------------------ RSN

/// A value for the tiny RSN writer below.
#[derive(Clone)]
enum V {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Map(Vec<(String, V)>),
    /// A named struct, or (no fields) a unit enum variant.
    Named(&'static str, Option<Vec<(&'static str, V)>>),
    List(Vec<V>),
}

fn s(x: &str) -> V {
    V::Str(x.into())
}

fn named(name: &'static str, fields: Vec<(&'static str, V)>) -> V {
    V::Named(name, Some(fields))
}

fn unit(name: &'static str) -> V {
    V::Named(name, None)
}

fn opt(x: Option<V>) -> V {
    x.unwrap_or(V::None)
}

fn strs(xs: &[impl AsRef<str>]) -> V {
    V::List(xs.iter().map(|x| s(x.as_ref())).collect())
}

fn rsn_str(x: &str) -> String {
    format!("\"{}\"", x.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"))
}

/// In the style increart-core writes: 4-space indent, no trailing commas,
/// `Option` values bare, floats as Rust's Display writes them.
fn rsn(v: &V, ind: usize) -> String {
    let (pad, pad1) = ("    ".repeat(ind), "    ".repeat(ind + 1));
    let block = |open: &str, items: Vec<String>, close: &str| {
        format!("{open}\n{}\n{pad}{close}", items.iter().map(|i| format!("{pad1}{i}")).collect::<Vec<_>>().join(",\n"))
    };
    match v {
        V::None => "None".into(),
        V::Bool(b) => b.to_string(),
        V::Int(n) => n.to_string(),
        V::Float(f) => f.to_string(),
        V::Str(x) => rsn_str(x),
        V::Map(m) if m.is_empty() => "{}".into(),
        V::Map(m) => block("{", m.iter().map(|(k, x)| format!("{}: {}", rsn_str(k), rsn(x, ind + 1))).collect(), "}"),
        V::Named(name, None) => name.to_string(),
        V::Named(name, Some(f)) => {
            block(&format!("{name} {{"), f.iter().map(|(k, x)| format!("{k}: {}", rsn(x, ind + 1))).collect(), "}")
        }
        V::List(l) if l.is_empty() => "[]".into(),
        V::List(l) => block("[", l.iter().map(|x| rsn(x, ind + 1)).collect(), "]"),
    }
}

fn image_ref(store: &str, id: &str) -> V {
    named("ImageRef", vec![("id", s(id)), ("store", unit(if store == "input" { "Input" } else { "Output" }))])
}

// ------------------------------------------------------------------ PNG

fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend(kind);
    out.extend(data);
    let crc = crc32fast::hash(&out[start..]);
    out.extend(crc.to_be_bytes());
}

/// Channels, the rows with PNG filter 1 (Sub, which keeps smooth images
/// small), and the raw pixels.
type Picture = (u8, Vec<u8>, Vec<u8>);

/// A two-colour gradient with a soft blob; with alpha, the blob on a
/// transparent background.
fn picture(seed: u64, alpha: bool) -> Picture {
    let mut r = Rng(seed);
    let hue = r.random();
    let colour = |off: f64| -> [f64; 3] {
        std::array::from_fn(|k| (255.0 * (0.5 + 0.5 * (TAU * (hue + off + k as f64 / 3.0)).cos())).trunc())
    };
    let (c1, c2, blob) = (colour(0.0), colour(0.4), colour(0.7));
    let size = SIZE as f64;
    let (cx, cy) = (size * (0.3 + 0.4 * r.random()), size * (0.3 + 0.4 * r.random()));
    let rad = size * (0.15 + 0.15 * r.random());
    let ch = if alpha { 4 } else { 3 };
    let (mut raw, mut pixels) = (Vec::new(), Vec::new());
    let mut row = Vec::with_capacity(SIZE as usize * ch);
    for y in 0..SIZE {
        let t = y as f64 / (size - 1.0);
        let base: [f64; 3] = std::array::from_fn(|k| (c1[k] + (c2[k] - c1[k]) * t).trunc());
        let dy2 = (y as f64 - cy).powi(2);
        row.clear();
        for x in 0..SIZE {
            let w = (1.0 - ((x as f64 - cx).powi(2) + dy2).sqrt() / rad).max(0.0);
            let w = w * w * (3.0 - 2.0 * w);
            if alpha {
                row.extend([blob[0] as u8, blob[1] as u8, blob[2] as u8, (255.0 * w) as u8]);
            } else {
                row.extend((0..3).map(|k| (base[k] + (blob[k] - base[k]) * w) as u8));
            }
        }
        raw.push(1);
        raw.extend(&row[..ch]);
        raw.extend((ch..row.len()).map(|i| row[i].wrapping_sub(row[i - ch])));
        pixels.extend(&row);
    }
    (ch as u8, raw, pixels)
}

/// Content fingerprint per docs/DESIGN.md (Pixel hash): canonical encoding v1.
fn pixel_sha256(ch: u8, pixels: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b"increart-pixels-v1");
    h.update(SIZE.to_be_bytes());
    h.update(SIZE.to_be_bytes());
    h.update([ch]);
    h.update(pixels);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

enum Text<'a> {
    T(&'a str, &'a str),
    I(&'a str, &'a str),
}

/// Returns (file bytes, pixel sha256). Store files (with an `image_id`) also
/// get `increart:id` and `increart:pixels`.
fn png(pic: &Picture, texts: &[Text], image_id: Option<&str>) -> (Vec<u8>, String) {
    let (ch, raw, pixels) = pic;
    let h = pixel_sha256(*ch, pixels);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = [SIZE.to_be_bytes(), SIZE.to_be_bytes()].concat();
    ihdr.extend([8, if *ch == 4 { 6 } else { 2 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let ids = image_id.map(|id| [Text::T("increart:id", id), Text::T("increart:pixels", &h)]);
    for t in ids.iter().flatten().chain(texts) {
        match *t {
            Text::I(key, text) => chunk(&mut out, b"iTXt", &[key.as_bytes(), &[0; 5], text.as_bytes()].concat()),
            Text::T(key, text) => {
                // Latin-1, anything beyond it as "?"
                let latin1 = |x: &str| x.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect::<Vec<_>>();
                chunk(&mut out, b"tEXt", &[latin1(key), vec![0], latin1(text)].concat())
            }
        }
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(raw).unwrap();
    chunk(&mut out, b"IDAT", &z.finish().unwrap());
    chunk(&mut out, b"IEND", b"");
    (out, h)
}

// ------------------------------------------------------------------ the library

/// An original as dropped: (name, seed, file name, its folder, format, conversions).
type Original = (&'static str, u64, Option<&'static str>, Option<&'static str>, &'static str, &'static [&'static str]);

const DL: &str = "/home/user/Downloads";
const PICS: &str = "/home/user/Pictures";
const PHONE: &str = "/mnt/storage/sdcpp/increart_lib/input/inbox_drop";
const ORIGINALS: [Original; 10] = [
    ("lighthouse", 2, None, None, "png", &[]),
    ("untitled-a", 5, Some("Screenshot_20261020-101500.png"), Some(PHONE), "png", &[]),
    ("untitled-b", 6, Some("Screenshot_20261020-101512.png"), Some(PHONE), "png", &[]),
    ("robot", 4, Some("robot-mascot.webp"), Some(DL), "webp", &["webp -> png"]),
    ("glaze", 9, Some("glaze.jpg"), Some(DL), "jpeg", &["jpeg -> png"]),
    ("hut", 11, Some("hut.jpg"), Some(PICS), "jpeg", &["exif-orientation: rotate 90", "jpeg -> png"]),
    ("fish-a", 12, Some("fish_01.jpg"), Some(PICS), "jpeg", &["icc: Display P3 -> sRGB", "jpeg -> png"]),
    ("fish-b", 13, Some("fish_02.jpg"), Some(PICS), "jpeg", &["icc: Display P3 -> sRGB", "jpeg -> png"]),
    ("papercut", 14, Some("papercut.png"), Some(PICS), "png", &[]),
    ("rejected", 21, Some("beach.jpg"), Some(DL), "jpeg", &["jpeg -> png"]),
];

const MODEL_FILES: [(&str, &str); 3] = [
    ("diffusion_model", "diffusion_models/qwen_image_2.1_int8_convrot.safetensors"),
    ("llm", "text_encoders/qwen3vl_8b_int8_convrot.safetensors"),
    ("vae", "vae/qwen_image_2.1_vae_bf16.safetensors"),
];

fn model() -> V {
    let files = MODEL_FILES
        .iter()
        .map(|(slot, id)| {
            (slot.to_string(), named("ModelFile", vec![("id", s(id)), ("sha256", s(&sha(id.as_bytes())))]))
        })
        .collect();
    named("Model", vec![("recipe", s("qwen-image-21")), ("files", V::Map(files))])
}

/// The `increart:source` record of an original.
struct Import {
    size: u64,
    sha256: String,
    mtime: String,
}

struct Idea {
    key: &'static str,
    id: String,
    dir: String,
    title: &'static str,
    text: &'static str,
    variants: Vec<&'static str>,
    tags: Vec<&'static str>,
    source: V,
    created: Time,
    images: Vec<String>,
    projects: Vec<String>,
    starred: bool,
}

#[derive(Clone, Copy)]
struct Sample {
    method: &'static str,
    scheduler: &'static str,
    steps: u32,
    cfg: f64,
    flow_shift: f64,
}

const SAMPLE: Sample = Sample { method: "euler", scheduler: "simple", steps: 20, cfg: 6.0, flow_shift: 3.0 };

impl Sample {
    fn rsn(&self) -> V {
        named(
            "Sample",
            vec![
                ("method", s(self.method)),
                ("scheduler", s(self.scheduler)),
                ("steps", V::Int(self.steps.into())),
                ("cfg", V::Float(self.cfg)),
                ("flow_shift", V::Float(self.flow_shift)),
            ],
        )
    }
}

/// The rest of a run, past its project, time, label, prompt, seeds and verdicts.
struct RunArgs {
    alpha: bool,
    mode: V,
    refs: Vec<V>,
    parent: Option<V>,
    source: Option<&'static str>,
    sample: Sample,
    first: u64,
    /// (sweep label, task, preset) for one style of a style sweep.
    sweep: Option<(&'static str, &'static str, &'static str)>,
}

impl Default for RunArgs {
    fn default() -> Self {
        RunArgs {
            alpha: false,
            mode: unit("Generate"),
            refs: vec![],
            parent: None,
            source: None,
            sample: SAMPLE,
            first: 0,
            sweep: None,
        }
    }
}

struct Gen {
    out: PathBuf,
    rng: Rng,
    /// (kind, minute) -> next number
    counters: HashMap<(&'static str, String), u32>,
    /// original name -> its picture, pixel sha256 and file name
    pictures: HashMap<&'static str, (Picture, String, &'static str)>,
    imports: HashMap<&'static str, Import>,
    /// pixel sha256 -> input image ID (the pixel index)
    input_ids: HashMap<String, String>,
    ideas: Vec<Idea>,
    /// key -> (ID, path)
    projects: HashMap<&'static str, (String, &'static str)>,
    /// Outputs being drawn and written on threads of their own (nothing
    /// reads them back while generating); joined before `write` returns.
    drawing: Vec<std::thread::JoinHandle<()>>,
}

impl Gen {
    /// `YYYY.MM.DD-HH.MM-NNNN` in UTC, numbered from 0000 per kind within the
    /// minute (docs/DESIGN.md, IDs).
    fn new_id(&mut self, kind: &'static str, t: Time) -> String {
        let minute = t.format("%Y.%m.%d-%H.%M").to_string();
        let n = self.counters.entry((kind, minute.clone())).or_insert(0);
        *n += 1;
        format!("{minute}-{:04}", *n - 1)
    }

    /// Text as UTF-8 with "\n" line ends on every OS, as the app writes it.
    fn write(&self, path: &str, data: impl AsRef<[u8]>) {
        let path = self.out.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }

    /// Relative symlink at `path` (library-relative) to `<store>_images/<image_id>.png`.
    fn link(&self, path: &str, store: &str, image_id: &str) {
        let at = self.out.join(path);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        let mut target: PathBuf = (1..path.split('/').count()).map(|_| "..").collect();
        target.push(format!("{store}_images"));
        target.push(format!("{image_id}.png"));
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, at).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, at).unwrap();
    }

    fn original(name: &str) -> &'static Original {
        ORIGINALS.iter().find(|o| o.0 == name).unwrap()
    }

    /// Imports into the input store at `imported` (the idea's creation time),
    /// reusing an existing image with the same pixels. Returns the image ID.
    fn store_input(&mut self, name: &'static str, imported: Time) -> String {
        let h = self.pictures[name].1.clone();
        if let Some(id) = self.input_ids.get(&h) {
            return id.clone();
        }
        let &(_, _, fname, dir, format, conversions) = Self::original(name);
        let src = &self.imports[name];
        let json_str = |x: Option<&str>| x.map_or("null".into(), |x| serde_json::to_string(x).unwrap());
        let convs = if conversions.is_empty() {
            "[]".into()
        } else {
            let items: Vec<_> = conversions.iter().map(|c| format!("    {}", json_str(Some(*c)))).collect();
            format!("[\n{}\n  ]", items.join(",\n"))
        };
        let path = dir.map(|d| format!("{d}/{}", fname.unwrap()));
        let fields = [
            ("version", "1".to_string()),
            ("original_name", json_str(fname)),
            ("original_path", json_str(path.as_deref())),
            ("original_format", json_str(Some(format))),
            ("original_size", src.size.to_string()),
            ("original_sha256", json_str(Some(&src.sha256))),
            ("original_mtime", json_str(Some(&src.mtime))),
            ("original_width", SIZE.to_string()),
            ("original_height", SIZE.to_string()),
            ("imported", json_str(Some(&ts(imported)))),
            ("conversions", convs),
        ];
        let json = format!(
            "{{\n{}\n}}",
            fields.iter().map(|(k, v)| format!("  \"{k}\": {v}")).collect::<Vec<_>>().join(",\n")
        );
        let id = self.new_id("input", imported);
        self.input_ids.insert(h, id.clone());
        let (data, _) = png(&self.pictures[name].0, &[Text::I("increart:source", &json)], Some(&id));
        self.write(&format!("input_images/{id}.png"), data);
        id
    }

    #[allow(clippy::too_many_arguments)]
    fn idea(
        &mut self,
        key: &'static str,
        at: &str,
        t: Time,
        title: &'static str,
        text: &'static str,
        tags: &[&'static str],
        imgs: &[&'static str],
        source: V,
    ) -> &mut Idea {
        let id = self.new_id("idea", t);
        // "library/<collection>": the idea in the library, in that collection
        let at = match at.strip_prefix("library/") {
            Some(collection) => {
                self.write(&format!("input/collections/{collection}/{id}.idea"), "");
                "library"
            }
            None => at,
        };
        let dir = if at.is_empty() { format!("input/{id}") } else { format!("input/{at}/{id}") };
        let images: Vec<_> = imgs.iter().map(|i| self.store_input(i, t)).collect();
        for (n, (name, image_id)) in imgs.iter().zip(&images).enumerate() {
            let file = self.pictures[name].2;
            let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
            self.link(&format!("{dir}/{}-{stem}.png", n + 1), "input", image_id);
        }
        self.ideas.push(Idea {
            key,
            id,
            dir,
            title,
            text,
            variants: vec![],
            tags: tags.to_vec(),
            source,
            created: t,
            images,
            projects: vec![],
            starred: false,
        });
        self.ideas.last_mut().unwrap()
    }

    fn idea_by(&mut self, key: &str) -> &mut Idea {
        self.ideas.iter_mut().find(|i| i.key == key).unwrap()
    }

    fn project(&mut self, key: &'static str, path: &'static str, t: Time, sources: &[&str]) {
        let id = self.new_id("project", t);
        let mut ids = vec![];
        for src in sources {
            let idea = self.idea_by(src);
            idea.projects.push(id.clone());
            ids.push(idea.id.clone());
        }
        let body = named("Project", vec![("id", s(&id)), ("created", s(&ts(t))), ("sources", strs(&ids))]);
        self.write(&format!("projects/{path}/project.rsn"), rsn(&body, 0) + "\n");
        self.projects.insert(key, (id, path));
    }

    fn input_ref(&self, name: &str) -> V {
        image_ref("input", &self.input_ids[&self.pictures[name].1])
    }

    /// Writes run.rsn and one PNG per finished seed. `verdicts[i]` is the
    /// review folder for seed i ("good"/"neutral"/"bad"), None = unreviewed.
    /// Returns each image's (ID, folder).
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        proj: &str,
        t: Time,
        label: &str,
        prompt: &str,
        seeds: u64,
        verdicts: &[Option<&str>],
        a: RunArgs,
    ) -> Vec<(String, String)> {
        let (pid_path, rid) = (self.projects[proj].1, self.new_id("run", t));
        let sent = if a.alpha {
            format!("{}{}{}", RGBA_WRAP[0], prompt.trim_end_matches('.'), RGBA_WRAP[1])
        } else {
            prompt.to_string()
        };
        let source_idea = a.source.map(|k| s(&self.idea_by(k).id));
        let device = |i: usize| s(["ROCm0", "ROCm1"][i % 2]);
        // each finished seed's timing (run.rsn `timings`, DESIGN.md Timing):
        // generating ~31-37 s, finished ~35 s apart on two GPUs
        let elapsed: Vec<f64> =
            verdicts.iter().map(|_| ((31.0 + self.rng.random() * 6.0) * 10.0).round() / 10.0).collect();
        let step_s = |e: f64| ((e - 2.5) / a.sample.steps as f64 * 100.0).round() / 100.0;
        let done = |i: usize| t + TimeDelta::seconds(35 * (i as i64 + 1));
        let timings: Vec<V> = elapsed
            .iter()
            .enumerate()
            .map(|(i, &e)| {
                named(
                    "SeedTiming",
                    vec![
                        ("rev", V::Int(0)),
                        ("seed", V::Int((a.first + i as u64) as i64)),
                        ("device", device(i)),
                        ("done", s(&ts(done(i)))),
                        ("elapsed_s", V::Float(e)),
                        ("queued_s", V::Float(1.0)),
                        ("step_s", V::Float(step_s(e))),
                    ],
                )
            })
            .collect();
        let seeds_v = named("Seeds", vec![("first", V::Int(a.first as i64)), ("count", V::Int(seeds as i64))]);
        let mut fields = vec![
            ("id", s(&rid)),
            ("label", s(label)),
            ("created", s(&ts(t))),
            ("prompt", s(prompt)),
            ("negative_prompt", s("")),
            ("alpha", V::Bool(a.alpha)),
            ("width", V::Int(SIZE.into())),
            ("height", V::Int(SIZE.into())),
            ("sample", a.sample.rsn()),
            ("mode", a.mode.clone()),
            ("refs", V::List(a.refs.clone())),
            ("model", model()),
            ("source_idea", opt(source_idea.clone())),
            (
                "revisions",
                V::List(vec![named("Revision", vec![("rev", V::Int(0)), ("created", s(&ts(t))), ("seeds", seeds_v)])]),
            ),
        ];
        if let Some((name, task, preset)) = a.sweep {
            fields.push(("sweep", named("Sweep", vec![("name", s(name)), ("task", s(task)), ("preset", s(preset))])));
        }
        if !timings.is_empty() {
            fields.push(("timings", V::List(timings)));
        }
        self.write(&format!("projects/{pid_path}/runs/{rid}/run.rsn"), rsn(&named("Run", fields), 0) + "\n");

        let mut out = vec![];
        for (i, verdict) in verdicts.iter().enumerate() {
            let seed = a.first + i as u64;
            let recipe = named(
                "Recipe",
                vec![
                    ("prompt_original", s(prompt)),
                    ("prompt_sent", s(&sent)),
                    ("negative_prompt", s("")),
                    ("width", V::Int(SIZE.into())),
                    ("height", V::Int(SIZE.into())),
                    ("seed", V::Int(seed as i64)),
                    ("alpha", V::Bool(a.alpha)),
                    ("sample", a.sample.rsn()),
                    ("mode", a.mode.clone()),
                    ("refs", V::List(a.refs.clone())),
                    ("parent", opt(a.parent.clone())),
                    ("source_idea", opt(source_idea.clone())),
                    ("rewrite", V::None),
                    ("model", model()),
                    ("sdcpp", s(SDCPP_COMMIT)),
                    ("increart", s("0.0.0-sample")),
                    ("device", device(i)),
                    ("elapsed_s", V::Float(elapsed[i])),
                    ("queued_s", V::Float(1.0)),
                    ("step_s", V::Float(step_s(elapsed[i]))),
                ],
            );
            let sm = &a.sample;
            let params = format!(
                "{sent}\nSteps: {}, Sampler: {}, CFG scale: {:?}, Seed: {seed}, Size: {SIZE}x{SIZE}",
                sm.steps, sm.method, sm.cfg
            );
            let oid = self.new_id("output", done(i)); // finished ~35 s apart
            let pic_seed = u64::from_str_radix(&sha(format!("{rid}-{seed}").as_bytes())[..8], 16).unwrap();
            let recipe = rsn(&recipe, 0);
            let path = self.out.join(format!("output_images/{oid}.png"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let (id, alpha) = (oid.clone(), a.alpha);
            self.drawing.push(std::thread::spawn(move || {
                let texts = [Text::I("increart:recipe", &recipe), Text::T("parameters", &params)];
                fs::write(path, png(&picture(pic_seed, alpha), &texts, Some(&id)).0).unwrap();
            }));
            let folder = verdict.map_or("unreviewed".into(), |v| format!("reviewed/{v}"));
            self.link(&format!("projects/{pid_path}/{folder}/{rid}_r0_s{seed:04}.png"), "output", &oid);
            out.push((oid, folder));
        }
        out
    }
}

/// Writes the sample library into `out`, which must be missing or empty.
pub fn write(out: &Path) {
    assert!(
        !out.is_dir() || fs::read_dir(out).unwrap().next().is_none(),
        "{} is not empty; remove it or pass another dir",
        out.display()
    );
    let mut g = Gen {
        out: out.to_path_buf(),
        rng: Rng(42),
        counters: HashMap::new(),
        pictures: HashMap::new(),
        imports: HashMap::new(),
        input_ids: HashMap::new(),
        ideas: vec![],
        projects: HashMap::new(),
        drawing: vec![],
    };
    let t0 = t0();
    let (day, hour, minute) = (TimeDelta::days(1), TimeDelta::hours(1), TimeDelta::minutes(1));

    let library = named("Library", vec![("format", V::Int(1)), ("link_mode", unit("Symlink"))]);
    g.write(
        "library.rsn",
        format!("// Library format and link kind (see docs/DESIGN.md, Links).\n{}\n", rsn(&library, 0)),
    );
    let res = |name: &str, width: i64, height: i64, default: bool| {
        let mut f = vec![("name", s(name)), ("width", V::Int(width)), ("height", V::Int(height))];
        if default {
            f.push(("default", V::Bool(true)));
        }
        named("Resolution", f)
    };
    let config = named(
        "Config",
        vec![(
            "resolutions",
            V::List(vec![
                res("Square", 1024, 1024, true),
                res("Square L", 1344, 1344, false),
                res("Square XL", 2048, 2048, false),
                res("Portrait 2:3", 896, 1344, false),
                res("Landscape 3:2", 1344, 896, false),
                res("Wide 16:9", 1664, 928, false),
            ]),
        )],
    );
    g.write(
        "config/config.rsn",
        format!("// Sample of ~/.config/increart/config.rsn (see docs/DESIGN.md, Config file).\n{}\n", rsn(&config, 0)),
    );

    // ------------------------------------------------------------------ input side

    // the synthetic originals, as dropped (PNGs here; the pixel hash ignores chunks)
    let mut dropped = HashMap::new();
    // drawn side by side (unoptimised in tests, they take a while)
    let pics: Vec<Picture> = std::thread::scope(|sc| {
        let drawn: Vec<_> =
            ORIGINALS.iter().map(|&(name, seed, ..)| sc.spawn(move || picture(seed, name == "robot"))).collect();
        drawn.into_iter().map(|d| d.join().unwrap()).collect()
    });
    for (&(name, _, fname, _, _, _), pic) in ORIGINALS.iter().zip(pics) {
        let (data, h) = png(&pic, &[], None);
        let import = Import {
            size: 150_000 + g.rng.below(3_000_000),
            sha256: sha(format!("synthetic original {name}").as_bytes()),
            mtime: ts(t0 - day * (1 + g.rng.below(299)) as i32),
        };
        g.imports.insert(name, import);
        g.pictures.insert(name, (pic, h, fname.unwrap_or("clipboard.png")));
        dropped.insert(name, data);
    }

    let manual = || unit("Manual");
    let dropped_file = |p: &str| named("DroppedFile", vec![("original_path", s(p))]);
    g.idea(
        "fox",
        "library",
        t0,
        "Fox in café",
        "a red fox reading a newspaper in a sunlit cafe, film photograph",
        &["animals", "photo"],
        &[],
        manual(),
    )
    .starred = true;
    g.idea(
        "lighthouse",
        "unreviewed",
        t0 + day * 2,
        "Moody lighthouse",
        "lighthouse on a cliff at dusk, storm rolling in, painterly",
        &["landscape"],
        &["lighthouse"],
        unit("Clipboard"),
    );
    g.idea(
        "untitled",
        "unreviewed",
        t0 + day * 2 + hour * 3,
        "",
        "",
        &[],
        &["untitled-a", "untitled-b"],
        unit("InboxFolder"),
    );
    g.idea(
        "robot",
        "library",
        t0 + hour * 5,
        "Sticker set",
        "cute robot mascot stickers, flat vector, transparent background",
        &["alpha", "vector"],
        &["robot"],
        manual(),
    )
    .starred = true;
    g.idea(
        "glaze",
        "unreviewed",
        t0 + day * 3,
        "Ceramic texture",
        "use this glaze texture as a style reference",
        &["style"],
        &["glaze"],
        dropped_file("/home/user/Downloads/glaze.jpg"),
    );
    g.idea(
        "jazz",
        "unreviewed",
        t0 + day * 3 + hour,
        "Poster: jazz night",
        "1960s jazz club poster with bold type 'BLUE NOTE NIGHTS'",
        &["poster", "text"],
        &[],
        manual(),
    );
    g.idea(
        "hut",
        "library/places",
        t0 + hour,
        "Kept: alpine hut",
        "wooden hut in alpine meadow, morning fog",
        &["landscape"],
        &["hut"],
        manual(),
    )
    .starred = true;
    g.idea(
        "fish",
        "library",
        t0 + hour * 2,
        "Kept: glass fish",
        "translucent glass fish sculpture, studio lighting",
        &["product"],
        &["fish-a", "fish-b"],
        manual(),
    );
    let alley = g.idea(
        "alley",
        "library/places",
        t0 + hour * 3,
        "Kept: neon alley",
        "rainy neon alley, cinematic",
        &["city"],
        &[],
        manual(),
    );
    alley.starred = true;
    alley.variants = vec![
        "A narrow alley at night in heavy rain, magenta and cyan neon signs reflected in puddles, shallow depth of field, anamorphic lens flare, cinematic colour grade.",
    ];
    g.idea(
        "papercut",
        "library/styles/paper",
        t0 + hour * 4,
        "Ref: paper cut style",
        "",
        &["style"],
        &["papercut"],
        dropped_file("/home/user/Pictures/papercut.png"),
    );
    g.idea(
        "rejected",
        "discarded",
        t0 + hour * 6,
        "Blurry beach",
        "beach at noon, harsh light",
        &["landscape"],
        &["rejected"],
        manual(),
    );

    // Dropped from outside the app: two files not yet picked up by the watcher,
    // and the originals of the InboxFolder idea, moved aside after import.
    g.write("input/inbox_drop/from-phone.txt", "otter floating on its back holding a teacup, watercolour\n");
    g.write("input/inbox_drop/IMG_2041.png", png(&picture(31, false), &[], None).0);
    for name in ["untitled-a", "untitled-b"] {
        g.write(&format!("input/inbox_drop/imported/{}", Gen::original(name).2.unwrap()), &dropped[name]);
    }

    // ------------------------------------------------------------------ projects

    let (good, neutral, bad) = (Some("good"), Some("neutral"), Some("bad"));
    let fox_text = g.idea_by("fox").text;
    g.project("fox", "clients/acme/fox-cafe", t0 + day, &["fox"]);
    let r01 = g.run(
        "fox",
        t0 + day,
        "r01",
        fox_text,
        8,
        &[good, neutral, bad, good, neutral, bad, neutral, bad],
        RunArgs { source: Some("fox"), sample: Sample { cfg: 1.0, ..SAMPLE }, ..Default::default() },
    );
    g.run(
        "fox",
        t0 + day + hour * 4,
        "r02 cfg6",
        fox_text,
        32,
        &[None; 6],
        RunArgs { source: Some("fox"), ..Default::default() },
    );

    g.project("fish", "experiments/glass-fish", t0 + day * 2, &["fish"]);
    let fish_init = g.input_ref("fish-a");
    let edit = named("Edit", vec![("init", fish_init.clone()), ("strength", V::Float(0.75))]);
    let refs = vec![g.input_ref("fish-b")];
    let args = RunArgs { mode: edit, refs, parent: Some(fish_init), source: Some("fish"), ..Default::default() };
    g.run("fish", t0 + day * 2, "r01 edit", "make the fish amber glass", 16, &[good, None, None, None], args);

    g.project("stickers", "stickers", t0 + day * 2 + hour * 6, &["robot"]);
    let args = RunArgs { alpha: true, refs: vec![g.input_ref("robot")], source: Some("robot"), ..Default::default() };
    let verdicts = [good, good, neutral, neutral, None, None];
    g.run("stickers", t0 + day * 2 + hour * 6, "r01 rgba", "cute robot mascot sticker", 6, &verdicts, args);

    // A style sweep: one run per style, the same three seeds; the last style's
    // third image is still to come.
    let styles: [(&str, &str, &[Option<&str>]); 4] = [
        ("Claymation", "claymation, soft plasticine with fingerprints", &[good, neutral, None]),
        ("Pixel art", "16-bit pixel art, crisp pixels, limited palette", &[bad, bad, None]),
        ("Watercolour", "loose watercolour, soft washes, paper texture", &[None, None, None]),
        ("Ukiyo-e woodblock", "ukiyo-e woodblock print, flat colour, bold outlines", &[None, None]),
    ];
    for (i, (style, words, verdicts)) in styles.into_iter().enumerate() {
        let args = RunArgs { sweep: Some(("r02", "Modify style", style)), ..Default::default() };
        let (label, prompt) = (format!("r02 · {style}"), format!("cute robot mascot sticker, {words}"));
        g.run("stickers", t0 + day * 3 + minute * i as i32, &label, &prompt, 3, verdicts, args);
    }

    // An idea made from an output (I in review), still in the Idea inbox.
    let good_id = r01.iter().find(|(_, folder)| folder == "reviewed/good").unwrap().0.clone();
    let from = named("FromOutput", vec![("image", image_ref("output", &good_id))]);
    g.idea("from-fox", "unreviewed", t0 + day * 3 + hour * 2, "from fox-cafe", fox_text, &[], &[], from);

    // ------------------------------------------------------------------ ideas, empty dirs, README

    for idea in &g.ideas {
        let variant = |v: &&str| {
            let origin = named(
                "Rewritten",
                vec![("model", s(PE_MODEL)), ("system_prompt_hash", s(&sha(b"sample PE system prompt (t2i)")))],
            );
            named("PromptVariant", vec![("text", s(v)), ("origin", origin)])
        };
        let body = named(
            "Idea",
            vec![
                ("id", s(&idea.id)),
                ("title", s(idea.title)),
                ("text", s(idea.text)),
                ("variants", V::List(idea.variants.iter().map(variant).collect())),
                ("tags", strs(&idea.tags)),
                ("source", idea.source.clone()),
                ("created", s(&ts(idea.created))),
                ("images", strs(&idea.images)),
                ("projects", strs(&idea.projects)),
                ("starred", V::Bool(idea.starred)),
            ],
        );
        g.write(&format!("{}/idea.rsn", idea.dir), rsn(&body, 0) + "\n");
    }
    for d in ["thumbs", "cache/conditioning"] {
        fs::create_dir_all(out.join(d)).unwrap();
    }
    g.write("README.md", README);
    for d in g.drawing.drain(..) {
        d.join().unwrap();
    }
}

const README: &str = "# Sample IncreArt library

Synthetic data in the layout of `docs/DESIGN.md` (Filesystem layout, Project
folders, Input library folders), generated by
`crates/increart-core/tests/common/sample.rs`. Mirrors the GUI prototype's
mock data.

- `config/config.rsn` stands in for `~/.config/increart/config.rsn`;
  `library.rsn` holds the format version and link mode (symlinks here).
- IDs (ideas, projects, runs, images) are `YYYY.MM.DD-HH.MM-NNNN`: UTC
  creation minute plus a counter from 0000, separate per kind.
- `input_images/` and `output_images/` are the image stores: every image, as
  PNG, named by its ID. Every other image file in the library is a relative
  symlink into them.
- `input/unreviewed/` is the Idea inbox, `input/library/` the input library
  (a folder tree), `input/discarded/` discarded ideas. Each idea folder has
  `idea.rsn` plus links to its images.
- `input/inbox_drop/` has two files not yet picked up; `imported/` holds
  originals already imported.
- `projects/` is the project folder tree; a directory with `project.rsn` is a
  project. Its `unreviewed/` and `reviewed/*/` hold links.
- Store PNGs carry `increart:id` (their ID) and `increart:pixels` (hash of
  the decoded pixels, see Pixel hash in the design doc). Inputs also carry
  `increart:source` (JSON: the original file's name, path, format, size,
  hash, conversions); outputs carry `increart:recipe` (RSN) plus an
  sd-server-style `parameters` chunk.
- `thumbs/` and `cache/conditioning/` are empty caches.

Not real data: images are procedural placeholders; the \"originals\" in
`increart:source` (names, sizes, hashes, conversions) are invented, and none
carries EXIF; model `sha256` values are the hash of the file id; timings are
made up.
";
