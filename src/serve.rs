//! How a request is answered: the file service, and the layers around it.

use crate::guard::{
    may_be_served, refuse_hidden_files, refuse_index_pages_outside, refuse_paths_outside,
};
use crate::list::serve_file_list;
use crate::version::{Version, answer_by_version, identity};
use axum::Router;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use http::uri::PathAndQuery;
use http::{HeaderName, HeaderValue, StatusCode, Uri, header};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_livereload::LiveReloadLayer;

pub(crate) const INDEX_FILE: &str = "index.html";

/// What browsers may keep.
#[derive(Clone, Copy)]
pub(crate) enum Caching {
    /// Nothing, since any file may have just changed.
    Off,
    /// Anything, checked for changes before each use. For `--production`.
    Checked,
    /// Files under /assets/ for a year without a check, and the rest checked.
    /// For `--cache-assets`.
    AssetsForAYear,
}

/// Where a build puts scripts, styles and images. `--cache-assets` takes a
/// name here to change whenever the file's contents do.
const ASSETS: &str = "/assets/";

/// A year, the longest any browser is asked to keep a file.
const KEEP_FOR_A_YEAR: &str = "public, max-age=31536000, immutable";

/// Keep it, but ask the server whether it changed before using it.
const CHECK_BEFORE_USE: &str = "no-cache";

/// The file service and the layers around it. Each layer added wraps the ones
/// before it, and the order matters:
/// - the file service's own tags come off right next to it, so the only tags
///   that reach a browser are the ones version checks add;
/// - version checks sit just outside that, inside every refusal, so nothing
///   refused is ever answered as unchanged;
/// - the index.html check sits just outside them, so `?list` never reaches it;
/// - the refusals sit inside the headers, so a refusal gets the same headers;
/// - the app shell and file lists sit inside live reload, so their pages get
///   the reload script;
/// - file lists sit outside the app shell, so an app route with the same name
///   as a folder still gets the app;
/// - leading slashes are collapsed outside everything, so every layer sees the
///   same address.
pub(crate) fn app(
    static_dir: &Path,
    spa: bool,
    list: bool,
    caching: Caching,
    livereload: Option<LiveReloadLayer>,
    no_app_page: bool,
) -> Router {
    let mut app = Router::new()
        .fallback_service(ServeDir::new(static_dir))
        .layer(middleware::from_fn(drop_file_service_tags));

    // With nothing kept, there is no copy to ask about.
    let checks_versions = !matches!(caching, Caching::Off);
    if checks_versions {
        let root = static_dir.to_path_buf();
        app = app.layer(middleware::from_fn(move |request, next| {
            answer_by_version(root.clone(), request, next)
        }));
    }

    let root = static_dir.to_path_buf();
    app = app.layer(middleware::from_fn(move |request, next| {
        refuse_index_pages_outside(root.clone(), request, next)
    }));

    if spa {
        let root = static_dir.to_path_buf();
        let index = static_dir.join(INDEX_FILE);
        // Whether the app page is missing, as far as anyone has said out
        // loud. The banner is about to warn about the same look, so the first
        // request should not say it again.
        let missing = Arc::new(Mutex::new(no_app_page));
        app = app.layer(middleware::from_fn(move |request, next| {
            serve_app_shell(
                root.clone(),
                index.clone(),
                Arc::clone(&missing),
                checks_versions,
                request,
                next,
            )
        }));
    }

    if list {
        let root = static_dir.to_path_buf();
        app = app.layer(middleware::from_fn(move |request, next| {
            serve_file_list(root.clone(), request, next)
        }));
    }

    if let Some(livereload) = livereload {
        app = app.layer(livereload);
    }

    let root = static_dir.to_path_buf();
    let mut app = app
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(refuse_hidden_files))
        .layer(middleware::from_fn(move |request, next| {
            refuse_paths_outside(root.clone(), request, next)
        }));

    app = match caching {
        Caching::Off => app
            .layer(middleware::from_fn(always_answer_in_full))
            .layer(set_header(header::CACHE_CONTROL, "no-store")),
        Caching::Checked => app.layer(set_header(header::CACHE_CONTROL, CHECK_BEFORE_USE)),
        Caching::AssetsForAYear => app.layer(middleware::from_fn(keep_assets)),
    };

    app.layer(set_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(set_header(header::X_FRAME_OPTIONS, "SAMEORIGIN"))
        .layer(set_header(
            header::REFERRER_POLICY,
            "strict-origin-when-cross-origin",
        ))
        .layer(middleware::from_fn(collapse_leading_slashes))
}

/// Collapses repeated slashes at the start of the address into one, as nginx
/// and Python's server do. Otherwise, with a folder named `example.com`,
/// `//example.com` redirects to `//example.com/`, which a browser treats as
/// another site. Backslashes count too: browsers read `/\` as `//`.
async fn collapse_leading_slashes(mut request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let rest = path.trim_start_matches(['/', '\\']);
    if path.len() - rest.len() > 1 {
        let collapsed = match request.uri().query() {
            Some(query) => format!("/{rest}?{query}"),
            None => format!("/{rest}"),
        };

        // Can't fail for a valid address; answer 400 if it somehow does.
        let Ok(path_and_query) = PathAndQuery::try_from(collapsed) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let mut parts = request.uri().clone().into_parts();
        parts.path_and_query = Some(path_and_query);
        let Ok(uri) = Uri::from_parts(parts) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        *request.uri_mut() = uri;
    }

    next.run(request).await
}

