//! A TCP relay in front of a server that can be cut, as the server stopping
//! looks from outside: connections open close at once (a request waiting
//! reads the end of it) and new ones are refused.

use std::io::{ErrorKind, copy};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Cut {
    pub url: String,
    cut: Arc<AtomicBool>,
    open: Arc<Mutex<Vec<TcpStream>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Cut {
    /// Relays to `upstream` ("http://host:port").
    pub fn start(upstream: &str) -> Cut {
        let to = upstream.trim_start_matches("http://").to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let cut = Arc::new(AtomicBool::new(false));
        let open = Arc::new(Mutex::new(Vec::<TcpStream>::new()));
        let (c, o) = (cut.clone(), open.clone());
        let thread = std::thread::spawn(move || {
            // the listener goes with the thread: after a cut, connects are refused
            while !c.load(Ordering::SeqCst) {
                let client = match listener.accept() {
                    Ok((s, _)) => s,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };
                client.set_nonblocking(false).unwrap();
                let Ok(server) = TcpStream::connect(&to) else { continue };
                o.lock().unwrap().extend([client.try_clone().unwrap(), server.try_clone().unwrap()]);
                for (mut a, mut b) in [(client.try_clone().unwrap(), server.try_clone().unwrap()), (server, client)] {
                    std::thread::spawn(move || {
                        let _ = copy(&mut a, &mut b);
                        let _ = b.shutdown(Shutdown::Write);
                    });
                }
            }
        });
        Cut { url, cut, open, thread: Some(thread) }
    }

    /// Stops relaying: closes every connection and refuses new ones.
    pub fn cut(&mut self) {
        self.cut.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        for s in self.open.lock().unwrap().drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Cut {
    fn drop(&mut self) {
        self.cut();
    }
}
