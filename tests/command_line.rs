//! Arguments, and what the server says on the way up.

mod common;

use common::{Server, TempDir, get};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long to let servio run before deciding it started rather than stopped.
const STARTS_WITHIN: Duration = Duration::from_secs(5);

/// Runs servio and returns what it said, and whether it is still standing.
///
/// A server that starts does not exit, so waiting for it to finish would wait
/// for ever. Systems disagree about what is refusable — a directory that
/// cannot be read stops the watcher on Linux and does not on macOS — so it is
/// stopped here and reported as running, and the test decides what that means.
fn run(args: &[&str]) -> (String, bool) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_servio"));
    command.args(args);
    run_command(command)
}

/// The same, for a command line of one's own.
fn run_command(mut command: Command) -> (String, bool) {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("could not run servio");

    let deadline = Instant::now() + STARTS_WITHIN;
    let running = loop {
        match child.try_wait().expect("could not wait for servio") {
            Some(status) => break status.success(),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break true;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };

    // The pipes are at their end now, so this cannot block either.
    let mut said = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut said);
    }
    if let Some(mut errors) = child.stderr.take() {
        let _ = errors.read_to_string(&mut said);
    }

    (said, running)
}

#[test]
fn a_directory_that_does_not_exist_names_itself() {
    let (said, ok) = run(&["--dir", "/definitely/not/here"]);

    assert!(!ok);
    assert!(
        said.contains("/definitely/not/here"),
        "the message should say which path failed: {said}"
    );
    assert!(
        said.contains("no such directory"),
        "it should say what is wrong in words: {said}"
    );
    assert!(
        !said.contains("os error"),
        "no numbered errors, please: {said}"
    );
}

#[cfg(unix)]
#[test]
fn a_directory_it_may_not_reach_says_so_plainly() {
    use std::os::unix::fs::PermissionsExt;

    // Closed at the parent: the name below it cannot even be looked up. Closing
    // the directory itself would not do, since it can still be named.
    let dir = TempDir::new("unreachable");
    let inner = dir.join("locked/inner");
    std::fs::create_dir_all(&inner).expect("could not create the directory");
    let locked = dir.join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
        .expect("could not close the directory");

    let (said, still_running) = run(&["--dir", inner.to_str().unwrap()]);
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));

    // Running as root, or on a system that reaches it anyway: nothing to check.
    if still_running {
        return;
    }

    assert!(said.contains("will not let this program read it"), "{said}");
    assert!(
        !said.contains("os error"),
        "no numbered errors, please: {said}"
    );
}

#[test]
fn a_file_is_not_a_directory() {
    let dir = TempDir::new("not-a-dir");
    dir.write("index.html", "<html>hi</html>");

    let (said, ok) = run(&["--dir", dir.join("index.html").to_str().unwrap()]);

    assert!(!ok);
    assert!(said.contains("is not a directory"), "{said}");
}

#[test]
fn reports_the_port_the_system_chose() {
    // Every other test relies on this: `--port 0` must print a usable address.
    let dir = TempDir::new("port-zero");
    dir.write("index.html", "<html>hi</html>");

    let server = Server::start(dir.path(), &[]);
    assert!(server.port > 0);
    assert_eq!(get(server.port, "/").status, 200);
}

#[test]
fn offers_localhost_when_listening_everywhere() {
    let dir = TempDir::new("everywhere");
    dir.write("index.html", "<html>hi</html>");

    // 0.0.0.0 is not an address a browser can open.
    let server = Server::start(dir.path(), &["--host", "0.0.0.0"]);
    assert!(server.said("localhost"));
    assert_eq!(get(server.port, "/").status, 200);
}

// Only Linux binds the whole of 127.0.0.0/8 without being asked.
#[cfg(target_os = "linux")]
#[test]
fn offers_the_real_address_when_it_is_not_localhost() {
    let dir = TempDir::new("other-loopback");
    dir.write("index.html", "<html>hi</html>");

    // 127.0.0.2 is loopback too, but nothing answers there under the name
    // localhost, so the banner has to say 127.0.0.2.
    let server = Server::start(dir.path(), &["--host", "127.0.0.2"]);
    assert!(
        server.said("127.0.0.2"),
        "the banner should point at the address it is listening on:\n{}",
        server.lines().join("\n")
    );
}