/// With `--spa`, an address matching no file gets index.html so the app can
/// route it. Only for page requests: a missing script or image still gets a
/// 404, rather than HTML the browser then refuses to run.
async fn serve_app_shell(
    root: PathBuf,
    index: PathBuf,
    missing: Arc<Mutex<bool>>,
    checks_versions: bool,
    request: Request,
    next: Next,
) -> Response {
    let if_none_match = request.headers().get(header::IF_NONE_MATCH).cloned();
    let wants_page = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|accept| accept.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));

    // Anything under /assets/ is a built file, never a route. Handing the app
    // back there could leave the browser keeping a page at that address for a
    // year.
    let could_be_a_route = !request.uri().path().starts_with(ASSETS);

    let response = next.run(request).await;
    if response.status() != StatusCode::NOT_FOUND || !wants_page || !could_be_a_route {
        // Another file answering says nothing about the app page, and `/` is
        // served from disk without ever reading it. So while the page is
        // thought missing, look: it may be back.
        let mut missing = missing.lock().await;
        if *missing && is_there(&root, &index).await {
            *missing = false;
        }

        return response;
    }

    // Held across the read, so two requests arriving together cannot record
    // what they found in the order they finished rather than the order they
    // looked, leaving the page down as gone while it is there.
    let mut missing = missing.lock().await;
    let page = if may_be_served(&root, &index) {
        read_page(&index).await.map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                "is gone"
            } else {
                "cannot be read"
            }
        })
    } else {
        // Refuse it here too, as the guards do everywhere else.
        Err("leads to a file that is never served")
    };

    match page {
        Ok((page, version)) => {
            // Back again, so the next time it goes is worth saying.
            *missing = false;

            // The page is the same at every route, so a copy kept for one is
            // current as long as index.html is.
            let html = HeaderValue::from_static("text/html; charset=utf-8");
            match version.filter(|_| checks_versions) {
                Some(version) if if_none_match.is_some_and(|tags| version.is_named_in(&tags)) => {
                    (StatusCode::NOT_MODIFIED, [(header::ETAG, version.tag())]).into_response()
                }
                Some(version) => (
                    [(header::CONTENT_TYPE, html), (header::ETAG, version.tag())],
                    page,
                )
                    .into_response(),
                None => ([(header::CONTENT_TYPE, html)], page).into_response(),
            }
        }
        // The 404 stands, but say why: the banner looked only at startup, so
        // a build that clears the directory takes the page away with nobody
        // watching.
        Err(reason) => {
            // Once each time it goes, not once for each address. A build
            // that makes the directory before writing the page can set this
            // off for a gap that closes itself; it was true when it was said.
            if !std::mem::replace(&mut *missing, true) {
                eprintln!("  {INDEX_FILE} {reason}, so the app will not load");
            }

            response
        }
    }
}

/// While you are working, the browser only asks whether a file changed
/// because you just saved it, so "nothing has changed" is never the right
/// answer. Range requests are left alone, so seeking in audio and video
/// still works.
async fn always_answer_in_full(mut request: Request, next: Next) -> Response {
    let headers = request.headers_mut();
    headers.remove(header::IF_MODIFIED_SINCE);
    headers.remove(header::IF_NONE_MATCH);

    next.run(request).await
}

/// Keeps a file under /assets/ for a year, and has everything else checked.
/// This relies on a file there getting a new name whenever it changes, as the
/// hashed names from a build do; nothing here checks.
async fn keep_assets(request: Request, next: Next) -> Response {
    // An address ending in `/` is a folder, whose page can change.
    let path = request.uri().path();
    let asset = path.starts_with(ASSETS) && !path.ends_with('/');
    let mut response = next.run(request).await;

    // Only a file that is really there: one missing during a deploy would
    // otherwise be remembered as missing for a year. "You already have it"
    // counts, since the browser takes the headers on that answer as the
    // file's own.
    let there = response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED;
    let keep = asset && there;

    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if keep {
            KEEP_FOR_A_YEAR
        } else {
            CHECK_BEFORE_USE
        }),
    );

    response
}

/// The file service tags each file it sends, by size and write time, and its
/// tags promise the exact bytes of the file. A browser often gets other bytes:
/// compressed, or a page with the reload script added. The only tags sent are
/// the ones version.rs makes, and only where a browser may keep the file.
async fn drop_file_service_tags(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().remove(header::ETAG);
    response
}

/// The page, and its version as it was read. The version is left out if the
/// page was replaced meanwhile, since its parts could then describe another
/// file than the one read.
async fn read_page(index: &Path) -> std::io::Result<(Vec<u8>, Option<Version>)> {
    let before = identity(index);
    let mut file = tokio::fs::File::open(index).await?;
    let metadata = file.metadata().await.ok();
    let mut page = Vec::new();
    file.read_to_end(&mut page).await?;

    let version = metadata
        .filter(|_| identity(index) == before)
        .and_then(|metadata| Version::of(before, &metadata));

    Ok((page, version))
}

/// Whether the app page can be served. An unreadable page, or a link the
/// server refuses, counts as missing. Only checked while the page is thought
/// missing.
async fn is_there(root: &Path, index: &Path) -> bool {
    if !may_be_served(root, index) {
        return false;
    }

    let Ok(page) = tokio::fs::File::open(index).await else {
        return false;
    };

    page.metadata().await.is_ok_and(|found| found.is_file())
}

fn set_header(name: HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}
