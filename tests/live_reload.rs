//! What does and does not make the browser refresh.

mod common;

use common::{Server, TempDir, get};
use std::time::{Duration, Instant};

/// Long enough for a poll to have had a look, and for the debounce to have
/// grouped what it found.
const A_LOOK: Duration = Duration::from_millis(2500);

/// Closes a directory to this program, which is how a look comes to fail on a
/// path that is still there.
#[cfg(unix)]
fn close_to_us(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
        .expect("could not close the directory");
}

fn site(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    dir.write("index.html", "<html>first</html>");
    dir.write("app.css", "body {}");
    dir
}

#[test]
fn saving_a_file_refreshes_the_browser() {
    let dir = site("save");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let browser = server.open_browser();
    dir.write("app.css", "body { color: red }");
    browser.wait_for_refreshes(1);
}

#[test]
fn a_burst_of_writes_refreshes_once() {
    // One save can write several files, and a build writes many at once. A
    // build that goes on writing for a while is tested in src/refresh.rs,
    // where no busy machine can stretch the pauses between its writes.
    let dir = site("burst");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let browser = server.open_browser();
    for part in 0..5 {
        dir.write(&format!("part-{part}.css"), "body {}");
    }
    browser.wait_for_refreshes(1);
    server.settle();

    assert_eq!(
        browser.refreshes(),
        1,
        "a burst of writes refreshed more than once:\n{}",
        server.lines().join("\n")
    );
}

#[test]
fn saving_a_file_refreshes_the_browser_while_polling() {
    // Where the system reports nothing, the look has to find the change itself.
    let dir = site("save-polling");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    let browser = server.open_browser();
    dir.write("app.css", "body { color: red }");
    browser.wait_for_refreshes(1);
}

#[test]
fn while_polling_the_ignored_files_are_still_ignored() {
    // A directory's write time moves for a swap file as readily as for a page,
    // and counting that refreshed the browser for every scratch file an editor
    // wrote.
    let dir = site("scratch-polling");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    let browser = server.open_browser();
    dir.write("index.html.swp", "vim");
    dir.write("node_modules/left-pad/index.js", "module.exports = 1");

    browser.expect_no_refresh_within(A_LOOK);
}

#[cfg(unix)]
#[test]
fn while_polling_a_path_it_cannot_read_is_not_a_broken_watch() {
    // A look meets the same closed directory every time. Treated as a watch
    // that had gone down, it set the watch up again once a second and refreshed
    // the browser every time round.
    let dir = site("unreadable-polling");
    let closed = dir.join("closed");
    std::fs::create_dir_all(&closed).expect("could not create the directory");
    close_to_us(&closed);

    // Running as root, which reads it anyway: nothing to check.
    if std::fs::read_dir(&closed).is_ok() {
        return;
    }

    let server = Server::start(dir.path(), &["--poll"]);
    let browser = server.open_browser();
    browser.expect_no_refresh_within(A_LOOK);

    // And it is worth saying once, by name, not once a second.
    assert_eq!(
        server.count("Cannot look at closed"),
        1,
        "the same problem was said over and over:\n{}",
        server.lines().join("\n")
    );
    assert!(
        !server.said("Cannot watch for changes"),
        "one unreadable path is not a watch that went down:\n{}",
        server.lines().join("\n")
    );

    // What must not be lost: a real change is still found.
    dir.write("app.css", "body { color: red }");
    browser.wait_for_refreshes(1);
}

#[cfg(unix)]
#[test]
fn while_polling_an_unreadable_path_the_browser_never_sees_is_not_worth_saying() {
    // Nothing under node_modules can change what the browser shows, so a
    // complaint about it would send people after a fault that is not there.
    let dir = site("ignored-unreadable-polling");
    let closed = dir.join("node_modules/.cache");
    std::fs::create_dir_all(&closed).expect("could not create it");
    close_to_us(&closed);

    if std::fs::read_dir(&closed).is_ok() {
        return;
    }

    let server = Server::start(dir.path(), &["--poll"]);
    let browser = server.open_browser();
    browser.expect_no_refresh_within(A_LOOK);

    assert!(
        !server.said("Cannot"),
        "an ignored path was complained about:\n{}",
        server.lines().join("\n")
    );
}