#[test]
fn says_what_it_is_doing_on_the_way_up() {
    let dir = TempDir::new("banner");
    dir.write("index.html", "<html>hi</html>");

    let server = Server::start(dir.path(), &[]);

    assert!(server.said("Serving"));
    assert!(server.said("Live reload"));
    assert!(server.said("Single-page app"));
    assert!(server.said("File lists"));
}

#[test]
fn a_banner_not_shown_in_a_terminal_has_no_colours_or_links() {
    // In a file or read by another program, their codes are noise.
    let dir = TempDir::new("plain-banner");
    dir.write("index.html", "<html>hi</html>");

    let server = Server::start(dir.path(), &[]);

    assert!(server.said("Open"));
    assert!(!server.said("\x1b"), "{:?}", server.lines());
}

#[test]
fn warns_when_other_devices_can_list_the_files() {
    // Lists are on by default: fine on this machine, not on a network.
    let dir = TempDir::new("lists-everywhere");
    dir.write("index.html", "<html>hi</html>");

    let reachable = Server::start(dir.path(), &["--host", "0.0.0.0"]);
    assert!(
        reachable.said("other devices can list the files here"),
        "{}",
        reachable.lines().join("\n")
    );

    let turned_off = Server::start(dir.path(), &["--host", "0.0.0.0", "--no-list"]);
    assert!(
        !turned_off.said("other devices can list") && turned_off.said("File lists     : off"),
        "{}",
        turned_off.lines().join("\n")
    );

    let local = Server::start(dir.path(), &[]);
    assert!(
        !local.said("other devices can list"),
        "{}",
        local.lines().join("\n")
    );
}

#[test]
fn says_when_it_is_looking_at_the_files_rather_than_being_told() {
    // Polling notices later and costs more, so the banner should not leave it
    // looking like an ordinary run.
    let dir = TempDir::new("polling-banner");
    dir.write("index.html", "<html>hi</html>");

    let server = Server::start(dir.path(), &["--poll"]);

    assert!(
        server.said("looking at the files"),
        "the banner should say that it is polling:\n{}",
        server.lines().join("\n")
    );
}

#[test]
fn looking_at_the_files_and_not_watching_at_all_is_refused() {
    // Asking for both says two different things about the same thing.
    let dir = TempDir::new("poll-and-no-reload");
    dir.write("index.html", "<html>hi</html>");

    let (said, ok) = run(&[
        "--dir",
        dir.path().to_str().unwrap(),
        "--poll",
        "--no-reload",
    ]);

    assert!(!ok);
    assert!(said.contains("--poll"), "it should say which flags: {said}");
    assert!(said.contains("--no-reload"), "{said}");
}

#[test]
fn ignoring_and_not_watching_at_all_is_refused() {
    // There is nothing to ignore a change from when nothing is watched.
    let dir = TempDir::new("ignore-and-no-reload");
    dir.write("index.html", "<html>hi</html>");

    let (said, ok) = run(&[
        "--dir",
        dir.path().to_str().unwrap(),
        "--ignore",
        "*.log",
        "--no-reload",
    ]);

    assert!(!ok);
    assert!(
        said.contains("--ignore"),
        "it should say which flags: {said}"
    );
    assert!(said.contains("--no-reload"), "{said}");
}

#[test]
fn production_with_polling_or_ignoring_is_refused_whichever_comes_first() {
    // Production watches nothing, so there is nothing to poll or ignore.
    let dir = TempDir::new("production-and-watching");
    dir.write("index.html", "<html>hi</html>");
    let served = dir.path().to_str().unwrap();

    for (flags, other) in [
        (&["--production", "--poll"][..], "--poll"),
        (&["--poll", "--production"][..], "--poll"),
        (&["--production", "--ignore", "*.log"][..], "--ignore"),
        (&["--ignore", "*.log", "--production"][..], "--ignore"),
    ] {
        let mut args = vec!["--dir", served];
        args.extend_from_slice(flags);
        let (said, ok) = run(&args);

        assert!(!ok, "{flags:?} should be refused");
        assert!(
            said.contains("--production") && said.contains(other),
            "{flags:?}: {said}"
        );
    }
}

