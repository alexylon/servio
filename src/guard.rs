//! What the server refuses to hand out: anything outside the served
//! directory, and anything hidden.

use crate::serve::INDEX_FILE;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use percent_encoding::percent_decode_str;
use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

/// The one hidden directory the web uses.
const WELL_KNOWN: &str = ".well-known";

/// Windows reads a backslash as a separator, so a hidden file could hide
/// behind one there. Elsewhere it is an ordinary character.
#[cfg(windows)]
const SEPARATORS: &[char] = &['/', '\\'];
#[cfg(not(windows))]
const SEPARATORS: &[char] = &['/'];

pub(crate) async fn refuse_paths_outside(root: PathBuf, request: Request, next: Next) -> Response {
    // The file service follows links, which can point anywhere: to
    // `/etc/passwd`, or to a hidden `.env` under an ordinary name.
    if !may_be_served(&root, &on_disk(&root, request.uri().path())) {
        return StatusCode::NOT_FOUND.into_response();
    }

    next.run(request).await
}

/// For an address ending in `/`, the file service serves the folder's
/// index.html, which `refuse_paths_outside` never checked. Check it here, just
/// before it's opened. Only the page is refused: `?list` still works.
pub(crate) async fn refuse_index_pages_outside(
    root: PathBuf,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if path.ends_with('/') && !may_be_served(&root, &on_disk(&root, path).join(INDEX_FILE)) {
        return StatusCode::NOT_FOUND.into_response();
    }

    next.run(request).await
}

/// The path on disk for an address, before following any links.
fn on_disk(root: &Path, path: &str) -> PathBuf {
    root.join(decode(path).trim_start_matches(SEPARATORS))
}

/// Whether the server may serve what `path` points to. After following links,
/// it must be inside the served folder, with nothing hidden on the way. A path
/// that doesn't exist is allowed, so the file service can answer 404.
///
/// This reads the disk on the current thread: one quick lookup isn't worth
/// handing to another thread.
pub(crate) fn may_be_served(root: &Path, path: &Path) -> bool {
    let Ok(resolved) = path.canonicalize() else {
        return true;
    };
    let Ok(below) = resolved.strip_prefix(root) else {
        return false;
    };

    !below.components().any(|part| match part {
        Component::Normal(name) => is_hidden(&name.to_string_lossy()),
        _ => false,
    })
}

pub(crate) async fn refuse_hidden_files(request: Request, next: Next) -> Response {
    // Otherwise `servio --host 0.0.0.0` in a project directory hands `.env`
    // and `.git/config` to anyone on the network.
    if names_a_hidden_file(request.uri().path()) {
        return StatusCode::NOT_FOUND.into_response();
    }

    next.run(request).await
}

/// True for a name the server will not serve: anything hidden, apart from
/// the one directory the web uses.
pub(crate) fn is_hidden(name: &str) -> bool {
    name.starts_with('.') && name != WELL_KNOWN
}

/// True for an address naming a hidden file. The address is decoded first,
/// as the file service decodes it: `%2e` is a dot and `%2f` a separator, so
/// a check on the raw text would miss `/sub%2f.env`.
fn names_a_hidden_file(path: &str) -> bool {
    decode(path).split(SEPARATORS).any(is_hidden)
}

/// Decodes each `%XX` once, as the file service does.
fn decode(path: &str) -> Cow<'_, str> {
    percent_decode_str(path).decode_utf8_lossy()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_addresses_are_refused() {
        for path in [
            "/.env",
            "/.git/config",
            "/%2e%65nv",
            "/%2Eenv",
            "/js/.hidden.js",
            "/.well-known/.secret",
        ] {
            assert!(names_a_hidden_file(path), "{path}");
        }
    }

    #[test]
    fn an_encoded_separator_does_not_hide_a_hidden_file() {
        // The file service reads %2f as a separator, so this check must too.
        for path in ["/sub%2f.env", "/sub%2F.env", "/sub%2f.git%2fconfig"] {
            assert!(names_a_hidden_file(path), "{path}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn a_backslash_is_an_ordinary_character_in_a_name_here() {
        // The file service would serve a file called `sub\.env`, so the
        // backslash must not be read as a separator here.
        assert!(!names_a_hidden_file("/sub%5c.env"));
    }

    #[test]
    fn ordinary_addresses_are_allowed() {
        for path in ["/", "/index.html", "/a.b/c.js", "/.well-known/acme/token"] {
            assert!(!names_a_hidden_file(path), "{path}");
        }
    }
}