#[cfg(unix)]
#[test]
fn while_polling_a_link_that_leads_nowhere_is_not_a_fault() {
    // Build output is full of these. Links are not followed, so there is
    // nothing to report.
    let dir = site("dangling-polling");
    std::os::unix::fs::symlink("/nowhere", dir.join("broken")).expect("could not link");

    let server = Server::start(dir.path(), &["--poll"]);
    let browser = server.open_browser();
    browser.expect_no_refresh_within(A_LOOK);

    assert!(
        !server.said("Cannot"),
        "a link that leads nowhere was reported as a fault:\n{}",
        server.lines().join("\n")
    );
}

#[cfg(unix)]
#[test]
fn while_polling_a_link_to_a_directory_above_is_not_a_walk_with_no_end() {
    // What npm and pnpm leave in node_modules. Followed, the walk never ends,
    // and servio said the watch was broken while it was doing its job.
    let dir = site("loop-polling");
    std::fs::create_dir_all(dir.join("node_modules/pkg")).expect("could not create it");
    std::os::unix::fs::symlink("../..", dir.join("node_modules/pkg/up")).expect("could not link");

    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    assert!(
        !server.said("Cannot"),
        "a link to a directory above was reported as a fault:\n{}",
        server.lines().join("\n")
    );

    // And the look still finds a real change.
    let browser = server.open_browser();
    dir.write("app.css", "body { color: red }");
    browser.wait_for_refreshes(1);
}

#[test]
fn while_polling_a_rebuild_is_not_reported_as_a_fault() {
    // A look that lands between the old directory going and the new one
    // arriving finds nothing there. The check says "Directory replaced" when it
    // does arrive; saying the directory cannot be read as well reads like a
    // fault.
    let dir = site("rebuild-quiet-polling");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    dir.remove_all();
    std::thread::sleep(Duration::from_millis(1500));
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");

    assert!(
        !server.said("Cannot"),
        "a rebuild was reported as a fault:\n{}",
        server.lines().join("\n")
    );
}

#[cfg(unix)]
#[test]
fn while_polling_a_second_save_in_the_same_second_still_refreshes() {
    // A poll keeps the write time only to the whole second, so two saves close
    // together carry the same one and only the contents tell them apart. The
    // write time is pinned rather than raced for: the same second is otherwise
    // a matter of luck.
    let dir = site("same-second");
    let save = |contents: &str| {
        dir.write("app.css", contents);
        let pinned = std::process::Command::new("touch")
            .args(["-t", "202601011200"])
            .arg(dir.join("app.css"))
            .status()
            .expect("could not run touch");
        assert!(pinned.success(), "could not pin the write time");
    };

    save("body { color: red }");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    let browser = server.open_browser();
    save("body { color: blue }");
    browser.wait_for_refreshes(1);
}

#[test]
fn a_rebuild_is_announced_once_while_polling() {
    // The check notices the new directory before the poll's own account of the
    // same rebuild arrives. One rebuild, one line.
    let dir = site("announced-once-polling");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    dir.remove_all();
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");

    // Long enough for the poll's own account of the rebuild to have arrived:
    // one look, plus the debounce that groups what it found.
    std::thread::sleep(Duration::from_millis(2000));
    assert_eq!(
        server.count("File changed"),
        0,
        "the rebuild was announced twice:\n{}",
        server.lines().join("\n")
    );

    // What must not be lost: editing after a rebuild still says so.
    dir.write("index.html", "<html>edited</html>");
    server.wait_for("File changed");
}

