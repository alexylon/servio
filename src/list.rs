//! File lists: a page of a folder's files, shown when the folder has no
//! index.html or the address ends in `?list`. `--no-list` turns them off.

use crate::guard::{is_hidden, may_be_served};
use crate::serve::INDEX_FILE;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::{Method, StatusCode, header};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use std::fmt::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The exact query that asks for a list. Anything else, like
/// `?list=groceries`, is left to the page.
const ASKS_FOR_THE_LIST: &str = "list";

/// Characters written as `%XX` in a link: all but letters, digits and `-._~`,
/// so a `#`, `?` or `:` in a file name can't break the link.
const IN_A_LINK: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Works in light and dark mode. The size and date columns stay narrow and
/// long names wrap, so the table fits on a phone.
const STYLE: &str = "
:root { color-scheme: light dark; }
body { max-width: 64rem; margin: 2rem auto; padding: 0 1rem; font: 15px/1.5 system-ui, sans-serif; }
h1 { margin: 0 0 1rem; font-size: 1.25rem; font-weight: 600; overflow-wrap: anywhere; }
table { width: 100%; border-collapse: collapse; }
th { text-align: left; font-weight: 500; opacity: .65; border-bottom: 1px solid rgba(127, 127, 127, .3); }
th, td { padding: .3rem .6rem; }
td:first-child { overflow-wrap: anywhere; }
th + th, td + td { width: 1%; white-space: nowrap; font-variant-numeric: tabular-nums; }
tbody tr:hover { background: rgba(127, 127, 127, .1); }
.size { text-align: right; }
.empty { opacity: .65; }
a { text-decoration: none; }
a:hover { text-decoration: underline; }
";

/// Shows each time in the reader's time zone. The page is written in UTC, and
/// a date the browser can't read keeps its UTC text.
const LOCAL_TIME: &str = r#"{
const two = (number) => String(number).padStart(2, "0");
for (const time of document.querySelectorAll("time")) {
  const at = new Date(time.dateTime);
  if (Number.isNaN(at.getTime())) continue;
  time.textContent = `${at.getFullYear()}-${two(at.getMonth() + 1)}-${two(at.getDate())} ${two(at.getHours())}:${two(at.getMinutes())}`;
}
}"#;

/// Shows a folder's files instead of a 404 when it has no index.html, and for
/// any folder when the address ends in `?list`. Everything else passes through.
pub(crate) async fn serve_file_list(root: PathBuf, request: Request, next: Next) -> Response {
    let address = request.uri().path().to_string();
    let asked = request.uri().query() == Some(ASKS_FOR_THE_LIST);

    // Only addresses ending in `/` get a list. The file service first
    // redirects `/docs` to `/docs/`, which keeps the relative links working.
    let reads = matches!(*request.method(), Method::GET | Method::HEAD);
    if !reads || !address.ends_with('/') {
        return next.run(request).await;
    }

    if asked {
        return match list(root, &address, asked).await {
            Some(list) => list,
            None => next.run(request).await,
        };
    }

    // Without `?list`, the folder's own page comes first.
    let response = next.run(request).await;
    if response.status() != StatusCode::NOT_FOUND {
        return response;
    }

    list(root, &address, asked).await.unwrap_or(response)
}

/// The list page for `address`, or `None` if there is no readable folder
/// there.
async fn list(root: PathBuf, address: &str, asked: bool) -> Option<Response> {
    let folder = folder_at(&root, address)?;
    let at_top = folder == root;

    // Reading a folder touches the disk once per entry, so do it on a
    // separate thread.
    let entries = tokio::task::spawn_blocking(move || {
        // Without `?list`, don't list a folder that has an index.html, even a
        // broken one: the list would hide that the page is broken.
        let has_a_page = folder.join(INDEX_FILE).symlink_metadata().is_ok();
        if has_a_page && !asked {
            return None;
        }
        read(&root, &folder)
    })
    .await
    .ok()
    .flatten()?;

    let shown = percent_decode_str(address).decode_utf8_lossy();
    let html = page(&shown, &entries, at_top, asked);
    Some(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response())
}

