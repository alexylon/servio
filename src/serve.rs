//! How a request is answered: the file service, and the layers around it.

use crate::guard::{
    may_be_served, refuse_hidden_files, refuse_index_pages_outside, refuse_paths_outside,
};
use crate::list::serve_file_list;
use axum::Router;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use http::uri::PathAndQuery;
use http::{HeaderName, HeaderValue, StatusCode, Uri, header};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_livereload::LiveReloadLayer;

pub(crate) const INDEX_FILE: &str = "index.html";

/// Where a build puts files whose name changes with their contents, so the
/// browser can keep them for as long as it likes.
const ASSETS: &str = "/assets/";

/// A year, the longest any browser is asked to keep a file.
const KEEP_FOR_A_YEAR: &str = "public, max-age=31536000, immutable";

/// The file service and the layers around it. Each layer added wraps the ones
/// before it, and the order matters:
/// - the index.html check sits right next to the file service, so `?list`
///   never reaches it;
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
    cache_assets: bool,
    livereload: Option<LiveReloadLayer>,
    no_app_page: bool,
) -> Router {
    let mut app = Router::new().fallback_service(ServeDir::new(static_dir));

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

    if cache_assets {
        app = app.layer(middleware::from_fn(keep_hashed_assets));
    } else {
        // Never let the browser hold on to a stale file.
        app = app
            .layer(middleware::from_fn(always_answer_in_full))
            .layer(set_header(header::CACHE_CONTROL, "no-store"));
    }

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
    request: Request,
    next: Next,
) -> Response {
    let wants_page = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|accept| accept.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));

    // A name under /assets/ carries a hash of the file's contents, so it is
    // a built file, never a route. Handing the app back there would leave the
    // browser keeping a page at that address for a year.
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
        tokio::fs::read(&index).await.map_err(|error| {
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
        Ok(page) => {
            // Back again, so the next time it goes is worth saying.
            *missing = false;
            ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response()
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

/// What a published site tells the browser to keep. Names under `/assets/`
/// carry a hash of their contents, so the file at one of those addresses
/// never changes. Everything else is checked each time.
async fn keep_hashed_assets(request: Request, next: Next) -> Response {
    // An address ending in `/` is a folder, never a hashed file.
    let path = request.uri().path();
    let hashed = path.starts_with(ASSETS) && !path.ends_with('/');
    let mut response = next.run(request).await;

    // Only a file that is really there: one missing during a deploy would
    // otherwise be remembered as missing for a year. "You already have it"
    // counts, since the browser takes the headers on that answer as the
    // file's own.
    let there = response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED;
    let keep = hashed && there;

    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if keep { KEEP_FOR_A_YEAR } else { "no-cache" }),
    );

    response
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