#[test]
fn a_slow_rebuild_is_announced_once_while_polling() {
    // A build that pauses between clearing the directory and writing the new
    // one lets a look land in the gap. That look finds every file gone, which
    // is the rebuild starting, not a change of its own.
    let dir = site("slow-rebuild-polling");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    dir.remove_all();
    std::thread::sleep(A_LOOK);
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");

    // Long enough for the poll's own account of the rebuild to have arrived,
    // and for the window in which a change counts as that rebuild to close.
    std::thread::sleep(Duration::from_millis(2000));
    assert_eq!(
        server.count("File changed"),
        0,
        "the rebuild was announced twice:\n{}",
        server.lines().join("\n")
    );

    // What must not be lost: editing after a rebuild still says so.
    dir.write("index.html", "<html>edited</html>");
    server.wait_for("File changed");
}

#[test]
fn while_polling_a_directory_taken_away_does_not_refresh_to_an_error_page() {
    // Nothing can load from a directory that is not there, so the page on
    // screen, the last one that could load, is left alone.
    let dir = site("taken-away-polling");
    let server = Server::start(dir.path(), &["--poll"]);
    server.settle();

    let browser = server.open_browser();
    dir.remove_all();
    browser.expect_no_refresh_within(A_LOOK);
    assert!(
        !server.said("File changed"),
        "a directory taken away was called a change:\n{}",
        server.lines().join("\n")
    );

    // Put back, the check says so, once, and the page loads again.
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");
    browser.wait_for_refreshes(1);
}

#[test]
fn a_directory_taken_away_does_not_refresh_to_an_error_page() {
    // The system's watcher hears of the removal at once, before a build has
    // had time to write the new directory. Left to the check, as with `--poll`.
    let dir = site("taken-away");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let browser = server.open_browser();
    dir.remove_all();
    browser.expect_no_refresh_within(A_LOOK);
    assert!(
        !server.said("File changed"),
        "a directory taken away was called a change:\n{}",
        server.lines().join("\n")
    );

    // Put back, the check says so, once, and the page loads again.
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");
    browser.wait_for_refreshes(1);
}

#[test]
fn serving_a_page_does_not_refresh_it() {
    // Reading a file is an event of its own on Linux. If that counted as a
    // change, every page would reload itself for ever.
    let dir = site("loop");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let browser = server.open_browser();
    for _ in 0..20 {
        assert_eq!(get(server.port, "/").status, 200);
        assert_eq!(get(server.port, "/app.css").status, 200);
    }

    browser.expect_no_refresh();
}

#[test]
fn editor_scratch_files_are_ignored() {
    let dir = site("scratch");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let browser = server.open_browser();
    dir.write("index.html.swp", "vim");
    dir.write("index.html~", "backup");
    dir.write("#index.html#", "emacs");
    dir.write(".#index.html", "emacs lock");
    dir.write(".git/HEAD", "ref: refs/heads/main");
    dir.write("node_modules/left-pad/index.js", "module.exports = 1");

    browser.expect_no_refresh();
}

#[test]
fn a_change_matching_an_ignore_pattern_does_not_refresh() {
    // A build that writes a log next to the page would otherwise refresh the
    // browser for every line of it.
    let dir = site("ignore");
    // The directory is there from the start: making one is a change of its own.
    dir.write("out/build.log", "");
    let server = Server::start(dir.path(), &["--ignore", "*.log", "--ignore", "cache"]);
    server.settle();

    let browser = server.open_browser();
    dir.write("build.log", "compiled");
    dir.write("out/build.log", "compiled");
    dir.write("cache/pages/index.html", "<html>cached</html>");

    browser.expect_no_refresh();

    // Everything else still does.
    dir.write("app.css", "body { color: red }");
    browser.wait_for_refreshes(1);
}