#[test]
fn production_never_moves_to_another_port() {
    // Whatever sends requests to a finished site expects its port, so a busy
    // one is an error there rather than a quiet move.
    let dir = TempDir::new("production-port");
    dir.write("index.html", "<html>hi</html>");
    // Held here while it is free; otherwise something else already holds it.
    let _held = std::net::TcpListener::bind(("127.0.0.1", 3030));

    let (said, ok) = run(&["--dir", dir.path().to_str().unwrap(), "--production"]);

    assert!(!ok, "{said}");
    assert!(said.contains("port 3030 is already in use"), "{said}");
}

#[test]
fn production_may_be_given_with_the_flags_it_stands_for() {
    let dir = TempDir::new("production-and-its-flags");
    dir.write("index.html", "<html>hi</html>");

    let server = Server::start(dir.path(), &["--no-list", "--production", "--no-reload"]);
    assert_eq!(get(server.port, "/").status, 200);
}

#[test]
fn a_broken_ignore_pattern_is_refused_in_plain_words() {
    let dir = TempDir::new("broken-ignore");
    dir.write("index.html", "<html>hi</html>");

    let (said, ok) = run(&["--dir", dir.path().to_str().unwrap(), "--ignore", "[abc"]);

    assert!(!ok);
    assert!(
        said.contains("cannot ignore \"[abc\": a [ has no ] to close it"),
        "{said}"
    );
}

#[test]
fn a_broken_line_of_the_ignore_file_is_refused_by_its_number() {
    let dir = TempDir::new("broken-ignore-file");
    dir.write("index.html", "<html>hi</html>");
    dir.write(".servioignore", "*.log\n\n[abc\n");

    let (said, ok) = run(&["--dir", dir.path().to_str().unwrap()]);

    assert!(!ok);
    assert!(
        said.contains("cannot ignore \"[abc\", line 3 of .servioignore: a [ has no ] to close it"),
        "{said}"
    );
}

#[test]
fn the_ignore_file_is_left_alone_when_nothing_is_watched() {
    // With no watcher there is nothing for it to say, so a broken one cannot
    // stop a server that would never have read it.
    let dir = TempDir::new("ignore-file-no-reload");
    dir.write("index.html", "<html>hi</html>");
    dir.write(".servioignore", "[abc\n");

    for flag in ["--no-reload", "--production"] {
        let server = Server::start(dir.path(), &[flag]);

        assert!(server.said("Serving"), "{flag}");
        assert!(!server.said("Ignoring"), "{flag}");
    }
}

#[cfg(unix)]
#[test]
fn a_served_directory_that_cannot_be_read_is_not_blamed_on_the_ignore_file() {
    // Nobody wrote a .servioignore here, so naming one would send the reader
    // looking for a file that does not exist.
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("closed-to-us");
    dir.write("index.html", "<html>hi</html>");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o000))
        .expect("could not close the directory");

    let (said, _) = run(&["--dir", dir.path().to_str().unwrap(), "--poll"]);

    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
        .expect("could not open the directory again");

    assert!(!said.contains(".servioignore"), "{said}");
}

#[test]
fn an_ignore_file_that_is_a_directory_is_refused_in_plain_words() {
    let dir = TempDir::new("ignore-file-is-a-directory");
    dir.write("index.html", "<html>hi</html>");
    std::fs::create_dir(dir.join(".servioignore")).expect("could not create the directory");

    let (said, ok) = run(&["--dir", dir.path().to_str().unwrap()]);

    assert!(!ok);
    assert!(said.contains("it is a directory, not a file"), "{said}");
    assert!(!said.contains("os error"), "{said}");
}

#[test]
fn a_mark_at_the_start_of_the_ignore_file_is_not_part_of_a_pattern() {
    // Some editors on Windows write one, and it would otherwise make the
    // first pattern match nothing.
    let dir = TempDir::new("ignore-file-bom");
    dir.write("index.html", "<html>hi</html>");
    dir.write(".servioignore", "\u{feff}*.log\ncache\n");

    let server = Server::start(dir.path(), &[]);
    server.settle();

    let before = server.reloads();
    dir.write("build.log", "compiled");
    server.expect_no_reload(before);
}

