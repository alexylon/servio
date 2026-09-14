//! `--no-reload` and `--cache-assets`: what changes when the site is published
//! rather than being worked on.

mod common;

use common::{PAGE, Server, TempDir, get, get_page, request};

fn site(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    dir.write("index.html", "<html>the app</html>");
    dir.write("assets/app-abc123.css", "body {}");
    dir
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
            "--no-reload",
            "--no-list",
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