#[test]
fn the_ignore_file_in_the_served_directory_is_read() {
    let dir = site("ignore-file");
    dir.write(".servioignore", "# what the build writes\n*.log\ncache\n");
    let server = Server::start(dir.path(), &["--ignore", "*.map"]);
    server.settle();

    let browser = server.open_browser();
    dir.write("build.log", "compiled");
    dir.write("cache/pages/index.html", "<html>cached</html>");
    dir.write("app.css.map", "{}");
    // The file itself is hidden, so editing it does not refresh either.
    dir.write(".servioignore", "*.log\n");

    browser.expect_no_refresh();

    dir.write("app.css", "body { color: red }");
    browser.wait_for_refreshes(1);
}

#[test]
fn a_change_matching_an_ignore_pattern_does_not_refresh_while_polling() {
    let dir = site("ignore-polling");
    let server = Server::start(dir.path(), &["--poll", "--ignore", "*.log"]);
    server.settle();

    let browser = server.open_browser();
    dir.write("build.log", "compiled");
    browser.expect_no_refresh_within(A_LOOK);

    dir.write("app.css", "body { color: red }");
    browser.wait_for_refreshes(1);
}

#[test]
fn files_that_are_never_served_do_not_refresh() {
    // An editor writing into .idea or .vscode inside the site should not
    // refresh the page, and neither should a change to .env.
    let dir = site("unservable");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let browser = server.open_browser();
    dir.write(".idea/workspace.xml", "<project/>");
    dir.write(".vscode/settings.json", "{}");
    dir.write(".env", "API_KEY=secret");

    browser.expect_no_refresh();
}

#[test]
fn changes_the_web_can_reach_still_refresh() {
    let dir = site("well-known");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let browser = server.open_browser();
    dir.write(".well-known/token", "public");
    browser.wait_for_refreshes(1);
}

// Linux only. macOS reports the changes to a file as one running total, so a
// permission change arrives carrying the file's creation as well, and there is
// no way to tell the two apart. The page refreshes once; nothing worse.
#[cfg(target_os = "linux")]
#[test]
fn changing_only_permissions_does_not_refresh() {
    let dir = site("permissions");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let browser = server.open_browser();
    set_permissions(&dir, 0o600);
    set_permissions(&dir, 0o644);

    browser.expect_no_refresh();
}

#[test]
fn survives_a_build_that_replaces_the_directory() {
    // `rm -rf dist && mkdir dist` is what most build tools do, and the watch
    // follows the old directory unless it is re-established.
    let dir = site("rebuild");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    dir.remove_all();
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");
    server.settle();

    let browser = server.open_browser();
    dir.write("index.html", "<html>edited after the rebuild</html>");
    browser.wait_for_refreshes(1);
}

#[test]
fn survives_a_build_that_renames_the_directory_away() {
    let dir = site("rename");
    let elsewhere = TempDir::new("rename-published");
    let moved = elsewhere.join("earlier");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    std::fs::rename(dir.path(), &moved).expect("could not rename the directory");
    dir.create();
    dir.write("index.html", "<html>published</html>");
    server.wait_for("Directory replaced");
    server.settle();

    // The old directory is no longer the site, so changes there mean nothing.
    let browser = server.open_browser();
    std::fs::write(moved.join("index.html"), "<html>stale</html>").unwrap();
    browser.expect_no_refresh();
}

#[test]
fn a_rebuild_is_announced_once() {
    // The watcher sees the old files go away and the check sees the new
    // directory, but that is one rebuild and reads better as one line.
    let dir = site("announced-once");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    dir.remove_all();
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");
    server.settle();

    assert_eq!(
        server.count("File changed"),
        0,
        "the rebuild was announced twice:\n{}",
        server.lines().join("\n")
    );

    // What must not be lost: editing after a rebuild still says so.
    dir.write("index.html", "<html>edited</html>");
    server.wait_for("File changed");
}

#[test]
fn a_change_made_while_the_directory_is_moved_aside_is_refreshed_once_it_is_back() {
    moved_aside_and_back(&[]);
}