/// The folder an address points to, worked out the same way the file service
/// does. `None` if the address could lead out of the served folder.
fn folder_at(root: &Path, address: &str) -> Option<PathBuf> {
    let decoded = percent_decode_str(address.trim_start_matches('/'))
        .decode_utf8()
        .ok()?;

    let mut folder = root.to_path_buf();
    for part in Path::new(&*decoded).components() {
        match part {
            // A plain name only, not something like `c:` on Windows.
            Component::Normal(name)
                if Path::new(name)
                    .components()
                    .all(|inner| matches!(inner, Component::Normal(_))) =>
            {
                folder.push(name);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }

    Some(folder)
}

struct Entry {
    name: String,
    is_folder: bool,
    size: u64,
    modified: Option<SystemTime>,
}

/// The folder's entries that the server would serve, folders first, then by
/// name. `None` if it isn't a folder the server can read and serve.
fn read(root: &Path, folder: &Path) -> Option<Vec<Entry>> {
    // Once the folder itself is checked, only links inside it need checking.
    if !folder.is_dir() || !may_be_served(root, folder) {
        return None;
    }

    let mut entries = Vec::new();
    for found in std::fs::read_dir(folder).ok()?.flatten() {
        // Skip names that aren't valid UTF-8; no address can reach them.
        let Ok(name) = found.file_name().into_string() else {
            continue;
        };

        // Skip anything the server refuses to serve.
        let is_link = found.file_type().is_ok_and(|kind| kind.is_symlink());
        if is_hidden(&name) || (is_link && !may_be_served(root, &found.path())) {
            continue;
        }

        // Follow links. Skip broken ones, and anything that isn't a file or
        // folder.
        let Ok(details) = std::fs::metadata(found.path()) else {
            continue;
        };
        if !details.is_dir() && !details.is_file() {
            continue;
        }

        entries.push(Entry {
            name,
            is_folder: details.is_dir(),
            size: details.len(),
            modified: details.modified().ok(),
        });
    }

    entries.sort_by_cached_key(|entry| {
        (
            !entry.is_folder,
            entry.name.to_lowercase(),
            entry.name.clone(),
        )
    });
    Some(entries)
}

/// Builds the list page. Links are relative, so they still work when the site
/// is served under a longer path.
fn page(shown: &str, entries: &[Entry], at_top: bool, asked: bool) -> String {
    let title = escape(shown);
    // On a `?list` page, folder links keep `?list`, so browsing down doesn't
    // stop at a folder that has its own page.
    let asking = if asked { "?list" } else { "" };

    let mut html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Index of {title}</title>
<style>{STYLE}</style>
</head>
<body>
<h1>Index of {title}</h1>
<table>
<thead><tr><th>Name</th><th class="size">Size</th><th>Modified</th></tr></thead>
<tbody>
"#
    );

    // Writing to a String can't fail.
    if !at_top {
        let _ = writeln!(
            html,
            r#"<tr><td><a href="../{asking}">../</a></td><td></td><td></td></tr>"#
        );
    }
    if entries.is_empty() {
        html.push_str(r#"<tr><td class="empty" colspan="3">This folder is empty</td></tr>"#);
        html.push('\n');
    }
    for entry in entries {
        let name = escape(&entry.name);
        let link = utf8_percent_encode(&entry.name, IN_A_LINK);
        let written = entry.modified.map(written_at).unwrap_or_default();

        if entry.is_folder {
            let _ = writeln!(
                html,
                r#"<tr><td><a href="{link}/{asking}">{name}/</a></td><td class="size"></td><td>{written}</td></tr>"#
            );
        } else {
            let size = readable_size(entry.size);
            let _ = writeln!(
                html,
                r#"<tr><td><a href="{link}">{name}</a></td><td class="size">{size}</td><td>{written}</td></tr>"#
            );
        }
    }

    let _ = writeln!(
        html,
        "</tbody>\n</table>\n<script>{LOCAL_TIME}</script>\n</body>\n</html>"
    );
    html
}

/// Escapes text for HTML, including inside quoted attributes.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Formats a size like `512 B`, `1.5 KB` or `3.4 MB`.
fn readable_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];

    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    // Switch units just below 1024, so 1023.96 KB shows as 1.0 MB, not
    // 1024.0 KB.
    while size >= 1023.95 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }

    format!("{size:.1} {}", UNITS[unit])
}

