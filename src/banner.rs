//! What the server says on the way up.

use crate::Args;
use crate::ignore::IGNORE_FILE;
use crate::listen::DEFAULT_PORT;
use crate::serve::{Caching, INDEX_FILE};
use crate::watch::POLL_INTERVAL;
use std::fmt::Display;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;

const BLUE: &str = "\x1b[94m";
const RESET: &str = "\x1b[0m";
const LINK_START: &str = "\x1b]8;;";
const LINK_END: &str = "\x1b]8;;\x1b\\";
const LINK_MID: &str = "\x1b\\";
const RULE: &str = "-----------------------------------------------";
/// Fits the longest label, so the colons line up.
const LABEL_WIDTH: usize = 15;

pub(crate) fn print(
    bound: SocketAddr,
    args: &Args,
    static_dir: &Path,
    no_app_page: bool,
    from_ignore_file: usize,
) {
    let authority = authority(bound);
    let url = url(bound);

    println!("{RULE}");
    row("Serving", static_dir.display());
    if args.port.is_none() && bound.port() != DEFAULT_PORT {
        row(
            "Note",
            format!("port {DEFAULT_PORT} was busy, using {}", bound.port()),
        );
    }
    row("Live reload", live_reload(args));
    if let Some(ignoring) = ignoring(&args.ignore, from_ignore_file) {
        row("Ignoring", ignoring);
    }
    row("Single-page app", on_off(args.spa));
    row(
        "File lists",
        if args.no_list {
            "off"
        } else {
            "where a folder has no index.html, or with ?list"
        },
    );
    row(
        "Caching",
        match args.caching() {
            Caching::Off => "off",
            Caching::Checked => "on, checked for changes each time",
            Caching::AssetsForAYear => {
                "files under /assets/ for a year, the rest checked each time"
            }
        },
    );
    if no_app_page {
        row(
            "Warning",
            format!("there is no {INDEX_FILE} here, so the app will not load"),
        );
    }
    if !args.no_list && reachable_from_elsewhere(args.host) {
        row(
            "Warning",
            "other devices can list the files here; --no-list turns that off",
        );
    }
    row("Open", hyperlink(&url, &authority));
    println!("{RULE}\n");
}

/// The address a browser can open.
pub(crate) fn url(bound: SocketAddr) -> String {
    format!("http://{}", authority(bound))
}

/// The host and port of that address. 0.0.0.0 and [::] mean every network
/// interface, which a browser cannot open, so those become localhost. So do
/// 127.0.0.1 and ::1, and nothing else: 127.0.0.2 is loopback too, but the
/// name does not lead there.
fn authority(bound: SocketAddr) -> String {
    let ip = bound.ip();
    let is_localhost = ip.is_unspecified()
        || ip == IpAddr::V4(Ipv4Addr::LOCALHOST)
        || ip == IpAddr::V6(Ipv6Addr::LOCALHOST);

    if is_localhost {
        format!("localhost:{}", bound.port())
    } else {
        bound.to_string()
    }
}

/// Whether other machines can reach this address. `::ffff:127.0.0.1` counts
/// as local.
fn reachable_from_elsewhere(host: IpAddr) -> bool {
    !host.to_canonical().is_loopback()
}

fn row(label: &str, value: impl Display) {
    println!("  {label:<LABEL_WIDTH$}: {BLUE}{value}{RESET}");
}

/// Worth saying when the server is looking at the files rather than being told
/// about them: that notices later, and reads the whole directory each time.
fn live_reload(args: &Args) -> String {
    if args.no_reload {
        return "off".to_string();
    }

    if args.poll {
        return format!("on, looking at the files every {POLL_INTERVAL:?}");
    }

    "on".to_string()
}

/// The patterns given, and how many the ignore file added, or nothing when
/// there were none of either.
fn ignoring(given: &[String], from_file: usize) -> Option<String> {
    let mut parts: Vec<String> = given.to_vec();
    match from_file {
        0 => {}
        1 => parts.push(format!("1 pattern in {IGNORE_FILE}")),
        many => parts.push(format!("{many} patterns in {IGNORE_FILE}")),
    }

    (!parts.is_empty()).then(|| parts.join(", "))
}

fn on_off(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

/// Makes `text` a clickable link to `url` in terminals that support it.
fn hyperlink(url: &str, text: impl Display) -> String {
    format!("{LINK_START}{url}{LINK_MID}{text}{LINK_END}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_keeps_the_lists_to_this_machine() {
        for local in ["127.0.0.1", "127.0.0.2", "::1", "::ffff:127.0.0.1"] {
            let host: IpAddr = local.parse().unwrap();
            assert!(!reachable_from_elsewhere(host), "{local}");
        }
        for open in ["0.0.0.0", "::", "192.168.1.20", "::ffff:192.168.1.20"] {
            let host: IpAddr = open.parse().unwrap();
            assert!(reachable_from_elsewhere(host), "{open}");
        }
    }

    #[test]
    fn what_is_ignored_is_listed_and_the_file_is_counted() {
        let given = ["*.map".to_string()];

        assert_eq!(ignoring(&[], 0), None);
        assert_eq!(ignoring(&given, 0).as_deref(), Some("*.map"));
        assert_eq!(
            ignoring(&given, 1).as_deref(),
            Some("*.map, 1 pattern in .servioignore")
        );
        assert_eq!(
            ignoring(&[], 2).as_deref(),
            Some("2 patterns in .servioignore")
        );
    }
}
