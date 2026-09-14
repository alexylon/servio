//! File lists: shown for folders with no index.html and for `?list`, unless
//! `--no-list` is given.

mod common;

use common::{PAGE, Server, TempDir, get, get_page, request};

fn site(name: &str) -> TempDir {
    let dir = TempDir::new(name);
    dir.write("index.html", "<html>the home page</html>");
    dir.write("about.html", "<html>about</html>");
    dir.write("docs/guide.html", "<html>the guide</html>");
    dir.write("docs/api/index.html", "<html>the api</html>");
    dir
}

#[test]
fn a_folder_with_no_page_of_its_own_lists_what_is_in_it() {
    let dir = site("no-page");
    let server = Server::start(dir.path(), &[]);

    let response = get_page(server.port, "/docs/");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("content-type"),
        Some("text/html; charset=utf-8")
    );

    let list = response.text();
    assert!(list.contains("Index of /docs/"), "{list}");
    assert!(list.contains(r#"href="guide.html""#), "{list}");
    assert!(list.contains(r#"href="api/""#), "{list}");
}

#[test]
fn a_folder_with_a_page_of_its_own_still_shows_the_page() {
    let dir = site("page-first");
    let server = Server::start(dir.path(), &[]);

    assert!(get_page(server.port, "/").text().contains("the home page"));
    assert!(
        get_page(server.port, "/docs/api/")
            .text()
            .contains("the api")
    );
}

#[test]
fn asking_for_the_list_shows_it_where_there_is_a_page() {
    let dir = site("asked");
    let server = Server::start(dir.path(), &[]);

    let list = get_page(server.port, "/?list").text();
    assert!(!list.contains("the home page"), "{list}");
    for link in [
        r#"href="index.html""#,
        r#"href="about.html""#,
        r#"href="docs/?list""#,
    ] {
        assert!(list.contains(link), "no {link}:\n{list}");
    }
}

#[test]
fn only_a_bare_list_query_asks_for_the_list() {
    // `?list=groceries` belongs to the page, not to the file list.
    let dir = site("list-query");
    let server = Server::start(dir.path(), &[]);

    let page = get_page(server.port, "/?list=groceries").text();
    assert!(page.contains("the home page"), "{page}");
}

#[test]
fn the_index_page_opens_from_the_list() {
    let dir = site("open-index");
    let server = Server::start(dir.path(), &[]);

    let list = get_page(server.port, "/?list").text();
    assert!(list.contains(r#"href="index.html""#), "{list}");

    let page = get_page(server.port, "/index.html");
    assert_eq!(page.status, 200);
    assert!(page.text().contains("the home page"));
}

#[test]
fn folders_come_first_then_files_by_name() {
    let dir = TempDir::new("order");
    dir.write("b.txt", "b");
    dir.write("A.txt", "a");
    dir.write("zebra/z.txt", "z");
    let server = Server::start(dir.path(), &[]);

    let list = get_page(server.port, "/").text();
    let place = |name: &str| {
        list.find(&format!(r#"href="{name}""#))
            .unwrap_or_else(|| panic!("no {name}:\n{list}"))
    };
    assert!(place("zebra/") < place("A.txt"), "{list}");
    assert!(place("A.txt") < place("b.txt"), "{list}");
}

#[test]
fn a_list_shows_each_files_size_and_when_it_was_written() {
    let dir = TempDir::new("details");
    dir.write("small.txt", "hello");
    dir.write("large.txt", &"x".repeat(1536));
    dir.write("folder/inside.txt", "x");

    // A fixed time, so the expected text is known.
    let written = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_789_397_460);
    std::fs::File::options()
        .write(true)
        .open(dir.join("small.txt"))
        .and_then(|file| file.set_modified(written))
        .expect("could not set the time");

    let server = Server::start(dir.path(), &[]);
    let list = get_page(server.port, "/").text();

    for row in [
        r#"<a href="small.txt">small.txt</a></td><td class="size">5 B</td><td><time datetime="2026-09-14T14:51Z">2026-09-14 14:51 UTC</time></td>"#,
        r#"<a href="large.txt">large.txt</a></td><td class="size">1.5 KB</td>"#,
        // Folders show no size.
        r#"<a href="folder/">folder/</a></td><td class="size"></td>"#,
    ] {
        assert!(list.contains(row), "no {row}:\n{list}");
    }
}

#[test]
fn a_folder_with_nothing_to_serve_says_it_is_empty() {
    // Hidden files don't count, so this folder shows as empty.
    let dir = TempDir::new("empty");
    dir.write("nothing/.DS_Store", "");
    let server = Server::start(dir.path(), &[]);

    let list = get_page(server.port, "/nothing/").text();
    assert!(list.contains("This folder is empty"), "{list}");
}

#[test]
fn once_asked_for_a_walk_through_the_folders_stays_on_the_lists() {
    // `/` and docs/api have their own pages; links on a `?list` page must
    // still lead to lists.
    let dir = site("walk");
    let server = Server::start(dir.path(), &[]);

    let top = get_page(server.port, "/?list").text();
    assert!(
        !top.contains(r#"href="../"#),
        "nothing is above the top:\n{top}"
    );

    let docs = get_page(server.port, "/docs/?list").text();
    assert!(docs.contains(r#"href="api/?list""#), "{docs}");
    assert!(docs.contains(r#"href="../?list""#), "{docs}");
}

#[test]
fn unasked_the_links_lead_to_pages_as_usual() {
    let dir = site("unasked");
    let server = Server::start(dir.path(), &[]);

    let docs = get_page(server.port, "/docs/").text();
    assert!(docs.contains(r#"href="api/""#), "{docs}");
    assert!(docs.contains(r#"href="../""#), "{docs}");
}

#[test]
fn a_folder_named_without_its_slash_is_sent_to_the_address_with_it() {
    // Relative links need the address to end in `/`.
    let dir = site("slash");
    let server = Server::start(dir.path(), &[]);

    let asked = get_page(server.port, "/docs?list");
    assert_eq!(asked.status, 307);
    assert_eq!(asked.header("location"), Some("/docs/?list"));

    let unasked = get_page(server.port, "/docs");
    assert_eq!(unasked.status, 307);
    assert_eq!(unasked.header("location"), Some("/docs/"));
}

#[test]
fn only_a_folder_has_a_list() {
    let dir = site("not-a-folder");
    let server = Server::start(dir.path(), &[]);

    let file = get_page(server.port, "/about.html?list").text();
    assert!(file.starts_with("<html>about</html>"), "{file}");
    assert_eq!(get_page(server.port, "/nowhere/?list").status, 404);
    assert_eq!(get_page(server.port, "/nowhere/").status, 404);
}

#[test]
fn what_is_never_served_is_never_listed() {
    let dir = site("hidden");
    dir.write(".env", "API_KEY=secret");
    dir.write(".git/config", "[core]");
    dir.write(".well-known/token", "public");
    let server = Server::start(dir.path(), &[]);

    let list = get_page(server.port, "/?list").text();
    assert!(!list.contains(".env"), "{list}");
    assert!(!list.contains(".git"), "{list}");
    assert!(list.contains(r#"href=".well-known/?list""#), "{list}");

    assert_eq!(get_page(server.port, "/.git/?list").status, 404);
}

#[test]
fn a_name_with_signs_in_it_reads_as_it_is_and_leads_to_its_file() {
    // Unescaped, `&` would break the HTML and `#` would cut the link short.
    let dir = TempDir::new("signs");
    dir.write("a b&c #1 100%.txt", "signs");
    dir.write("café.txt", "accent");
    let server = Server::start(dir.path(), &[]);

    let list = get_page(server.port, "/").text();
    assert!(
        list.contains(r#"<a href="a%20b%26c%20%231%20100%25.txt">a b&amp;c #1 100%.txt</a>"#),
        "{list}"
    );
    assert!(list.contains(r#"href="caf%C3%A9.txt""#), "{list}");

    assert_eq!(
        get(server.port, "/a%20b%26c%20%231%20100%25.txt").text(),
        "signs"
    );
    assert_eq!(get(server.port, "/caf%C3%A9.txt").text(), "accent");
}

/// Windows doesn't allow these characters in file names.
#[cfg(unix)]
#[test]
fn a_name_that_looks_like_markup_stays_text() {
    let dir = TempDir::new("markup");
    dir.write("<img src=x onerror=alert(1)>.txt", "markup");
    let server = Server::start(dir.path(), &[]);

    let list = get_page(server.port, "/").text();
    assert!(!list.contains("<img"), "{list}");
    assert!(
        list.contains("&lt;img src=x onerror=alert(1)&gt;.txt"),
        "{list}"
    );
}

#[test]
fn no_list_lists_nothing() {
    let dir = site("off");
    let server = Server::start(dir.path(), &["--no-list"]);

    assert_eq!(get_page(server.port, "/docs/").status, 404);
    assert!(
        get_page(server.port, "/?list")
            .text()
            .contains("the home page")
    );
}

#[test]
fn with_spa_a_folder_gets_the_app_unless_its_list_is_asked_for() {
    // An app route can share its name with a folder; the app must still
    // answer there.
    let dir = site("spa");
    let server = Server::start(dir.path(), &["--spa"]);

    let folder = get_page(server.port, "/docs/").text();
    assert!(folder.contains("the home page"), "{folder}");

    let asked = get_page(server.port, "/docs/?list").text();
    assert!(asked.contains(r#"href="guide.html""#), "{asked}");

    let route = get_page(server.port, "/users/123");
    assert_eq!(route.status, 200);
    assert!(route.text().contains("the home page"));
}

#[test]
fn a_list_gets_the_reload_script() {
    // So the list refreshes when files change.
    let dir = site("script");
    let server = Server::start(dir.path(), &[]);

    assert!(
        get_page(server.port, "/?list")
            .text()
            .contains("tower-livereload")
    );
}

#[test]
fn a_list_is_answered_like_a_page() {
    let dir = site("methods");
    let server = Server::start(dir.path(), &[]);

    let head = request(server.port, "HEAD", "/docs/", &[("Accept", PAGE)]);
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());

    assert_eq!(request(server.port, "POST", "/docs/?list", &[]).status, 405);
}

#[test]
fn a_list_under_assets_is_never_kept() {
    // Files under /assets/ are cached for a year with --cache-assets. A
    // folder's list changes, so it must not be.
    let dir = site("kept");
    dir.write("assets/app-abc123.css", "body {}");
    let server = Server::start(dir.path(), &["--cache-assets"]);

    for address in ["/assets/", "/assets/?list"] {
        let response = get_page(server.port, address);
        assert_eq!(response.status, 200, "for {address}");
        assert_eq!(
            response.header("cache-control"),
            Some("no-cache"),
            "for {address}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_read_has_no_list() {
    use std::os::unix::fs::PermissionsExt;

    let dir = site("closed");
    let closed = dir.join("docs");
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000))
        .expect("could not close the folder");

    // No watcher: on Linux, an unreadable folder would stop it.
    let server = Server::start(dir.path(), &["--no-reload"]);
    let asked = get_page(server.port, "/docs/?list");
    let _ = std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755));

    // Running as root, which reads it anyway: nothing to check.
    if asked.text().contains("guide.html") {
        return;
    }
    assert_eq!(asked.status, 404);
}

#[cfg(unix)]
#[test]
fn a_folder_whose_index_page_cannot_be_read_is_not_listed_in_its_place() {
    use std::os::unix::fs::PermissionsExt;

    // The file service answers an unreadable page with 404, like a missing
    // one. A list there would hide the problem.
    let dir = site("closed-index");
    dir.write("docs/index.html", "<html>the docs</html>");
    let page = dir.join("docs/index.html");
    std::fs::set_permissions(&page, std::fs::Permissions::from_mode(0o000))
        .expect("could not close the page");

    let server = Server::start(dir.path(), &["--no-reload"]);
    let unasked = get_page(server.port, "/docs/");
    let asked = get_page(server.port, "/docs/?list").text();
    let _ = std::fs::set_permissions(&page, std::fs::Permissions::from_mode(0o644));

    // Running as root, which reads it anyway: nothing to check.
    if unasked.text().contains("the docs") {
        return;
    }
    assert_eq!(unasked.status, 404);
    assert!(asked.contains(r#"href="guide.html""#), "{asked}");
}

/// Creating links on Windows needs extra permissions, so these are Unix only.
#[cfg(unix)]
mod links {
    use super::common::{Server, TempDir, get_page};
    use std::os::unix::fs::symlink;

    #[test]
    fn a_link_leading_out_is_left_off_and_one_inside_is_listed() {
        let dir = TempDir::new("list-links");
        let elsewhere = TempDir::new("list-links-target");
        elsewhere.write("private.txt", "SECRET");
        dir.write("real/app.css", "body {}");

        symlink(elsewhere.path(), dir.join("out")).expect("could not make the link");
        symlink(elsewhere.join("private.txt"), dir.join("private.txt"))
            .expect("could not make the link");
        symlink(dir.join("real"), dir.join("linked")).expect("could not make the link");
        symlink(dir.join("gone"), dir.join("broken")).expect("could not make the link");

        let server = Server::start(dir.path(), &[]);
        let list = get_page(server.port, "/").text();

        assert!(!list.contains(r#"href="out/""#), "{list}");
        assert!(!list.contains("private.txt"), "{list}");
        assert!(!list.contains("broken"), "{list}");
        assert!(list.contains(r#"href="linked/""#), "{list}");
    }

    #[test]
    fn a_folder_whose_index_page_is_broken_is_not_listed_in_its_place() {
        // A list would hide that the page is broken.
        let dir = TempDir::new("list-broken-index");
        dir.write("docs/guide.html", "<html>the guide</html>");
        symlink(dir.join("docs/gone.html"), dir.join("docs/index.html"))
            .expect("could not make the link");

        let server = Server::start(dir.path(), &[]);

        assert_eq!(get_page(server.port, "/docs/").status, 404);
        let asked = get_page(server.port, "/docs/?list").text();
        assert!(asked.contains(r#"href="guide.html""#), "{asked}");
    }

    #[test]
    fn a_link_to_something_hidden_is_left_off_the_list() {
        let dir = TempDir::new("list-link-to-hidden");
        dir.write(".git/config", "[core]");
        dir.write(".env", "API_KEY=secret");
        dir.write("readme.txt", "hi");
        symlink(dir.join(".git"), dir.join("docs")).expect("could not make the link");
        symlink(dir.join(".env"), dir.join("visible.txt")).expect("could not make the link");

        let server = Server::start(dir.path(), &[]);
        let list = get_page(server.port, "/").text();

        assert!(!list.contains(r#"href="docs/""#), "{list}");
        assert!(!list.contains("visible.txt"), "{list}");
        assert!(list.contains(r#"href="readme.txt""#), "{list}");
    }

    #[test]
    fn a_folder_whose_index_page_leads_out_still_lists_when_asked() {
        // Only the page is refused; the folder can still be listed.
        let dir = TempDir::new("list-index-out");
        let elsewhere = TempDir::new("list-index-out-target");
        elsewhere.write("secret.html", "SECRET");
        dir.write("readme.txt", "hi");
        symlink(elsewhere.join("secret.html"), dir.join("index.html"))
            .expect("could not make the link");

        let server = Server::start(dir.path(), &[]);

        let page = get_page(server.port, "/");
        assert_eq!(page.status, 404);
        assert!(!page.text().contains("SECRET"));

        let list = get_page(server.port, "/?list").text();
        assert!(list.contains(r#"href="readme.txt""#), "{list}");
        assert!(!list.contains(r#"href="index.html""#), "{list}");
    }
}
