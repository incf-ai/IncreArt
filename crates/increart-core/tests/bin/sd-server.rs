//! A stand-in for sd-server (tests/client.rs): on `--listen-port`, answers
//! every request with `{"argv": [...]}`, its arguments; with `--exit`, says
//! "bad device" and exits 3; with `--crash-after <ms>`, answers until then,
//! says "generating" and a progress bar, and aborts (as a crash).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--exit") {
        println!("bad device");
        std::process::exit(3);
    }
    let port = args.iter().position(|a| a == "--listen-port").map(|i| &args[i + 1]).expect("--listen-port");
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
    println!("listening");
    if let Some(i) = args.iter().position(|a| a == "--crash-after") {
        let ms: u64 = args[i + 1].parse().expect("--crash-after <ms>");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            println!("generating");
            print!("  |=>    |  1/35\r");
            std::io::stdout().flush().ok();
            std::process::abort();
        });
    }
    let body = serde_json::json!({ "argv": args }).to_string();
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        // the request line and headers; any body is ignored
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
            line.clear();
        }
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body.as_bytes())).ok();
    }
}