#[test]
fn says_how_many_patterns_the_ignore_file_added() {
    let dir = TempDir::new("ignore-file-banner");
    dir.write("index.html", "<html>hi</html>");
    dir.write(".servioignore", "*.log\ncache\n");

    let server = Server::start(dir.path(), &["--ignore", "*.map"]);

    assert!(
        server.said("*.map, 2 patterns in .servioignore"),
        "the banner should say what it is ignoring:\n{}",
        server.lines().join("\n")
    );
}

#[test]
fn says_what_it_is_ignoring_on_the_way_up() {
    let dir = TempDir::new("ignore-banner");
    dir.write("index.html", "<html>hi</html>");

    let server = Server::start(dir.path(), &["--ignore", "*.log", "--ignore", "cache"]);

    assert!(
        server.said("Ignoring") && server.said("*.log, cache"),
        "the banner should list the patterns:\n{}",
        server.lines().join("\n")
    );
}

#[cfg(unix)]
#[test]
fn opens_an_address_a_browser_can_open() {
    use std::os::unix::fs::PermissionsExt;

    // A stand-in for the browser, kept outside the served directory so that
    // what it writes is not taken for a change.
    let dir = TempDir::new("open");
    dir.write("index.html", "<html>hi</html>");
    let outside = TempDir::new("open-browser");
    let browser = outside.join("browser.sh");
    let opened = outside.join("opened");
    outside.write(
        "browser.sh",
        &format!("#!/bin/sh\necho \"$@\" > '{}'\n", opened.display()),
    );
    std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755))
        .expect("could not make the browser runnable");

    // 0.0.0.0 is not an address a browser can open; localhost is.
    let server = Server::start_in(
        dir.path(),
        &["--open", "--host", "0.0.0.0"],
        &[("BROWSER", browser.to_str().unwrap())],
    );

    assert_eq!(
        wait_for_a_line(&opened, &server),
        format!("http://localhost:{}", server.port)
    );
}

/// What the stand-in browser was given. The shell creates the file before
/// writing the line, so an empty file means the browser has not run yet.
fn wait_for_a_line(written: &std::path::Path, server: &Server) -> String {
    let deadline = Instant::now() + STARTS_WITHIN;
    loop {
        if let Ok(line) = std::fs::read_to_string(written)
            && !line.trim().is_empty()
        {
            return line.trim().to_string();
        }
        assert!(
            Instant::now() < deadline,
            "no browser was opened:\n{}",
            server.lines().join("\n")
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[test]
fn a_browser_named_with_its_own_arguments_still_opens() {
    use std::os::unix::fs::PermissionsExt;

    // Options and %s for the address, as the variable is used elsewhere.
    // Handed to the system whole, this named no program and nothing opened.
    let dir = TempDir::new("open-arguments");
    dir.write("index.html", "<html>hi</html>");
    let outside = TempDir::new("open-arguments-browser");
    let browser = outside.join("browser.sh");
    let opened = outside.join("opened");
    outside.write(
        "browser.sh",
        &format!("#!/bin/sh\necho \"$@\" > '{}'\n", opened.display()),
    );
    std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755))
        .expect("could not make the browser runnable");

    let server = Server::start_in(
        dir.path(),
        &["--open"],
        &[(
            "BROWSER",
            &format!("{} --new-window %s", browser.to_str().unwrap()),
        )],
    );

    assert_eq!(
        wait_for_a_line(&opened, &server),
        format!("--new-window http://localhost:{}", server.port)
    );
}

#[cfg(unix)]
#[test]
fn a_browser_that_stops_at_once_with_an_error_is_reported() {
    // `firefox --no-such-flag` prints its usage and stops: started, as far as
    // the system can tell, and not opened at all.
    let dir = TempDir::new("browser-stops");
    dir.write("index.html", "<html>hi</html>");

    let server = Server::start_in(dir.path(), &["--open"], &[("BROWSER", "false")]);
    server.wait_for("Cannot open a browser");

    assert!(
        server.said("false stopped with an error"),
        "{}",
        server.lines().join("\n")
    );
    assert_eq!(get(server.port, "/").status, 200);
}

#[cfg(unix)]
#[test]
fn a_browser_that_stops_at_once_lets_the_next_one_open() {
    use std::os::unix::fs::PermissionsExt;

    // `false` starts and stops with an error; the next on the list should
    // open instead.
    let dir = TempDir::new("browser-list");
    dir.write("index.html", "<html>hi</html>");
    let outside = TempDir::new("browser-list-open");
    let browser = outside.join("browser.sh");
    let opened = outside.join("opened");
    outside.write(
        "browser.sh",
        &format!("#!/bin/sh\necho \"$@\" > '{}'\n", opened.display()),
    );
    std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755))
        .expect("could not make the browser runnable");

    let named = format!("false:{}", browser.display());
    let server = Server::start_in(dir.path(), &["--open"], &[("BROWSER", &named)]);

    assert_eq!(
        wait_for_a_line(&opened, &server),
        format!("http://localhost:{}", server.port)
    );
    assert!(
        !server.said("Cannot open a browser"),
        "one browser opened, so nothing should be said:\n{}",
        server.lines().join("\n")
    );
}

