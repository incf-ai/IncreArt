// Windows: the release build opens no console window beside its own.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! IncreArt: Qwen-Image 2.1 batch generation and review (docs/DESIGN.md).
//!
//! Settings: `config.rsn` in the app's config folder (`~/.config/increart`
//! on Linux, `%APPDATA%\increart` on Windows, `~/Library/Application
//! Support/increart` on macOS), or the file named by
//! `INCREART_CONFIG`. The command line can skip the start-up screens (see
//! `USAGE`); a screen is only skipped when its argument is valid.

mod app;
mod folderview;
mod history;
mod ideas;
mod logs;
mod markup;
mod pe_settings;
mod presets;
mod project_pane;
mod projects;
mod refpicker;
mod review;
mod rewrite;
mod settings;
mod startup;
mod styles;
mod textures;
mod tiles;
mod tour;
mod tree;
mod widgets;

use std::path::PathBuf;

use eframe::egui;
use increart_core::config::Config;
use increart_gui_core::StartupServers;

/// `increart 0.1.0 (abc1234, 2026-10-29)`: the version, and the commit built
/// from with its date (see build.rs).
pub fn version() -> String {
    let v = env!("CARGO_PKG_VERSION");
    match env!("INCREART_COMMIT") {
        "" => format!("increart {v}"),
        c => format!("increart {v} ({c})"),
    }
}

const USAGE: &str = "\
usage: increart [--library <folder>|last] [--servers <preset>|last | --no-servers]
       increart --version
       increart --check-library <folder>

  --library <folder>   open this library (skips the library screen)
  --library last       the most recently opened library
  --servers <preset>   open this server preset (skips the servers screen;
                       the main window opens while the servers start)
  --servers last       the preset opened last
  --no-servers         continue without servers
  -V, --version        print the version and the commit built from
  --check-library <folder>
                       check a library (every link, image and pixel hash),
                       print what's wrong and exit: 0 if nothing is

A screen is still shown when its argument doesn't work (not a library, no
such preset, sd-server missing), saying why.";

/// The command line's answers for the start-up screens.
#[derive(Default)]
pub struct Startup {
    pub library: Option<String>,
    pub servers: Option<StartupServers>,
}

/// Windows: the release build has no console of its own (it's a GUI app),
/// so before printing it attaches to the one it was started from, if any.
/// Output already redirected (a pipe, a file) goes there as it is.
fn console() {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn AttachConsole(process: u32) -> i32;
        }
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        // SAFETY: no arguments to get wrong; failing (no parent console, or
        // one already attached) leaves things as they were
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// `--check-library`: checks the library at `dir` and exits (1 when
/// something is wrong, 2 when it can't be opened).
fn check_library(dir: &str) -> ! {
    let lib = match increart_core::Library::open(std::path::Path::new(dir)) {
        Ok(lib) => lib,
        Err(e) => {
            eprintln!("increart: {dir}: {e}");
            std::process::exit(2);
        }
    };
    match increart_core::check::check(&lib) {
        Ok(r) => {
            for p in &r.problems {
                println!("{p}");
            }
            println!("{dir}: {}", r.summary());
            std::process::exit(if r.problems.is_empty() { 0 } else { 1 });
        }
        Err(e) => {
            eprintln!("increart: {dir}: {e}");
            std::process::exit(2);
        }
    }
}

fn parse_args() -> Result<Startup, String> {
    let mut s = Startup::default();
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_some() {
        console();
    }
    while let Some(a) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("{flag} needs a value"));
        match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("{}", version());
                std::process::exit(0);
            }
            "--check-library" => check_library(&value("--check-library")?),
            "--library" => s.library = Some(value("--library")?),
            "--servers" => s.servers = Some(StartupServers::Preset(value("--servers")?)),
            "--no-servers" => s.servers = Some(StartupServers::Without),
            _ => return Err(format!("unknown argument: {a}")),
        }
    }
    Ok(s)
}

/// The smallest window: the top bar's tabs and buttons still fit (the
/// server statuses shrink, then hide).
pub const MIN_WINDOW: [f32; 2] = [1000.0, 640.0];

