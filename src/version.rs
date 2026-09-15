//! Whether a browser's copy of a file is still current, told by which file it
//! is, its size and the time it was written, to the nanosecond where the disk
//! keeps it. The file service compares whole seconds only, so it would answer a
//! file rolled back to an older copy, or saved twice within a second, as
//! unchanged.

use crate::serve::INDEX_FILE;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use file_id::FileId;
use http::{HeaderValue, Method, StatusCode, header};
use percent_encoding::percent_decode_str;
use std::fs::Metadata;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

/// A file as it was when it was looked at.
pub(crate) struct Version {
    tag: HeaderValue,
    written_second: u64,
}

impl Version {
    /// None where the system cannot say when the file was written. `identity`
    /// is which file this is, where the system can say.
    pub(crate) fn of(identity: Option<FileId>, metadata: &Metadata) -> Option<Version> {
        let written = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;

        // One number made from all the parts, so the tag shows nothing of the
        // disk, such as an inode. The same parts give the same number on every
        // run.
        let mut parts = DefaultHasher::new();
        (
            identity,
            metadata.len(),
            written.as_secs(),
            written.subsec_nanos(),
        )
            .hash(&mut parts);
        // Weak, since compression and the reload script change the bytes sent
        // without changing the file.
        let tag = format!("W/\"{:016x}\"", parts.finish());

        Some(Version {
            tag: HeaderValue::try_from(tag).ok()?,
            written_second: written.as_secs(),
        })
    }

    pub(crate) fn tag(&self) -> HeaderValue {
        self.tag.clone()
    }

    /// Whether an `If-None-Match` names this version. Tags are compared without
    /// their weak mark, as that header asks.
    pub(crate) fn is_named_in(&self, if_none_match: &HeaderValue) -> bool {
        let Ok(tags) = if_none_match.to_str() else {
            return false;
        };
        let ours = without_weak_mark(self.tag.to_str().unwrap_or_default());

        tags.split(',')
            .map(str::trim)
            .any(|tag| tag == "*" || without_weak_mark(tag) == ours)
    }

    /// Whether a date names the second this version was written.
    fn was_written_at(&self, date: &HeaderValue) -> bool {
        date.to_str()
            .ok()
            .and_then(|date| httpdate::parse_http_date(date).ok())
            .and_then(|date| date.duration_since(UNIX_EPOCH).ok())
            .is_some_and(|date| date.as_secs() == self.written_second)
    }
}

/// Which file this is. Some builds give every file one fixed time, and then a
/// change that keeps the length shows only as a different file.
pub(crate) fn identity(path: &Path) -> Option<FileId> {
    file_id::get_file_id(path).ok()
}

fn without_weak_mark(tag: &str) -> &str {
    tag.strip_prefix("W/").unwrap_or(tag)
}

/// Answers "is my copy still current?" for a file on disk, and tags the file
/// when it is sent. The questions come off the request either way, so the file
/// service never answers them by date.
pub(crate) async fn answer_by_version(root: PathBuf, mut request: Request, next: Next) -> Response {
    let version = match *request.method() {
        Method::GET | Method::HEAD => file_version(&root, request.uri().path()),
        _ => None,
    };

    let headers = request.headers_mut();
    let if_none_match = headers.remove(header::IF_NONE_MATCH);
    let if_modified_since = headers.remove(header::IF_MODIFIED_SINCE);
    let if_range = headers.remove(header::IF_RANGE);

    let Some(version) = version else {
        return next.run(request).await;
    };

    // A browser sends both, and then the tag decides, as HTTP says it should.
    let unchanged = match (if_none_match, if_modified_since) {
        (Some(tags), _) => version.is_named_in(&tags),
        (None, Some(date)) => version.was_written_at(&date),
        (None, None) => false,
    };
    if unchanged {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, version.tag())]).into_response();
    }

    // A part of the file as it is now would not fit a copy of how it was.
    if if_range.is_some_and(|date| !version.was_written_at(&date)) {
        request.headers_mut().remove(header::RANGE);
    }

    let mut response = next.run(request).await;
    if response.status().is_success() {
        response.headers_mut().insert(header::ETAG, version.tag());
    }

    response
}

/// The version of the file the file service would answer `path` with, found
/// the way it finds it.
fn file_version(root: &Path, path: &str) -> Option<Version> {
    let decoded = percent_decode_str(path.trim_start_matches('/'))
        .decode_utf8()
        .ok()?;
    let mut file = root.to_path_buf();
    for part in Path::new(&*decoded).components() {
        match part {
            Component::Normal(name)
                if Path::new(name)
                    .components()
                    .all(|inner| matches!(inner, Component::Normal(_))) =>
            {
                file.push(name);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }

    if file.is_dir() {
        // Without its slash, a folder's address is redirected, not answered.
        if !path.ends_with('/') {
            return None;
        }
        file.push(INDEX_FILE);
    }

    let metadata = std::fs::metadata(&file).ok()?;
    if !metadata.is_file() {
        return None;
    }

    Version::of(identity(&file), &metadata)
}