#[cfg(unix)]
#[test]
fn a_browser_that_will_not_open_does_not_stop_the_server() {
    let dir = TempDir::new("no-browser");
    dir.write("index.html", "<html>hi</html>");

    let server = Server::start_in(
        dir.path(),
        &["--open"],
        &[("BROWSER", "/definitely/not/a/browser")],
    );

    server.wait_for("Cannot open a browser");
    assert!(
        server.said("/definitely/not/a/browser"),
        "{}",
        server.lines().join("\n")
    );
    assert!(
        !server.said("os error"),
        "no numbered errors, please:\n{}",
        server.lines().join("\n")
    );
    assert_eq!(get(server.port, "/").status, 200);
}

#[test]
fn a_port_that_was_asked_for_is_never_swapped_for_another() {
    // Something else expects that number: a proxy rule, a service file, a
    // bookmark. Moving quietly would turn a loud failure into a puzzling one.
    let dir = TempDir::new("busy-port");
    dir.write("index.html", "<html>hi</html>");
    let taken = Server::start(dir.path(), &[]);

    let (said, ok) = run(&[
        "--dir",
        dir.path().to_str().unwrap(),
        "--port",
        &taken.port.to_string(),
    ]);

    assert!(!ok);
    assert!(said.contains(&taken.port.to_string()), "{said}");
    assert!(said.contains("already in use"), "{said}");
    assert!(
        !said.contains("os error"),
        "no numbered errors, please: {said}"
    );
}

#[test]
fn the_next_port_is_used_when_none_was_asked_for() {
    let dir = TempDir::new("next-port");
    dir.write("index.html", "<html>hi</html>");

    // Nothing here assumes 3030 itself is free: this machine may well be
    // running the very server that makes the second one step aside.
    let first = Server::start_choosing_a_port(dir.path());
    let second = Server::start_choosing_a_port(dir.path());

    // Higher, not exactly one higher: anything else on this machine may hold
    // the port in between, and stepping past that is the point.
    assert!(
        second.port > first.port,
        "the second server should have stepped up from {}, not taken {}",
        first.port,
        second.port
    );
    assert!(second.said("was busy"), "{}", second.lines().join("\n"));
    assert_eq!(get(second.port, "/").status, 200);
}

