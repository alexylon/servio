//! Starts the real binary on a temporary directory and talks HTTP to it.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long to wait before deciding that something did *not* happen. The
/// server groups file events for 200 ms, so this leaves room to spare.
pub const SETTLE: Duration = Duration::from_millis(700);

/// How long to wait for something that should happen.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// What a browser sends when it asks for a page.
pub const PAGE: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(name: &str) -> TempDir {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "servio-test-{}-{name}-{unique}",
            std::process::id()
        ));

        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("could not create the test directory");
        TempDir { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }

    pub fn write(&self, relative: &str, contents: &str) {
        let file = self.join(relative);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("could not create a parent directory");
        }
        std::fs::write(file, contents).expect("could not write the test file");
    }

    pub fn remove_all(&self) {
        std::fs::remove_dir_all(&self.path).expect("could not remove the test directory");
    }

    pub fn create(&self) {
        std::fs::create_dir_all(&self.path).expect("could not create the test directory");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if std::fs::remove_dir_all(&self.path).is_ok() {
            return;
        }

        // A test that closed a directory to see what the server says leaves
        // one nothing can remove. Opened again, so the run leaves nothing
        // behind in the system's temporary directory.
        #[cfg(unix)]
        {
            open_again(&self.path);
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Opens `path` and everything below it, as far as it can be read.
#[cfg(unix)]
fn open_again(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        // The kind of the entry itself, not of what a link leads to: the tests
        // make one that leads to a directory above, and following it would
        // walk without end.
        let is_directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if is_directory {
            open_again(&entry.path());
        }
    }
}

pub struct Server {
    child: Child,
    pub port: u16,
    log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    /// Starts the server on a port the system chooses, and waits until it is
    /// listening. Everything it prints is collected for the assertions below.
    pub fn start(dir: &Path, args: &[&str]) -> Server {
        Server::spawn(dir, &["--port", "0"], args, &[])
    }

    /// The same, with these variables in the server's environment.
    pub fn start_in(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Server {
        Server::spawn(dir, &["--port", "0"], args, env)
    }

    /// Says nothing about the port, so the server picks one itself.
    pub fn start_choosing_a_port(dir: &Path) -> Server {
        Server::spawn(dir, &[], &[], &[])
    }

    fn spawn(dir: &Path, port: &[&str], args: &[&str], env: &[(&str, &str)]) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_servio"))
            .arg("--dir")
            .arg(dir)
            .args(port)
            .args(args)
            .envs(env.iter().copied())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("could not start servio");

        let log = Arc::new(Mutex::new(Vec::new()));
        let (found_port, port) = mpsc::channel();

        let stdout = child.stdout.take().expect("no stdout");
        collect(stdout, Arc::clone(&log), Some(found_port));

        let stderr = child.stderr.take().expect("no stderr");
        collect(stderr, Arc::clone(&log), None);

        let Ok(port) = port.recv_timeout(TIMEOUT) else {
            let stopped = match child.try_wait() {
                Ok(Some(status)) => format!("it stopped with {status}"),
                _ => "it is still running".to_string(),
            };

            panic!(
                "servio never said which port it was listening on, {stopped}:\n{}",
                log.lock().unwrap().join("\n")
            );
        };

        Server { child, port, log }
    }

    pub fn lines(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    pub fn count(&self, needle: &str) -> usize {
        self.lines()
            .iter()
            .filter(|line| line.contains(needle))
            .count()
    }

    pub fn said(&self, needle: &str) -> bool {
        self.count(needle) > 0
    }

    /// Every announcement that makes a connected browser refresh.
    pub fn reloads(&self) -> usize {
        self.count("File changed")
            + self.count("Directory replaced")
            + self.count("Directory back")
            + self.count("Watching again")
    }

    pub fn wait_for_reloads(&self, wanted: usize) {
        self.wait_until(
            || self.reloads() >= wanted,
            &format!("{wanted} reload(s), saw {}", self.reloads()),
        );
    }

    pub fn wait_for(&self, needle: &str) {
        self.wait_until(|| self.said(needle), needle);
    }

    /// For something said more than once, where waiting for the first would
    /// return straight away.
    pub fn wait_for_count(&self, needle: &str, wanted: usize) {
        // No count here: this message is built before the waiting starts, so
        // it would be stale. The lines it did see come with the panic.
        self.wait_until(
            || self.count(needle) >= wanted,
            &format!("{needle} {wanted} time(s)"),
        );
    }

    fn wait_until(&self, done: impl Fn() -> bool, wanted: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        panic!("waited for {wanted}:\n{}", self.lines().join("\n"));
    }

    /// Waits long enough for a reload to have been announced, then insists
    /// none was.
    pub fn expect_no_reload(&self, before: usize) {
        self.expect_no_reload_within(before, SETTLE);
    }

    /// The same, where the server needs longer to have noticed at all: a poll
    /// hears nothing until its next look.
    pub fn expect_no_reload_within(&self, before: usize, patience: Duration) {
        std::thread::sleep(patience);
        assert_eq!(
            self.reloads(),
            before,
            "nothing should have reloaded:\n{}",
            self.lines().join("\n")
        );
    }

    /// Gives the watcher, or whatever the last request had to say, a moment
    /// to arrive before a test starts counting.
    pub fn settle(&self) {
        std::thread::sleep(SETTLE);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn collect(
    stream: impl Read + Send + 'static,
    log: Arc<Mutex<Vec<String>>>,
    found_port: Option<mpsc::Sender<u16>>,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            let port = found_port.as_ref().zip(port_in(&line));
            log.lock().unwrap().push(line);

            // Announced last, so the whole banner is already readable by the
            // time a test is handed the port.
            if let Some((sender, port)) = port {
                let _ = sender.send(port);
            }
        }
    });
}

/// Reads the port out of the banner's "Open" line, which is printed once the
/// server is listening.
fn port_in(line: &str) -> Option<u16> {
    let (label, address) = line.split_once(':')?;
    if !label.contains("Open") {
        return None;
    }

    address.trim().rsplit(':').next()?.trim().parse().ok()
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub fn get(port: u16, path: &str) -> Response {
    request(port, "GET", path, &[])
}

/// A request that looks like a browser opening a page, rather than fetching
/// a script or an image.
pub fn get_page(port: u16, path: &str) -> Response {
    request(port, "GET", path, &[("Accept", PAGE)])
}

pub fn request(port: u16, method: &str, path: &str, headers: &[(&str, &str)]) -> Response {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("could not connect");
    socket.set_read_timeout(Some(TIMEOUT)).unwrap();

    // Closing the connection after the answer means reading to the end is
    // enough; there is no second response to worry about.
    let mut lines = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (header, value) in headers {
        lines.push_str(&format!("{header}: {value}\r\n"));
    }
    lines.push_str("\r\n");

    socket.write_all(lines.as_bytes()).expect("could not send");
    let mut raw = Vec::new();
    socket.read_to_end(&mut raw).expect("could not read");
    parse(&raw)
}

/// Holds a live reload connection open and reports whether the server asks it
/// to refresh within `TIMEOUT`.
pub fn watches_for_reload(port: u16) -> std::thread::JoinHandle<bool> {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("could not connect");
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    socket
        .write_all(
            b"GET /_tower-livereload/event-stream HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        )
        .expect("could not send");

    std::thread::spawn(move || {
        let deadline = Instant::now() + TIMEOUT;
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            let mut chunk = [0; 512];
            match socket.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => seen.extend_from_slice(&chunk[..read]),
                Err(error) if would_block(&error) => continue, // simply quiet
                Err(_) => break,                               // the connection broke
            }
            if String::from_utf8_lossy(&seen).contains("event: reload") {
                return true;
            }
        }
        false
    })
}

fn would_block(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn parse(raw: &[u8]) -> Response {
    let split = find(raw, b"\r\n\r\n").expect("no end of headers");
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.lines();

    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("no status");

    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(header, value)| (header.trim().to_string(), value.trim().to_string()))
        .collect();

    let body = &raw[split + 4..];
    let chunked = headers.iter().any(|(header, value)| {
        header.eq_ignore_ascii_case("transfer-encoding") && value.contains("chunked")
    });

    Response {
        status,
        headers,
        body: if chunked {
            dechunk(body)
        } else {
            body.to_vec()
        },
    }
}

/// The reload script is added as the page streams, so answers arrive in
/// chunks, each introduced by its length in hexadecimal.
fn dechunk(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = body;

    while let Some(end_of_size) = find(rest, b"\r\n") {
        let size = String::from_utf8_lossy(&rest[..end_of_size]);
        let size = size.split(';').next().unwrap_or_default().trim();
        let Ok(size) = usize::from_str_radix(size, 16) else {
            break;
        };
        let start = end_of_size + 2;
        let Some(chunk) = rest.get(start..start + size) else {
            break; // the answer was cut short
        };
        if size == 0 {
            break;
        }

        out.extend_from_slice(chunk);
        rest = rest.get(start + size + 2..).unwrap_or_default();
    }

    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