#[test]
fn a_change_made_while_the_directory_is_moved_aside_is_refreshed_once_it_is_back_while_polling() {
    moved_aside_and_back(&["--poll"]);
}

/// A script moves the directory aside, works on it there, and moves it back.
/// The change was reported under a name that led nowhere at the time.
fn moved_aside_and_back(args: &[&str]) {
    let dir = site("aside");
    let elsewhere = TempDir::new("aside-elsewhere");
    let aside = elsewhere.join("site");
    let server = Server::start(dir.path(), args);
    server.settle();

    let browser = server.open_browser();
    std::fs::rename(dir.path(), &aside).expect("could not move the directory aside");
    std::fs::write(aside.join("index.html"), "<html>worked on</html>")
        .expect("could not write the test file");
    std::thread::sleep(Duration::from_millis(400));
    std::fs::rename(&aside, dir.path()).expect("could not move the directory back");

    server.wait_for("Directory back");
    browser.wait_for_refreshes(1);
    assert!(get(server.port, "/").text().contains("worked on"));
}

#[test]
fn keeps_serving_after_the_directory_is_replaced() {
    let dir = site("still-serving");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    dir.remove_all();
    dir.create();
    dir.write("index.html", "<html>rebuilt</html>");
    server.wait_for("Directory replaced");

    assert!(get(server.port, "/").text().contains("rebuilt"));
}

#[test]
fn a_file_that_is_written_without_pause_does_not_hold_the_refresh_back_for_long() {
    // A log written every moment never goes quiet. Waiting for that, an edit
    // made meanwhile would never reach the browser.
    let dir = site("never-quiet");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    let browser = server.open_browser();
    let writing_since = Instant::now();
    while writing_since.elapsed() < Duration::from_millis(2500) {
        dir.write("log.txt", &format!("{:?}", writing_since.elapsed()));
        std::thread::sleep(Duration::from_millis(50));
    }
    server.settle();

    assert!(
        browser.refreshes() >= 2,
        "a file written without pause held the refresh back:\n{}",
        server.lines().join("\n")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_rebuild_with_a_directory_it_may_not_read_says_so_rather_than_going_quiet() {
    use std::os::unix::fs::PermissionsExt;

    let dir = site("locked-rebuild");
    let server = Server::start(dir.path(), &[]);
    server.settle();

    // Built elsewhere and moved into place whole, so the check cannot catch
    // the new directory before the locked one is in it.
    let next = TempDir::new("locked-rebuild-next");
    next.write("index.html", "<html>rebuilt</html>");
    let locked = next.join("locked");
    std::fs::create_dir(&locked).expect("could not create the directory");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
        .expect("could not close the directory");
    dir.remove_all();
    std::fs::rename(next.path(), dir.path()).expect("could not move the directory into place");
    let locked = dir.join("locked");

    let deadline = Instant::now() + common::TIMEOUT;
    while !server.said("Cannot watch locked") && !server.said("Directory replaced") {
        assert!(
            Instant::now() < deadline,
            "nothing was said:\n{}",
            server.lines().join("\n")
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Running as root: the directory can be read after all.
    if server.said("Directory replaced") {
        let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
        return;
    }
    assert!(
        server.said("will not let this program read it"),
        "the reason was not given:\n{}",
        server.lines().join("\n")
    );

    // Tried again every second, and said once.
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(
        server.count("Cannot watch"),
        1,
        "{}",
        server.lines().join("\n")
    );
    assert!(!server.said("Directory replaced"));

    // Let in, the watch goes on and the rebuild is announced; edits count again.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
        .expect("could not open the directory");
    server.wait_for("Directory replaced");
    server.settle();
    dir.write("index.html", "<html>edited</html>");
    server.wait_for("File changed");
}

#[cfg(target_os = "linux")]
fn set_permissions(dir: &TempDir, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.join("app.css"), std::fs::Permissions::from_mode(mode))
        .expect("could not change the permissions");
}
