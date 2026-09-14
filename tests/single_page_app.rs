//! `--spa`: page requests fall back to index.html, everything else does not.

mod common;

use common::{PAGE, Server, TempDir, get, get_page, request};

fn app(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    dir.write("index.html", "<html>the app</html>");
    dir.write("assets/app.css", "body {}");
    dir
}

#[test]
fn an_address_with_no_file_behind_it_gets_the_app() {
    let dir = app("route");
    let server = Server::start(dir.path(), &["--spa"]);

    let response = get_page(server.port, "/users/123");
    assert_eq!(response.status, 200, "a 404 stops the app from starting");
    assert!(response.text().contains("the app"));
}

#[test]
fn the_app_page_still_gets_the_reload_script() {
    let dir = app("script");
    let server = Server::start(dir.path(), &["--spa"]);

    assert!(
        get_page(server.port, "/users/123")
            .text()
            .contains("tower-livereload")
    );
}

#[test]
fn a_missing_script_or_image_is_still_not_found() {
    // Otherwise a typo in a src attribute arrives as a page of HTML that the
    // browser then refuses to run.
    let dir = app("assets");
    let server = Server::start(dir.path(), &["--spa"]);

    assert_eq!(
        request(server.port, "GET", "/assets/typo.js", &[("Accept", "*/*")]).status,
        404
    );
    assert_eq!(
        request(server.port, "GET", "/photo.png", &[("Accept", "image/*")]).status,
        404
    );
}

#[test]
fn real_files_are_still_served() {
    let dir = app("real");
    let server = Server::start(dir.path(), &["--spa"]);

    assert_eq!(get(server.port, "/assets/app.css").status, 200);
    assert_eq!(get_page(server.port, "/").status, 200);
}

#[test]
fn the_app_page_carries_none_of_the_index_files_own_details() {
    // The index has a date and a length of its own, and neither describes the
    // address that was asked for.
    let dir = app("conditional");
    let server = Server::start(dir.path(), &["--spa"]);

    let conditional = request(
        server.port,
        "GET",
        "/users/123",
        &[
            ("Accept", PAGE),
            ("If-Modified-Since", "Thu, 01 Jan 2099 00:00:00 GMT"),
        ],
    );
    assert_eq!(conditional.status, 200, "a 304 would leave the page blank");
    assert_eq!(conditional.header("last-modified"), None);
    assert_eq!(conditional.header("etag"), None);
    assert!(conditional.text().contains("the app"));

    let ranged = request(
        server.port,
        "GET",
        "/users/123",
        &[("Accept", PAGE), ("Range", "bytes=200000-")],
    );
    assert_eq!(
        ranged.status, 200,
        "the index's length is not this address's"
    );
    assert_eq!(ranged.header("content-range"), None);
}

#[test]
fn a_missing_file_stays_missing_however_it_is_asked_for() {
    let dir = app("not-a-page");
    let server = Server::start(dir.path(), &["--spa"]);

    let conditional = request(
        server.port,
        "GET",
        "/gone.js",
        &[
            ("Accept", "*/*"),
            ("If-Modified-Since", "Thu, 01 Jan 2099 00:00:00 GMT"),
        ],
    );
    assert_eq!(conditional.status, 404);

    let ranged = request(
        server.port,
        "GET",
        "/clip.mp4",
        &[("Accept", "*/*"), ("Range", "bytes=200000-")],
    );
    assert_eq!(ranged.status, 404);
}

#[test]
fn hidden_files_stay_hidden() {
    let dir = app("hidden");
    dir.write(".env", "API_KEY=secret");
    let server = Server::start(dir.path(), &["--spa"]);

    assert_eq!(get_page(server.port, "/.env").status, 404);
}

#[test]
fn without_the_flag_a_missing_address_is_not_found() {
    let dir = app("off");
    let server = Server::start(dir.path(), &[]);

    assert_eq!(get_page(server.port, "/users/123").status, 404);
}

#[test]
fn says_so_when_there_is_no_index_to_fall_back_on() {
    let dir = TempDir::new("no-index");
    let server = Server::start(dir.path(), &["--spa"]);

    assert!(server.said("Warning"));
    assert_eq!(get_page(server.port, "/users/123").status, 404);

    // The banner already said it; the first request should not say it again.
    server.settle();
    assert_eq!(server.count("the app will not load"), 1, "said twice");
}