/// A `<time>` tag for when a file last changed, in UTC to the minute.
fn written_at(time: SystemTime) -> String {
    let (date, minute) = utc_minute(seconds_since_1970(time));
    format!(r#"<time datetime="{date}T{minute}Z">{date} {minute} UTC</time>"#)
}

fn seconds_since_1970(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        // Before 1970, round down: half a second before counts as -1.
        Err(before) => {
            let gap = before.duration();
            let whole = i64::try_from(gap.as_secs()).unwrap_or(i64::MAX);
            -whole - i64::from(gap.subsec_nanos() > 0)
        }
    }
}

/// Turns seconds since 1970 into a UTC date and time of day, like
/// `("2026-09-14", "14:51")`. Uses Howard Hinnant's `civil_from_days`
/// method, which starts each year in March so leap days fall at the end.
fn utc_minute(seconds: i64) -> (String, String) {
    let days = seconds.div_euclid(86_400);
    let minutes = seconds.rem_euclid(86_400) / 60;

    let from_march = days + 719_468;
    let era = from_march.div_euclid(146_097);
    let day_of_era = from_march.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;

    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    (
        format!("{year:04}-{month:02}-{day:02}"),
        format!("{:02}:{:02}", minutes / 60, minutes % 60),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_names_a_folder_only_inside_the_served_directory() {
        let root = Path::new("/site");

        assert_eq!(folder_at(root, "/"), Some(root.to_path_buf()));
        assert_eq!(
            folder_at(root, "/docs/api/"),
            Some(root.join("docs").join("api"))
        );
        assert_eq!(folder_at(root, "/./docs/"), Some(root.join("docs")));
        assert_eq!(folder_at(root, "/caf%C3%A9/"), Some(root.join("café")));

        // Leading out, plainly or encoded; an encoded slash; invalid UTF-8.
        for address in ["/../", "/docs/../../", "/%2e%2e/", "/%2Fetc/", "/%FF/"] {
            assert_eq!(folder_at(root, address), None, "{address}");
        }
    }

    #[test]
    fn a_name_in_a_link_stays_one_name() {
        let link = |name: &str| utf8_percent_encode(name, IN_A_LINK).to_string();

        assert_eq!(link("a b#c?d:e%f"), "a%20b%23c%3Fd%3Ae%25f");
        assert_eq!(link("café"), "caf%C3%A9");
        assert_eq!(link("A-z_0.9~"), "A-z_0.9~");
    }

    #[test]
    fn a_name_on_the_page_stays_text() {
        assert_eq!(
            escape(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn sizes_read_as_a_person_would_say_them() {
        assert_eq!(readable_size(0), "0 B");
        assert_eq!(readable_size(1023), "1023 B");
        assert_eq!(readable_size(1024), "1.0 KB");
        assert_eq!(readable_size(1536), "1.5 KB");
        assert_eq!(readable_size(1024 * 1024 - 1), "1.0 MB");
        assert_eq!(readable_size(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn a_time_before_1970_falls_in_the_second_before_it() {
        use std::time::Duration;

        let before = |millis| UNIX_EPOCH - Duration::from_millis(millis);
        assert_eq!(seconds_since_1970(before(500)), -1);
        assert_eq!(seconds_since_1970(before(1_000)), -1);
        assert_eq!(seconds_since_1970(before(60_500)), -61);
        assert_eq!(
            seconds_since_1970(UNIX_EPOCH + Duration::from_millis(1_500)),
            1
        );
        assert_eq!(utc_minute(seconds_since_1970(before(500))).1, "23:59");
    }

    #[test]
    fn times_are_written_in_utc_to_the_minute() {
        for (seconds, date, minute) in [
            (0, "1970-01-01", "00:00"),
            (951_868_740, "2000-02-29", "23:59"),
            (1_789_397_460, "2026-09-14", "14:51"),
            (-60, "1969-12-31", "23:59"),
        ] {
            assert_eq!(
                utc_minute(seconds),
                (date.to_string(), minute.to_string()),
                "{seconds}"
            );
        }
    }
}
