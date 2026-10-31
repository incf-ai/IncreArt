//! **Save for a bug report…** (the Logs window): a zip of what's needed to
//! see what went wrong (DESIGN.md, Server logs): the config and the copies
//! kept beside it, the end of every log (the app's own and the servers'),
//! what's in the model directories (names and sizes only), and the
//! version and system. Passwords in the logs are masked; no images, no
//! library.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// How much of a log's end goes in.
pub const LOG_TAIL: u64 = 2 << 20;
/// A model directory is listed this deep, and to this many entries.
const LIST_DEPTH: usize = 4;
const LIST_MAX: usize = 3000;
/// A file beside the config bigger than this is left out.
const CONFIG_MAX: u64 = 4 << 20;
/// A validation report bigger than this is left out.
const VALIDATION_MAX: u64 = 16 << 20;

pub struct Report {
    /// `config.rsn`; its folder's `config.rsn.*` go in too.
    pub config: Option<PathBuf>,
    /// The log folder: each `.log` in it.
    pub logs: PathBuf,
    pub model_dirs: Vec<String>,
    /// The version and how the app was started ([`crate::applog::about`]).
    pub about: String,
    /// The server validation stamps file: it, and each preset's newest
    /// validation report, go in.
    pub validations: Option<PathBuf>,
}

/// Writes `r` to `dest`, returning the zip's size; a zip not finished is
/// removed.
pub fn write(r: &Report, dest: &Path) -> Result<u64, String> {
    let done = write_zip(r, dest).map_err(|e| e.to_string());
    if done.is_err() {
        let _ = fs::remove_file(dest);
    }
    done
}