/// A page that is there but closed to this program is no more use than one
/// that is gone, and must not read as having come back.
#[cfg(unix)]
#[test]
fn a_page_that_cannot_be_read_is_reported_once_like_a_missing_one() {
    use std::os::unix::fs::PermissionsExt;

    let dir = app("index-unreadable");
    let page = dir.join("index.html");
    std::fs::set_permissions(&page, std::fs::Permissions::from_mode(0o000))
        .expect("could not close the page");

    let server = Server::start(dir.path(), &["--spa", "--no-reload"]);
    let closed = get_page(server.port, "/users/123").status == 404;
    if !closed {
        // Running as root, which reads it anyway: nothing to check.
        let _ = std::fs::set_permissions(&page, std::fs::Permissions::from_mode(0o644));
        return;
    }

    server.wait_for_count("cannot be read", 1);

    // A request some other file answers must not take this for the page
    // coming back, or the next address would say it all over again.
    assert_eq!(get(server.port, "/assets/app.css").status, 200);
    assert_eq!(get_page(server.port, "/users/456").status, 404);

    server.settle();
    assert_eq!(
        server.count("the app will not load"),
        1,
        "said for each address"
    );

    let _ = std::fs::set_permissions(&page, std::fs::Permissions::from_mode(0o644));
}

#[test]
fn says_so_each_time_the_app_page_goes_missing() {
    let dir = app("index-comes-and-goes");
    let server = Server::start(dir.path(), &["--spa", "--no-reload"]);
    let page = dir.join("index.html");

    std::fs::remove_file(&page).expect("could not remove the page");
    assert_eq!(get_page(server.port, "/users/123").status, 404);
    server.wait_for_count("the app will not load", 1);

    // A build clearing the directory takes the page away for a moment. That
    // must not use up the warning the real outage needs — and the page counts
    // as back although the only request in between never reads it.
    dir.write("index.html", "<html>the app</html>");
    assert_eq!(get(server.port, "/assets/app.css").status, 200);

    std::fs::remove_file(&page).expect("could not remove the page");
    assert_eq!(get_page(server.port, "/users/456").status, 404);
    server.wait_for_count("the app will not load", 2);

    // Still once for each time it goes, not once for each address.
    assert_eq!(get_page(server.port, "/users/789").status, 404);
    server.settle();
    assert_eq!(
        server.count("the app will not load"),
        2,
        "said for each address"
    );
}

/// Creating links on Windows needs extra permissions, so this is Unix only.
#[cfg(unix)]
#[test]
fn an_app_page_leading_out_of_the_directory_is_never_served() {
    let dir = TempDir::new("app-link-out");
    let elsewhere = TempDir::new("app-link-target");
    elsewhere.write("secret.html", "SECRET");
    std::os::unix::fs::symlink(elsewhere.join("secret.html"), dir.join("index.html"))
        .expect("could not make the link");

    let server = Server::start(dir.path(), &["--spa", "--no-reload"]);

    for address in ["/", "/users/123"] {
        let response = get_page(server.port, address);
        assert_eq!(response.status, 404, "for {address}");
        assert!(!response.text().contains("SECRET"), "for {address}");
    }
    server.wait_for("leads to a file that is never served");
}

/// Creating links on Windows needs extra permissions, so this is Unix only.
#[cfg(unix)]
#[test]
fn an_app_page_linked_from_inside_the_directory_is_served() {
    // Only links leading out are refused; a build may link its page in place.
    let dir = TempDir::new("app-link-in");
    dir.write("build/app.html", "<html>the app</html>");
    std::os::unix::fs::symlink(dir.join("build/app.html"), dir.join("index.html"))
        .expect("could not make the link");

    let server = Server::start(dir.path(), &["--spa", "--no-reload"]);

    for address in ["/", "/users/123"] {
        let response = get_page(server.port, address);
        assert_eq!(response.status, 200, "for {address}");
        assert!(response.text().contains("the app"), "for {address}");
    }
}
