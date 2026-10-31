//! A stand-in sd-server (increart-core's test fake) for the suite's tests
//! and CI: started as sd-server is (`--listen-ip`, `--listen-port`; other
//! sd-server arguments ignored), it answers `/sdcpp/v1/...` until killed.
//! `--fake-files a,b` is its model list (none: 404), `--fake-steps n` the
//! polls a job takes, `--fake-serial` one job at a time.

#[allow(dead_code)]
#[path = "../../../increart-core/tests/common/fake.rs"]
mod fake;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let host = value("--listen-ip").unwrap_or_else(|| "127.0.0.1".into());
    let Some(port) = value("--listen-port") else {
        eprintln!("fake-sd-server: --listen-port is needed");
        std::process::exit(2);
    };
    let f = fake::Fake::start_on(&format!("{host}:{port}"), None);
    f.with(|s| {
        s.model_files = value("--fake-files").map(|v| v.split(',').map(str::to_string).collect());
        s.steps = value("--fake-steps").and_then(|v| v.parse().ok()).unwrap_or(3);
        s.serial = args.iter().any(|a| a == "--fake-serial");
        s.strict_files = true;
        s.sized = true;
    });
    println!("fake sd-server listening on {}", f.url);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