#[test]
fn a_port_the_system_will_not_give_us_says_so_plainly() {
    // Ports below 1024 need privileges on Unix, and Windows keeps whole ranges
    // for itself. Either way the answer has to be readable.
    let Err(refused) = std::net::TcpListener::bind(("127.0.0.1", 80)) else {
        return; // This machine allows it, so there is nothing to test.
    };
    if refused.kind() != std::io::ErrorKind::PermissionDenied {
        return; // Something is simply listening there.
    }

    let dir = TempDir::new("forbidden-port");
    dir.write("index.html", "<html>hi</html>");

    let (said, ok) = run(&["--dir", dir.path().to_str().unwrap(), "--port", "80"]);

    assert!(!ok);
    assert!(said.contains("80"), "it should say which port: {said}");
    assert!(said.contains("--port"), "it should say what to do: {said}");
    assert!(
        !said.contains("os error"),
        "no numbered errors, please: {said}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn running_out_of_open_files_is_said_in_plain_words() {
    // A dozen are not enough to watch with. Each editor and dev server holds
    // a watcher too, so the limit is reached in ordinary use. Which step runs
    // out first, the port or the watcher, depends on how many files were open
    // to begin with, so several limits are tried and each has to be explained.
    let dir = TempDir::new("few-files");
    dir.write("index.html", "<html>hi</html>");
    let mut explained = 0;

    for limit in 9..=12 {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!(
                r#"ulimit -n {limit} && exec "$0" --dir "$1" --port 0"#
            ))
            .arg(env!("CARGO_BIN_EXE_servio"))
            .arg(dir.path());

        let (said, still_running) = run_command(command);

        // Enough after all, or too few even to start the runtime: nothing to
        // check either way.
        if still_running || said.contains("Runtime") {
            continue;
        }

        assert!(
            !said.contains("os error"),
            "no numbered errors, please: {said}"
        );
        assert!(said.contains("no open files left"), "{said}");
        explained += 1;
    }

    assert!(explained > 0, "no limit between 9 and 12 ran out");
}

#[cfg(unix)]
#[test]
fn a_directory_below_that_it_may_not_read_is_named_along_with_a_way_round() {
    use std::os::unix::fs::PermissionsExt;

    // The served directory is fine; one below it is closed. That is the one
    // to name, and there are two ways to serve regardless.
    let dir = TempDir::new("closed-below");
    dir.write("index.html", "<html>hi</html>");
    let locked = dir.join("locked");
    std::fs::create_dir_all(&locked).expect("could not create the directory");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
        .expect("could not close the directory");

    let (said, still_running) = run(&["--dir", dir.path().to_str().unwrap()]);
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));

    // macOS watches a directory it cannot read, and root reads it outright.
    if still_running {
        return;
    }

    assert!(said.contains("locked for changes"), "{said}");
    assert!(said.contains("will not let this program read it"), "{said}");
    assert!(
        said.contains("--poll") && said.contains("--no-reload"),
        "no way round was offered: {said}"
    );
}

#[cfg(unix)]
#[test]
fn a_directory_it_may_not_read_says_so_plainly() {
    use std::os::unix::fs::PermissionsExt;

    // This one can be named, so it gets past the first check and fails in the
    // watcher instead — which used to answer with a number and a list of paths.
    let dir = TempDir::new("closed");
    let closed = dir.join("closed");
    std::fs::create_dir_all(&closed).expect("could not create the directory");
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000))
        .expect("could not close the directory");

    let (said, still_running) = run(&["--dir", closed.to_str().unwrap()]);
    let _ = std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755));

    // macOS watches a directory it cannot read, and root reads it outright.
    // Where the watcher does refuse, it has to say so in words.
    if still_running {
        return;
    }

    assert!(said.contains("will not let this program read it"), "{said}");
    assert!(
        !said.contains("os error"),
        "no numbered errors, please: {said}"
    );
    assert!(
        !said.contains("about ["),
        "the watcher's own wording leaked through: {said}"
    );
}

#[test]
fn a_server_that_starts_is_not_waited_on_for_ever() {
    // The macOS build hung here once. A directory Linux refuses to watch is
    // watched happily there, so servio stayed up, and a helper that waited for
    // it to exit waited until the job was killed twenty minutes later. No test
    // may depend on servio choosing to stop.
    let dir = TempDir::new("stays-up");
    dir.write("index.html", "<html>hi</html>");

    let began = Instant::now();
    let (said, still_running) = run(&["--dir", dir.path().to_str().unwrap(), "--port", "0"]);

    assert!(still_running, "servio should have stayed up: {said}");
    assert!(
        began.elapsed() < STARTS_WITHIN * 3,
        "waited {:?}, which is not a bound at all",
        began.elapsed()
    );
    assert!(said.contains("Serving"), "no banner: {said}");
}