fn main() -> eframe::Result {
    let startup = parse_args().unwrap_or_else(|e| {
        eprintln!("increart: {e}\n\n{USAGE}");
        std::process::exit(2);
    });
    let config_path = std::env::var_os("INCREART_CONFIG").map(PathBuf::from).or_else(Config::default_path);
    // the app's own log (not a tour's: its screenshots' messages aren't the user's)
    if std::env::var_os("INCREART_TOUR").is_none() {
        let config = match (&config_path, std::env::var_os("INCREART_CONFIG").is_some()) {
            (Some(p), true) => format!("{} (INCREART_CONFIG)", p.display()),
            (Some(p), false) => p.display().to_string(),
            (None, _) => "none (no config folder)".into(),
        };
        let args: Vec<String> = std::env::args().skip(1).collect();
        let args = if args.is_empty() { String::new() } else { format!("\narguments: {}", args.join(" ")) };
        increart_core::applog::start(
            &increart_gui_core::runtime::log_dir(),
            &format!("{}\nconfig: {config}{args}", version()),
        );
        increart_core::applog::log_panics();
    }
    let (mut config, warnings) = match &config_path {
        Some(p) => Config::load_or_default(p),
        None => {
            let mut c = Config::default();
            let w = c.normalize();
            (c, w)
        }
    };
    // a release zip's example library: in the startup list until removed
    // from it
    if let Some(lib) = increart_core::config::bundled_example_library() {
        config.offer_example_library(lib.to_string_lossy().into_owned());
    }
    // KoboldCpp named nowhere: found where it usually is (saved with the
    // settings' next save)
    if config.known_koboldcpp().is_none()
        && let Some(b) = increart_core::kobold::find_binary()
    {
        config.koboldcpp = b;
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size(MIN_WINDOW)
            .with_title("IncreArt"),
        ..Default::default()
    };
    eframe::run_native(
        "IncreArt",
        options,
        Box::new(move |cc| {
            add_icon_font(&cc.egui_ctx);
            #[cfg(unix)]
            close_on_signals(&cc.egui_ctx);
            Ok(Box::new(app::App::new(cc, config, config_path, warnings, startup)))
        }),
    )
}

#[cfg(unix)]
static SIGNALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_signal(_: libc::c_int) {
    SIGNALLED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// SIGTERM, SIGINT and SIGHUP close the window as its close button does,
/// so drafts are saved and the servers the app started are stopped.
#[cfg(unix)]
fn close_on_signals(ctx: &egui::Context) {
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only stores to an atomic.
        unsafe { libc::signal(sig, on_signal as *const () as libc::sighandler_t) };
    }
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        while !SIGNALLED.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    });
}

/// Noto Sans CJK SC (SIL OFL 1.1, `assets/fonts/OFL.txt`).
static CJK_FONT: &[u8] = include_bytes!("../assets/fonts/NotoSansCJKsc-Regular.otf");

/// Registers Phosphor as a fallback after the text font, and a CJK font last. egui-phosphor's own
/// `add_to_fonts` targets egui 0.36, so only its font bytes and constants are used.
fn add_icon_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("phosphor".into(), egui::FontData::from_static(egui_phosphor::Variant::Regular.font_bytes()).into());
    if let Some(keys) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        keys.insert(1.min(keys.len()), "phosphor".into());
    }
    // The filled glyphs (same code points), for two-tone icons.
    fonts
        .font_data
        .insert("phosphor-fill".into(), egui::FontData::from_static(egui_phosphor::Variant::Fill.font_bytes()).into());
    fonts.families.insert(egui::FontFamily::Name(widgets::ICON_FILL.into()), vec!["phosphor-fill".into()]);
    // Last in both families: Chinese, Japanese and Korean, as PE sometimes answers in them.
    fonts.font_data.insert("cjk".into(), egui::FontData::from_static(CJK_FONT).into());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("cjk".into());
    }
    ctx.set_fonts(fonts);
}
