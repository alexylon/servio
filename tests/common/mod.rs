//! Starts the real binary on a temporary directory and talks HTTP to it.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
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
        // A test that closed a directory to see what the server says leaves
        // one nothing can remove. Opened again, so the run leaves nothing
        // behind in the system's temporary directory.
        if std::fs::remove_dir_all(&self.path).is_err() {
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

/// Only tests on Unix close directories, so elsewhere there is nothing to open.
#[cfg(not(unix))]
fn open_again(_path: &Path) {}

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

    /// Opens the site in a browser that counts the refreshes the server sends
    /// it from now on. What the server prints is not a count of those: some
    /// refreshes are made without a line.
    pub fn open_browser(&self) -> Browser<'_> {
        Browser::open(self)
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

/// What a page's script asks for to hear about refreshes.
const EVENT_STREAM: &[u8] =
    b"GET /_tower-livereload/event-stream HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";

/// A page open in a browser, as far as refreshing goes: it listens for the
/// server's refresh events and counts them. After each one it connects again,
/// as the reloaded page would, and like that page it misses a refresh sent
/// while it does.
pub struct Browser<'a> {
    server: &'a Server,
    seen: Arc<Seen>,
    listener: Option<std::thread::JoinHandle<()>>,
}

/// What the browser has heard, shared with the thread that listens.
#[derive(Default)]
struct Seen {
    refreshes: AtomicUsize,
    listening: AtomicBool,
    closed: AtomicBool,
}

impl<'a> Browser<'a> {
    fn open(server: &'a Server) -> Browser<'a> {
        let seen = Arc::new(Seen::default());
        let port = server.port;
        let listener = std::thread::spawn({
            let seen = Arc::clone(&seen);
            move || listen(port, &seen)
        });

        let browser = Browser {
            server,
            seen,
            listener: Some(listener),
        };
        // The server's first event means it will pass on the next refresh, so
        // none is missed from here on.
        server.wait_until(
            || browser.listening(),
            "the browser to listen for refreshes",
        );
        browser
    }

    pub fn refreshes(&self) -> usize {
        self.seen.refreshes.load(Ordering::SeqCst)
    }

    fn listening(&self) -> bool {
        self.seen.listening.load(Ordering::SeqCst)
    }

    /// Waits for this many refreshes in all, and for the browser to listen
    /// again after the last.
    pub fn wait_for_refreshes(&self, wanted: usize) {
        self.server.wait_until(
            || self.refreshes() >= wanted && self.listening(),
            &format!("{wanted} refresh(es)"),
        );
    }

    /// Waits long enough for a refresh to have come, then insists none came
    /// since the browser was opened.
    pub fn expect_no_refresh(&self) {
        self.expect_no_refresh_within(SETTLE);
    }

    /// The same, where the server needs longer to have noticed at all: a poll
    /// hears nothing until its next look.
    pub fn expect_no_refresh_within(&self, patience: Duration) {
        std::thread::sleep(patience);
        assert_eq!(
            self.refreshes(),
            0,
            "nothing should have refreshed:\n{}",
            self.server.lines().join("\n")
        );
        // A server that stopped would send no refresh either, and pass.
        assert!(
            self.listening(),
            "the browser stopped hearing from the server:\n{}",
            self.server.lines().join("\n")
        );
    }
}

impl Drop for Browser<'_> {
    fn drop(&mut self) {
        self.seen.closed.store(true, Ordering::SeqCst);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

/// Listens for the server's events until the browser is closed. The server
/// ends the stream after each refresh, so this connects again every time.
fn listen(port: u16, seen: &Seen) {
    while !seen.closed.load(Ordering::SeqCst) {
        let Ok(mut socket) = TcpStream::connect(("127.0.0.1", port)) else {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        // Short, so a closed browser stops soon.
        let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));
        if socket.write_all(EVENT_STREAM).is_err() {
            continue;
        }

        let mut received = Vec::new();
        let mut chunk = [0; 512];
        while !seen.closed.load(Ordering::SeqCst) {
            match socket.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => received.extend_from_slice(&chunk[..read]),
                Err(error) if would_block(&error) => continue,
                Err(_) => break,
            }

            let events = String::from_utf8_lossy(&received);
            if events.contains("event: reload") {
                // Not listening first, so a test waiting for both never sees
                // the new count while this connection still looks open.
                seen.listening.store(false, Ordering::SeqCst);
                seen.refreshes.fetch_add(1, Ordering::SeqCst);
                break;
            }
            if events.contains("event: init") {
                seen.listening.store(true, Ordering::SeqCst);
            }
        }
        seen.listening.store(false, Ordering::SeqCst);
    }
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