fn write_zip(r: &Report, dest: &Path) -> std::io::Result<u64> {
    use zip::write::SimpleFileOptions;
    let mut z = zip::ZipWriter::new(fs::File::create(dest)?);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let top = dest.file_stem().map_or("increart-bug-report".into(), |s| s.to_string_lossy().into_owned());
    let mut put = |name: &str, bytes: &[u8]| -> std::io::Result<()> {
        z.start_file(format!("{top}/{name}"), opts)?;
        z.write_all(bytes)
    };
    let mut contents = vec![];

    let configs = config_files(r.config.as_deref());
    for (name, path) in &configs {
        match fs::read(path) {
            Ok(bytes) => {
                put(&format!("config/{name}"), &bytes)?;
                contents.push(format!("config/{name}"));
            }
            Err(e) => contents.push(format!("config/{name}: not read ({e})")),
        }
    }
    if configs.is_empty() {
        contents.push(match &r.config {
            Some(p) => format!("no config: {} isn't there (the defaults are in use)", p.display()),
            None => "no config: no config folder found".into(),
        });
    }

    for name in log_files(&r.logs) {
        let (text, cut) = tail(&r.logs.join(&name), LOG_TAIL)?;
        let mut text = mask_passwords(&text);
        if cut > 0 {
            text = format!("[the first {:.1} MB left out]\n{text}", cut as f64 / 1e6);
        }
        put(&format!("logs/{name}"), text.as_bytes())?;
        contents.push(format!("logs/{name}"));
    }

    put("model-directories.txt", list_model_dirs(&r.model_dirs).as_bytes())?;
    contents.push("model-directories.txt".into());

    // which presets were validated, each with the report that says how
    match r.validations.as_deref().filter(|p| p.is_file()) {
        None => contents.push("validation: no server preset validated yet".into()),
        Some(path) => {
            put("validation/validated.json", &fs::read(path)?)?;
            contents.push("validation/validated.json".into());
            let v = crate::validation::Validations::load(path);
            let mut zips: Vec<&Path> = vec![];
            for s in v.newest_each() {
                let Some(zip) = s.zip.as_deref() else { continue };
                if zips.contains(&zip) {
                    continue;
                }
                zips.push(zip);
                let name = zip.file_name().map_or("report.zip".into(), |n| n.to_string_lossy().into_owned());
                match fs::metadata(zip) {
                    Ok(m) if m.len() <= VALIDATION_MAX => {
                        put(&format!("validation/{name}"), &fs::read(zip)?)?;
                        contents.push(format!("validation/{name} ({}'s newest)", s.preset));
                    }
                    Ok(m) => contents.push(format!("validation/{name}: left out ({:.0} MB)", m.len() as f64 / 1e6)),
                    Err(e) => contents.push(format!("validation/{name}: not read ({e})")),
                }
            }
        }
    }

    let about = format!(
        "{}\nsystem: {} {}\nconfig: {}\nlogs: {}\nsaved: {}\n\nIn this zip:\n  about.txt\n{}\n",
        r.about.trim_end(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        r.config.as_ref().map_or("none".into(), |p| p.display().to_string()),
        r.logs.display(),
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z"),
        contents.iter().map(|c| format!("  {c}")).collect::<Vec<_>>().join("\n"),
    );
    put("about.txt", about.as_bytes())?;
    let mut f = z.finish()?;
    f.flush()?;
    f.seek(SeekFrom::End(0))
}

/// `config.rsn` and the files kept beside it (`config.rsn.unreadable`,
/// `config.rsn.before-…`), as (name, path).
fn config_files(config: Option<&Path>) -> Vec<(String, PathBuf)> {
    let Some(config) = config else { return vec![] };
    let Some(name) = config.file_name().map(|n| n.to_string_lossy().into_owned()) else { return vec![] };
    let dir = config.parent().unwrap_or(Path::new("."));
    let mut found: Vec<(String, PathBuf)> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    let small = e.metadata().is_ok_and(|m| m.is_file() && m.len() <= CONFIG_MAX);
                    (small && (n == name || n.starts_with(&format!("{name}.")))).then(|| (n, e.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    // the config first, then the rest by name
    found.sort_by(|a, b| (a.0 != name).cmp(&(b.0 != name)).then(a.0.cmp(&b.0)));
    found
}

fn log_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.metadata().is_ok_and(|m| m.is_file()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".log"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The last `max` bytes of `path` (from a line's start), and how many
/// before them were left out.
fn tail(path: &Path, max: u64) -> std::io::Result<(String, u64)> {
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    let from = len.saturating_sub(max);
    f.seek(SeekFrom::Start(from))?;
    let mut bytes = vec![];
    f.read_to_end(&mut bytes)?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    let mut cut = from;
    if from > 0
        && let Some(nl) = text.find('\n')
    {
        cut += nl as u64 + 1;
        text.drain(..=nl);
    }
    Ok((text, cut))
}

/// `…password='x'`, `…password=x`, `"…password": "x"` and `--…password x`
/// with the value as `***` (KoboldCpp prints its settings, the admin
/// password among them, as it starts). None and empty values are kept.
pub fn mask_passwords(text: &str) -> String {
    text.split_inclusive('\n').map(mask_line).collect()
}

fn mask_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let (mut out, mut at) = (String::new(), 0);
    while let Some(p) = lower[at..].find("password") {
        let end = at + p + "password".len();
        out.push_str(&line[at..end]);
        at = end;
        let rest = &line[end..];
        // what comes between the name and the value
        let sep = if rest.starts_with("\": \"") {
            4
        } else if rest.starts_with("='") || rest.starts_with("=\"") {
            2
        } else if rest.starts_with('=') || (rest.starts_with(' ') && flag(&lower[..end])) {
            1
        } else {
            continue;
        };
        let quote = rest[..sep].chars().last().filter(|c| *c == '\'' || *c == '"');
        let value = &rest[sep..];
        let len = match quote {
            Some(q) => value.find(q).unwrap_or(value.len()),
            None => value.find(|c: char| c.is_whitespace() || c == ',' || c == ')').unwrap_or(value.len()),
        };
        out.push_str(&rest[..sep]);
        let v = &value[..len];
        out.push_str(if v.is_empty() || v == "None" || v == "null" || v == "***" { v } else { "***" });
        at = end + sep + len;
    }
    out.push_str(&line[at..]);
    out
}

/// The word `before` ends with is a `--flag` (only then is the next word
/// its value).
fn flag(before: &str) -> bool {
    before[before.rfind(char::is_whitespace).map_or(0, |i| i + 1)..].starts_with("--")
}

/// Each model directory's files under it (`loras/` too), with their sizes.
fn list_model_dirs(dirs: &[String]) -> String {
    let mut out = String::from("The model directories, in their order (names and sizes only).\n");
    if dirs.is_empty() {
        out.push_str("\nNone set.\n");
    }
    for (i, d) in dirs.iter().enumerate() {
        let root = Path::new(d);
        out.push_str(&format!("\n{}. {d}\n", i + 1));
        if !root.is_dir() {
            out.push_str("   isn't there (or isn't a folder)\n");
            continue;
        }
        let mut lines = vec![];
        walk(root, root, 0, &mut lines);
        lines.sort();
        let more = lines.len().saturating_sub(LIST_MAX);
        for l in lines.iter().take(LIST_MAX) {
            out.push_str(&format!("   {l}\n"));
        }
        if more > 0 {
            out.push_str(&format!("   … and {more} more\n"));
        }
        if lines.is_empty() {
            out.push_str("   (empty)\n");
        }
    }
    out
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else {
        out.push(format!("{}/: can't be read", rel(root, dir)));
        return;
    };
    for e in rd.flatten() {
        if out.len() > LIST_MAX * 2 {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        // a link is followed (models are often linked in), its size the target's
        let Ok(meta) = fs::metadata(&path) else {
            out.push(format!("{}  (a broken link)", rel(root, &path)));
            continue;
        };
        if meta.is_dir() {
            if depth + 1 < LIST_DEPTH {
                walk(root, &path, depth + 1, out);
            } else {
                out.push(format!("{}/  (not listed: deeper than {LIST_DEPTH})", rel(root, &path)));
            }
        } else {
            out.push(format!("{}  {}", rel(root, &path), size(meta.len())));
        }
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

fn size(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.2} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n => format!("{n} bytes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_masked_and_nothing_else() {
        let kobold = "Namespace(admin=True, adminpassword='8a62c0', agent_api_key=None, ssl=None)\n";
        assert_eq!(
            mask_passwords(kobold),
            "Namespace(admin=True, adminpassword='***', agent_api_key=None, ssl=None)\n"
        );
        assert_eq!(mask_line("koboldcpp --adminpassword s3cret --port 1"), "koboldcpp --adminpassword *** --port 1");
        assert_eq!(mask_line("adminpassword=None, password=hunter2)"), "adminpassword=None, password=***)");
        assert_eq!(mask_line("{\"password\": \"x y\", \"a\": 1}"), "{\"password\": \"***\", \"a\": 1}");
        let modules = "Inactive Modules: ApiKeyPassword WebSearchProxy";
        assert_eq!(mask_line(modules), modules, "a word after a name that isn't a --flag is no value");
        assert_eq!(mask_line("--adminpassword *** --x"), "--adminpassword *** --x");
    }

    #[test]
    fn the_zip_has_the_config_the_logs_ends_and_the_model_files() {
        let dir = tempfile::tempdir().unwrap();
        let (conf, logs, models) = (dir.path().join("conf"), dir.path().join("logs"), dir.path().join("models"));
        for d in [&conf, &logs, &models.join("loras")] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(conf.join("config.rsn"), "Config ()").unwrap();
        fs::write(conf.join("config.rsn.unreadable"), "Config { oops").unwrap();
        fs::write(conf.join("other.rsn"), "not this").unwrap();
        let long = format!("{}\nlast line adminpassword='abc'\n", "x".repeat(LOG_TAIL as usize + 10));
        fs::write(logs.join("koboldcpp-k.log"), &long).unwrap();
        fs::write(logs.join("increart.log"), "2026-10-31 10:00:00 error: no such model\n").unwrap();
        fs::write(logs.join("notes.txt"), "not this").unwrap();
        fs::write(models.join("z.gguf"), vec![0u8; 2048]).unwrap();
        fs::write(models.join("loras/drawing.safetensors"), "l").unwrap();
        // a preset validated: the stamps and its report
        let stamps = dir.path().join("state").join(crate::validation::FILE);
        let report = dir.path().join("apitest-1.zip");
        fs::write(&report, "zip").unwrap();
        let stamp = crate::validation::Stamp {
            preset: "Remote".into(),
            fingerprint: "f".into(),
            when: "2026-10-31T17:00:00+01:00".into(),
            outcome: crate::apitest::Outcome::Pass,
            counts: Default::default(),
            servers: vec![],
            suite: String::new(),
            report_sha256: String::new(),
            zip: Some(report),
            seal: String::new(),
        };
        crate::validation::record(&stamps, stamp.sealed()).unwrap();
        let dest = dir.path().join("increart-bug-report.zip");
        let r = Report {
            config: Some(conf.join("config.rsn")),
            logs,
            model_dirs: vec![models.to_string_lossy().into_owned(), "/no/such/dir".into()],
            about: "increart 0.1.0 (abc1234)".into(),
            validations: Some(stamps),
        };
        let size = write(&r, &dest).unwrap();
        assert_eq!(size, fs::metadata(&dest).unwrap().len());

        let mut z = zip::ZipArchive::new(fs::File::open(&dest).unwrap()).unwrap();
        let mut names: Vec<String> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        let t = "increart-bug-report";
        assert_eq!(
            names,
            [
                "about.txt",
                "config/config.rsn",
                "config/config.rsn.unreadable",
                "logs/increart.log",
                "logs/koboldcpp-k.log",
                "model-directories.txt",
                "validation/apitest-1.zip",
                "validation/validated.json"
            ]
            .map(|n| format!("{t}/{n}"))
        );
        let mut read = |n: &str| {
            let mut s = String::new();
            z.by_name(&format!("{t}/{n}")).unwrap().read_to_string(&mut s).unwrap();
            s
        };
        let k = read("logs/koboldcpp-k.log");
        assert!(k.starts_with("[the first "), "{}", &k[..60]);
        assert!(k.ends_with("last line adminpassword='***'\n"));
        let m = read("model-directories.txt");
        assert!(m.contains("   loras/drawing.safetensors  1 bytes\n   z.gguf  2048 bytes\n"), "{m}");
        assert!(m.contains("2. /no/such/dir\n   isn't there"), "{m}");
        let a = read("about.txt");
        assert!(a.starts_with("increart 0.1.0 (abc1234)\nsystem: "), "{a}");
        assert!(a.contains("  logs/increart.log\n"), "{a}");
    }
}
