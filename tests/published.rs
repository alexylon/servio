//! `--production`, `--no-reload` and `--cache-assets`: what changes when a
//! finished site is served rather than worked on.

mod common;

use common::{PAGE, Response, Server, TempDir, get, get_page, request};
use std::time::{Duration, SystemTime};

fn site(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    dir.write("index.html", "<html>the app</html>");
    dir.write("assets/app-abc123.css", "body {}");
    dir
}

/// Writes a file and sets the time it says it was written.
fn write_at(dir: &TempDir, relative: &str, contents: &str, written: SystemTime) {
    dir.write(relative, contents);
    std::fs::File::options()
        .write(true)
        .open(dir.join(relative))
        .and_then(|file| file.set_modified(written))
        .expect("could not set when the file was written");
}

/// What a browser sends to check a copy it kept.
fn check(server: &Server, address: &str, kept: &Response) -> Response {
    let mut headers = vec![("Accept", PAGE)];
    headers.extend(kept.header("etag").map(|tag| ("If-None-Match", tag)));
    headers.extend(
        kept.header("last-modified")
            .map(|date| ("If-Modified-Since", date)),
    );
    request(server.port, "GET", address, &headers)
}

#[test]
fn hashed_assets_are_kept_and_everything_else_is_checked() {
    // A build puts the hash of the contents in the name, so the file at that
    // address never changes. The page itself does.
    let dir = site("cache");
    let server = Server::start(dir.path(), &["--cache-assets"]);

    assert_eq!(
        get(server.port, "/assets/app-abc123.css").header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(
        get(server.port, "/").header("cache-control"),
        Some("no-cache")
    );
    assert_eq!(
        get(server.port, "/nowhere.png").header("cache-control"),
        Some("no-cache")
    );
}

#[test]
fn a_file_that_is_not_there_is_never_kept() {
    // A deploy can be caught halfway, with the page asking for a file that has
    // not been copied yet. Keeping that answer for a year would mean the file
    // is never asked for again on that browser, and the name carries a hash so
    // it never changes.
    let dir = site("missing");
    let server = Server::start(dir.path(), &["--cache-assets"]);

    let response = get(server.port, "/assets/not-copied-yet-abc123.js");
    assert_eq!(response.status, 404);
    assert_eq!(response.header("cache-control"), Some("no-cache"));
}

#[test]
fn a_page_at_a_folders_own_address_under_assets_is_not_kept() {
    // Only file names carry a hash; the page at a folder's address can change.
    let dir = site("folder-page");
    dir.write("assets/docs/index.html", "<html>docs</html>");
    let server = Server::start(dir.path(), &["--cache-assets"]);

    let response = get(server.port, "/assets/docs/");
    assert_eq!(response.status, 200);
    assert_eq!(response.header("cache-control"), Some("no-cache"));
}

#[test]
fn a_missing_asset_is_never_answered_with_the_app() {
    // Nothing under /assets/ is a route: those names carry a hash of the
    // file's contents. Answering with the app page there would leave the
    // browser holding HTML at that address for a year, so the real file, once
    // the deploy has finished, would never be asked for again.
    let dir = site("shell");
    let server = Server::start(dir.path(), &["--spa", "--cache-assets"]);

    let response = request(
        server.port,
        "GET",
        "/assets/not-copied-yet-abc123.js",
        &[("Accept", PAGE)],
    );

    assert_eq!(response.status, 404);
    assert!(!response.text().contains("the app"));
    assert_eq!(response.header("cache-control"), Some("no-cache"));
}

#[test]
fn a_refused_file_is_answered_like_anything_else() {
    let dir = site("refused");
    dir.write(".env", "API_KEY=secret");
    let server = Server::start(dir.path(), &["--cache-assets"]);

    let response = get(server.port, "/.env");
    assert_eq!(response.status, 404);
    assert_eq!(response.header("cache-control"), Some("no-cache"));
    assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
}

#[test]
fn a_browser_that_already_has_the_file_is_told_so() {
    // Without this, "check each time" would still send the whole file back.
    let dir = site("revalidate");
    let server = Server::start(dir.path(), &["--cache-assets"]);

    let modified = get(server.port, "/assets/app-abc123.css")
        .header("last-modified")
        .expect("no last-modified to test with")
        .to_string();

    let response = request(
        server.port,
        "GET",
        "/assets/app-abc123.css",
        &[("If-Modified-Since", &modified)],
    );

    assert_eq!(response.status, 304);
    assert!(response.body.is_empty());

    // A browser takes the headers on this answer as the file's own, replacing
    // the ones it stored. Leave the year off and one check turns the file back
    // into one checked on every visit.
    assert_eq!(
        response.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
}

#[test]
fn without_the_flag_nothing_is_kept() {
    let dir = site("off");
    let server = Server::start(dir.path(), &[]);

    assert_eq!(
        get(server.port, "/assets/app-abc123.css").header("cache-control"),
        Some("no-store")
    );
}

#[test]
fn each_mode_tells_the_browser_what_it_may_keep() {
    const NOTHING: &str = "no-store";
    const CHECKED: &str = "no-cache";
    const YEAR: &str = "public, max-age=31536000, immutable";

    let dir = site("modes");
    // Kept all the same, though its name has no hash: servio does not check.
    dir.write("assets/logo.png", "png");
    dir.write("assets/docs/index.html", "<html>docs</html>");

    let addresses = [
        "/",
        "/assets/app-abc123.css",
        "/assets/logo.png",
        "/assets/not-copied-yet-abc123.js",
        "/assets/docs/",
    ];
    let assets = "files under /assets/ for a year, the rest checked each time";
    let modes: [(&[&str], &str, [&str; 5]); 4] = [
        (&[], "off", [NOTHING; 5]),
        (
            &["--production"],
            "on, checked for changes each time",
            [CHECKED; 5],
        ),
        (
            &["--cache-assets"],
            assets,
            [CHECKED, YEAR, YEAR, CHECKED, CHECKED],
        ),
        (
            &["--production", "--cache-assets"],
            assets,
            [CHECKED, YEAR, YEAR, CHECKED, CHECKED],
        ),
    ];

    for (flags, banner, kept) in modes {
        let server = Server::start(dir.path(), flags);
        assert!(
            server.said(&format!("Caching        : {banner}")),
            "{flags:?}:\n{}",
            server.lines().join("\n")
        );

        for (address, kept) in addresses.into_iter().zip(kept) {
            assert_eq!(
                get(server.port, address).header("cache-control"),
                Some(kept),
                "{address} with {flags:?}"
            );
        }
    }
}

#[test]
fn production_turns_off_reload_and_lists() {
    let dir = site("production");
    dir.write("docs/notes.txt", "notes");
    let server = Server::start(dir.path(), &["--production"]);

    let page = get_page(server.port, "/?list");
    assert!(page.text().contains("the app"), "{}", page.text());
    assert!(!page.text().contains("tower-livereload"));
    assert_eq!(get_page(server.port, "/docs/").status, 404);

    assert!(server.said("Live reload    : off"));
    assert!(server.said("File lists     : off"));
}

#[test]
fn with_production_an_unchanged_file_is_not_sent_again() {
    // The browser checks before every use, so an unchanged file must not be
    // sent again.
    let dir = site("production-revalidate");
    let server = Server::start(dir.path(), &["--production"]);

    let modified = get(server.port, "/")
        .header("last-modified")
        .expect("no last-modified to test with")
        .to_string();

    let response = request(server.port, "GET", "/", &[("If-Modified-Since", &modified)]);

    assert_eq!(response.status, 304);
    assert!(response.body.is_empty());
    assert_eq!(response.header("cache-control"), Some("no-cache"));
}

#[test]
fn a_file_rolled_back_to_an_older_copy_is_sent_again() {
    // A rollback by `rsync -a` or `tar x` brings the older file back with its
    // older date, which a check by date alone takes for no change.
    let dir = site("rollback");
    let now = SystemTime::now();
    write_at(&dir, "index.html", "<html>version 2</html>", now);
    let server = Server::start(dir.path(), &["--production"]);

    let kept = get(server.port, "/");
    write_at(
        &dir,
        "index.html",
        "<html>version 1</html>",
        now - Duration::from_secs(3600),
    );

    let checked = check(&server, "/", &kept);
    assert_eq!(checked.status, 200);
    assert!(checked.text().contains("version 1"), "{}", checked.text());

    // Asked by date alone, only the very same second counts as unchanged.
    let date = kept.header("last-modified").expect("no date to check with");
    let by_date = request(server.port, "GET", "/", &[("If-Modified-Since", date)]);
    assert_eq!(by_date.status, 200);
}

#[test]
fn a_file_changed_twice_within_a_second_is_sent_again() {
    let dir = site("same-second");
    let second = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    write_at(
        &dir,
        "index.html",
        "<html>first</html>",
        second + Duration::from_millis(100),
    );

    // A disk that keeps whole seconds cannot tell the two apart.
    let written = std::fs::metadata(dir.join("index.html")).and_then(|file| file.modified());
    if written.is_ok_and(|written| written == second) {
        return;
    }

    let server = Server::start(dir.path(), &["--production"]);
    let kept = get(server.port, "/");

    // As long as the first, and in the same second.
    write_at(
        &dir,
        "index.html",
        "<html>again</html>",
        second + Duration::from_millis(600),
    );

    let checked = check(&server, "/", &kept);
    assert_eq!(checked.status, 200);
    assert!(checked.text().contains("again"), "{}", checked.text());
}

#[test]
fn a_copy_named_by_its_tag_is_not_sent_again() {
    let dir = site("tag");
    let server = Server::start(dir.path(), &["--production"]);

    let kept = get(server.port, "/assets/app-abc123.css");
    let tag = kept.header("etag").expect("no tag to check with");
    let checked = request(
        server.port,
        "GET",
        "/assets/app-abc123.css",
        &[("If-None-Match", tag)],
    );

    assert_eq!(checked.status, 304);
    assert!(checked.body.is_empty());
    assert_eq!(checked.header("etag"), Some(tag));
    assert_eq!(checked.header("cache-control"), Some("no-cache"));
}

#[test]
fn the_rest_of_a_file_that_changed_is_sent_whole() {
    // A browser resuming a download asks for the rest only of the file it
    // began with.
    let dir = site("if-range");
    let server = Server::start(dir.path(), &["--production"]);

    let kept = get(server.port, "/assets/app-abc123.css");
    let date = kept
        .header("last-modified")
        .expect("no date to check with")
        .to_string();
    let rest = [("Range", "bytes=4-"), ("If-Range", date.as_str())];
    assert_eq!(
        request(server.port, "GET", "/assets/app-abc123.css", &rest).status,
        206
    );

    write_at(
        &dir,
        "assets/app-abc123.css",
        "main { color: red }",
        SystemTime::now() + Duration::from_secs(5),
    );
    let changed = request(server.port, "GET", "/assets/app-abc123.css", &rest);
    assert_eq!(changed.status, 200);
    assert_eq!(changed.text(), "main { color: red }");
}

#[test]
fn with_production_a_route_is_not_sent_the_app_again_while_it_is_unchanged() {
    let dir = site("app-version");
    let server = Server::start(dir.path(), &["--production", "--spa"]);

    let kept = get_page(server.port, "/users/123");
    assert!(kept.text().contains("the app"));
    assert_eq!(check(&server, "/users/123", &kept).status, 304);

    write_at(
        &dir,
        "index.html",
        "<html>the new app</html>",
        SystemTime::now() + Duration::from_secs(5),
    );
    let changed = check(&server, "/users/123", &kept);
    assert_eq!(changed.status, 200);
    assert!(changed.text().contains("the new app"));
}

#[test]
fn no_reload_leaves_the_page_alone() {
    let dir = site("quiet");
    let server = Server::start(dir.path(), &["--no-reload"]);

    assert!(
        !get_page(server.port, "/")
            .text()
            .contains("tower-livereload")
    );
    assert!(server.said("Live reload    : off"));
}

#[test]
fn no_reload_stops_watching_altogether() {
    let dir = site("unwatched");
    let server = Server::start(dir.path(), &["--no-reload"]);
    server.settle();

    dir.write("index.html", "<html>edited</html>");
    server.expect_no_reload(0);
}

#[test]
fn a_published_site_still_refuses_hidden_files_and_serves_its_routes() {
    let dir = site("published");
    dir.write(".env", "API_KEY=secret");
    let server = Server::start(
        dir.path(),
        &[
            "--spa",
            "--production",
            "--cache-assets",
            "--host",
            "0.0.0.0",
        ],
    );

    assert_eq!(get_page(server.port, "/.env").status, 404);

    let route = request(server.port, "GET", "/users/123", &[("Accept", PAGE)]);
    assert_eq!(route.status, 200);
    assert!(route.text().contains("the app"));
    assert!(!route.text().contains("tower-livereload"));
}
