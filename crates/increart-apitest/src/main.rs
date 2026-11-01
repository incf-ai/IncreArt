//! `increart-apitest`: validates the servers of an IncreArt config (see
//! the library's docs, and DESIGN.md, Validating servers).

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use increart_core::apitest::Event;
use increart_core::config::{Config, ServerConfig, ServerPreset};
use increart_core::kobold::{KoboldRun, KoboldServer};
use increart_core::pe::{PeRun, PeServer};

const USAGE: &str = "\
usage: increart-apitest list [<options>] [--json]
       increart-apitest run [<options>] [--cases <pattern>,…] [--case <pattern>] [--light | --basic]
                            [--out <folder>]
                            [--no-stamp] [--jsonl] [--verbose]
       increart-apitest --version

Validates the servers of an IncreArt config: each server preset's servers
are opened together, as the app opens them (those it starts, started by
the suite: close the app's first), then tested case by case. Every request
and its answer is logged; the report (report.md: hand it to a person or an
LLM) and the started servers' logs are zipped beside the --out folder.

options:
  --config <file>     the IncreArt config (default: $INCREART_CONFIG, else the app's)
  --preset <name>     only this server preset (again for more; default: all)
  --url <url> --kind sd-server|koboldcpp|chat [--password <p>]
                      test one running server instead of a config's presets
                      (the config's model presets are used to load and generate)

run:
  --cases <patterns>  only these case IDs, comma-separated; * for any text
                      (\"*/jobs.*\", \"Remote/proxy/*\"); `list` shows the IDs
  --case <pattern>    one case ID or pattern, taken whole (again for more): for IDs
                      whose preset name has a comma
  --light             only the light cases: nothing loaded, nothing generated
  --basic             only the basic cases: each server answers, lists its models and
                      makes a small image, a chat server answers, and the servers
                      together route each model preset and run through the engine
  --no-stamp          don't note the result: a preset whose basic cases all ran is
                      noted as validated (or not) for its settings, which the app shows
  --out <folder>      the report's folder (default: increart-apitest-<date-time>)
  --jsonl             events as JSON lines (for the app); a `stop` line on stdin stops
  --verbose           each case's log and traffic as it runs

exit status: 0 all passed, 1 a case failed, 2 warnings only, 3 couldn't run.
";

struct Args {
    command: String,
    config: Option<PathBuf>,
    presets: Vec<String>,
    url: Option<String>,
    kind: Option<String>,
    password: String,
    cases: Vec<String>,
    light: bool,
    basic: bool,
    no_stamp: bool,
    out: Option<PathBuf>,
    json: bool,
    jsonl: bool,
    verbose: bool,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        command: String::new(),
        config: None,
        presets: vec![],
        url: None,
        kind: None,
        password: String::new(),
        cases: vec![],
        light: false,
        basic: false,
        no_stamp: false,
        out: None,
        json: false,
        jsonl: false,
        verbose: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--version" | "-V" => a.command = "version".into(),
            "--help" | "-h" => a.command = "help".into(),
            "list" | "run" if a.command.is_empty() => a.command = arg,
            "--config" => a.config = Some(value("--config")?.into()),
            "--preset" => a.presets.push(value("--preset")?),
            "--url" => a.url = Some(value("--url")?),
            "--kind" => a.kind = Some(value("--kind")?),
            "--password" => a.password = value("--password")?,
            "--cases" => {
                a.cases.extend(value("--cases")?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
            }
            "--case" => a.cases.push(value("--case")?),
            "--light" => a.light = true,
            "--basic" => a.basic = true,
            "--no-stamp" => a.no_stamp = true,
            "--out" => a.out = Some(value("--out")?.into()),
            "--json" => a.json = true,
            "--jsonl" => a.jsonl = true,
            "--verbose" | "-v" => a.verbose = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.command.is_empty() {
        return Err("say list or run".into());
    }
    Ok(a)
}

/// The config, where it came from, the presets to test, and the config's
/// warnings.
type Target = (Config, Option<PathBuf>, Vec<ServerPreset>, Vec<String>);

/// The config and the presets to test.
fn target(a: &Args) -> Result<Target, String> {
    let path = a
        .config
        .clone()
        .or_else(|| std::env::var_os("INCREART_CONFIG").map(PathBuf::from))
        .or_else(Config::default_path);
    let (config, warnings) = match &path {
        Some(p) if p.exists() => Config::load_or_default(p),
        Some(p) if a.url.is_none() => return Err(format!("no config at {}", p.display())),
        _ => {
            let mut c = Config::default();
            let w = c.normalize();
            (c, w)
        }
    };
    increart_core::config::set_timeouts(config.timeouts);
    if let Some(url) = &a.url {
        let server = match a.kind.as_deref().unwrap_or("sd-server") {
            "sd-server" => ServerConfig::Connect { name: "sd-server".into(), url: url.clone(), device: String::new() },
            "koboldcpp" => ServerConfig::Kobold(KoboldServer {
                name: "koboldcpp".into(),
                run: KoboldRun::Connect { url: url.clone(), password: a.password.clone(), device: String::new() },
                chat: None,
                swap: false,
            }),
            "chat" => ServerConfig::Chat(PeServer {
                name: "chat".into(),
                run: PeRun::Connect { url: url.clone() },
                ..Default::default()
            }),
            k => return Err(format!("--kind {k}: say sd-server, koboldcpp or chat")),
        };
        let preset = ServerPreset { name: "url".into(), servers: vec![server] };
        return Ok((config, path, vec![preset], warnings));
    }
    let presets: Vec<ServerPreset> = if a.presets.is_empty() {
        config.server_presets.clone()
    } else {
        let mut v = vec![];
        for name in &a.presets {
            match config.preset(name) {
                Some(p) => v.push(p.clone()),
                None => {
                    let names: Vec<&str> = config.server_presets.iter().map(|p| p.name.as_str()).collect();
                    return Err(format!("no server preset {name} (the config has: {})", names.join(", ")));
                }
            }
        }
        v
    };
    if presets.is_empty() {
        return Err("the config has no server presets".into());
    }
    Ok((config, path, presets, warnings))
}

fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("increart-apitest: {e}\n\n{USAGE}");
            return ExitCode::from(3);
        }
    };
    match a.command.as_str() {
        "version" => {
            println!("{}", increart_apitest::version());
            return ExitCode::SUCCESS;
        }
        "help" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let (config, path, presets, warnings) = match target(&a) {
        Ok(t) => t,
        Err(e) => {
            if a.jsonl || a.json {
                println!("{}", serde_json::to_string(&Event::Error { text: e.clone() }).unwrap_or_default());
            }
            eprintln!("increart-apitest: {e}");
            return ExitCode::from(3);
        }
    };
    if a.command == "list" {
        let cases = increart_apitest::plan(&config, &presets);
        // written, not printed: a reader that stops early (| head) is no panic
        let mut so = std::io::stdout().lock();
        if a.json {
            let _ = writeln!(so, "{}", serde_json::to_string(&Event::Plan { cases }).unwrap_or_default());
        } else {
            for c in cases {
                let na = c.not_applicable.map(|w| format!("  (n/a: {w})")).unwrap_or_default();
                if writeln!(so, "{:<60} {:<14} {}{na}", c.id, c.cost.label(), c.title).is_err() {
                    break;
                }
            }
        }
        return ExitCode::SUCCESS;
    }
    // a pattern that picks nothing is a mistake (or a split ID), not a pass
    let all = increart_apitest::plan(&config, &presets);
    let unmatched: Vec<&str> = a
        .cases
        .iter()
        .filter(|p| !all.iter().any(|c| increart_core::apitest::matches(p, &c.id)))
        .map(String::as_str)
        .collect();
    if !unmatched.is_empty() {
        let e = format!("no case matches {} (`list` shows the IDs)", unmatched.join(", "));
        if a.jsonl {
            println!("{}", serde_json::to_string(&Event::Error { text: e.clone() }).unwrap_or_default());
        }
        eprintln!("increart-apitest: {e}");
        return ExitCode::from(3);
    }
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("increart-apitest-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"))));
    let out = std::path::absolute(&out).unwrap_or(out);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        // a `stop` line (the app's Stop), or stdin closing in --jsonl
        let (cancel, jsonl) = (cancel.clone(), a.jsonl);
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                match line {
                    Ok(l) if l.trim() == "stop" => break,
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            if jsonl {
                cancel.store(true, Ordering::Relaxed);
            }
        });
    }
    let (jsonl, verbose) = (a.jsonl, a.verbose);
    let emit: Box<dyn FnMut(&Event) + Send> = Box::new(move |e| {
        let mut so = std::io::stdout().lock();
        if jsonl {
            let _ = writeln!(so, "{}", serde_json::to_string(e).unwrap_or_default());
            let _ = so.flush();
            return;
        }
        match e {
            Event::Start { id } if verbose => {
                let _ = writeln!(so, "… {id}");
            }
            Event::End { id, outcome, summary, secs } => {
                let _ = writeln!(so, "{:>8}  {id}  {summary} ({secs:.1} s)", outcome.label());
            }
            Event::Log { id: None, text } => {
                let _ = writeln!(so, "{text}");
            }
            Event::Log { text, .. } if verbose => {
                let _ = writeln!(so, "            {text}");
            }
            Event::Traffic { text, .. } if verbose => {
                for l in text.lines() {
                    let _ = writeln!(so, "            | {l}");
                }
            }
            Event::Done { counts, report, zip } => {
                let _ = writeln!(so, "\n{}", counts.line());
                if let Some(r) = report {
                    let _ = writeln!(so, "report: {}", r.join("report.md").display());
                }
                if let Some(z) = zip {
                    let _ = writeln!(so, "zipped: {}", z.display());
                }
            }
            _ => {}
        }
    });
    // a --url server isn't a preset of the config: nothing to note
    let stamps = (!a.no_stamp && a.url.is_none()).then(increart_core::validation::path).flatten();
    let opts = increart_apitest::Options {
        config,
        config_path: path,
        presets,
        cases: a.cases,
        light: a.light,
        basic: a.basic,
        stamps,
        out,
    };
    // the config's own warnings first
    let mut emit = emit;
    for w in warnings {
        emit(&Event::Log { id: None, text: format!("config: {w}") });
    }
    let counts = increart_apitest::run(&opts, &cancel, emit);
    if counts.fail > 0 {
        ExitCode::from(1)
    } else if counts.warn > 0 {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}
