//! What the server says on the way up.

use crate::Args;
use crate::ignore::IGNORE_FILE;
use crate::listen::DEFAULT_PORT;
use crate::serve::{Caching, INDEX_FILE};
use crate::watch::POLL_INTERVAL;
use std::ffi::OsStr;
use std::fmt::Display;
use std::io::IsTerminal;
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

/// Whether the banner may colour its text and make links.
#[derive(Clone, Copy)]
struct Style {
    colour: bool,
    links: bool,
}

impl Style {
    fn for_stdout() -> Style {
        Style::new(
            std::io::stdout().is_terminal(),
            std::env::var_os("TERM").as_deref(),
            std::env::var_os("NO_COLOR").as_deref(),
        )
    }

    /// Colours and links only for a terminal that shows them. In a file or a
    /// pipe their codes are noise, and so they are in a terminal that calls
    /// itself dumb, such as an Emacs shell, or outside Windows names no kind at
    /// all. A `NO_COLOR` that is not empty turns colour off as well.
    fn new(terminal: bool, term: Option<&OsStr>, no_color: Option<&OsStr>) -> Style {
        let shows_codes = terminal
            && match term {
                Some(kind) => kind != "dumb",
                // Terminals on Windows set no TERM.
                None => cfg!(windows),
            };
        let no_color = no_color.is_some_and(|value| !value.is_empty());

        Style {
            colour: shows_codes && !no_color,
            links: shows_codes,
        }
    }

    fn row(self, label: &str, value: impl Display) {
        if self.colour {
            println!("  {label:<LABEL_WIDTH$}: {BLUE}{value}{RESET}");
        } else {
            println!("  {label:<LABEL_WIDTH$}: {value}");
        }
    }

    /// Makes `text` a clickable link to `url` in terminals that support it.
    fn link(self, url: &str, text: impl Display) -> String {
        if self.links {
            format!("{LINK_START}{url}{LINK_MID}{text}{LINK_END}")
        } else {
            text.to_string()
        }
    }
}

pub(crate) fn print(
    bound: SocketAddr,
    args: &Args,
    static_dir: &Path,
    no_app_page: bool,
    from_ignore_file: usize,
) {
    let style = Style::for_stdout();
    let authority = authority(bound);
    let url = url(bound);

    println!("{RULE}");
    style.row("Serving", static_dir.display());
    if args.exact_port().is_none() && bound.port() != DEFAULT_PORT {
        style.row(
            "Note",
            format!("port {DEFAULT_PORT} was busy, using {}", bound.port()),
        );
    }
    style.row("Live reload", live_reload(args));
    if let Some(ignoring) = ignoring(&args.ignore, from_ignore_file) {
        style.row("Ignoring", ignoring);
    }
    style.row("Single-page app", on_off(args.spa));
    style.row(
        "File lists",
        if args.lists() {
            "where a folder has no index.html, or with ?list"
        } else {
            "off"
        },
    );
    style.row(
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
        style.row(
            "Warning",
            format!("there is no {INDEX_FILE} here, so the app will not load"),
        );
    }
    if args.lists() && reachable_from_elsewhere(args.host) {
        style.row(
            "Warning",
            "other devices can list the files here; --no-list turns that off",
        );
    }
    style.row("Open", style.link(&url, &authority));
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

/// Worth saying when the server is looking at the files rather than being told
/// about them: that notices later, and reads the whole directory each time.
fn live_reload(args: &Args) -> String {
    if !args.watches() {
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
    fn only_a_terminal_that_shows_codes_gets_colour_and_links() {
        let xterm = Some(OsStr::new("xterm-256color"));

        let terminal = Style::new(true, xterm, None);
        assert!(terminal.colour && terminal.links);

        let elsewhere = Style::new(false, xterm, None);
        assert!(!elsewhere.colour && !elsewhere.links);

        let dumb = Style::new(true, Some(OsStr::new("dumb")), None);
        assert!(!dumb.colour && !dumb.links);
    }

    #[test]
    fn a_terminal_naming_no_kind_is_trusted_only_on_windows() {
        let unnamed = Style::new(true, None, None);
        assert_eq!(unnamed.colour, cfg!(windows));
        assert_eq!(unnamed.links, cfg!(windows));
    }

    #[test]
    fn no_color_turns_colour_off_but_leaves_links_unless_it_is_empty() {
        let xterm = Some(OsStr::new("xterm-256color"));

        let set = Style::new(true, xterm, Some(OsStr::new("1")));
        assert!(!set.colour && set.links);

        let empty = Style::new(true, xterm, Some(OsStr::new("")));
        assert!(empty.colour && empty.links);
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
