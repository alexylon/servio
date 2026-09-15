mod banner;
mod browser;
mod errors;
mod guard;
mod ignore;
mod list;
mod listen;
mod serve;
mod version;
mod watch;

use crate::errors::cannot_reach;
use crate::listen::DEFAULT_PORT;
use crate::serve::Caching;
use anyhow::{Context, Result, bail};
use clap::Parser;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tower_livereload::LiveReloadLayer;

/// The flags that turn watching off, which every flag about watching conflicts
/// with.
const TURN_WATCHING_OFF: [&str; 2] = ["no_reload", "production"];

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Port to listen on [default: 3030, or the next free one, except with
    /// --production]
    #[arg(short, long)]
    port: Option<u16>,

    /// Address to listen on (use 0.0.0.0 to reach this server from other devices)
    #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    host: IpAddr,

    /// Directory to serve
    #[arg(short, long, default_value = ".")]
    dir: PathBuf,

    /// Serve index.html when the address matches no file, for single-page apps
    #[arg(long)]
    spa: bool,

    /// Serve a finished site: no live reload, no file lists, no moving to another
    /// port, and browsers check kept files for changes, apart from those
    /// --cache-assets keeps
    #[arg(long)]
    production: bool,

    /// Do not show file lists, not even with ?list
    #[arg(long)]
    no_list: bool,

    /// Do not watch for changes, and do not refresh the browser
    #[arg(long)]
    no_reload: bool,

    /// Find changes by looking at the files, for a network or shared folder
    /// the system reports no changes in
    #[arg(long, conflicts_with_all = TURN_WATCHING_OFF)]
    poll: bool,

    /// Let browsers keep files under /assets/ for a year; only safe when a file
    /// there gets a new name whenever it changes
    #[arg(long)]
    cache_assets: bool,

    /// Open the address in the browser once the server is up
    #[arg(long)]
    open: bool,

    /// Do not refresh the browser for a change matching this pattern, such as
    /// "*.log" or "cache"; may be given more than once, or kept one to a line
    /// in a .servioignore file in the served directory
    #[arg(long, value_name = "PATTERN", conflicts_with_all = TURN_WATCHING_OFF)]
    ignore: Vec<String>,
}

impl Args {
    fn watches(&self) -> bool {
        !self.no_reload && !self.production
    }

    fn lists(&self) -> bool {
        !self.no_list && !self.production
    }

    /// The port to take, or none to step up from 3030 to a free one. A finished
    /// site stays put, since whatever sends requests to it expects its port.
    fn exact_port(&self) -> Option<u16> {
        self.port.or(self.production.then_some(DEFAULT_PORT))
    }

    /// `--cache-assets` keeps files under /assets/ for a year, with or without
    /// `--production`.
    fn caching(&self) -> Caching {
        if self.cache_assets {
            Caching::AssetsForAYear
        } else if self.production {
            Caching::Checked
        } else {
            Caching::Off
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            errors::report(&error);
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let args = Args::parse();
    let static_dir = resolve_dir(&args.dir)?;

    // A missing path already failed in resolve_dir; this catches a file.
    if !static_dir.is_dir() {
        bail!(
            "cannot serve {}: it is not a directory",
            static_dir.display()
        );
    }

    let livereload = LiveReloadLayer::new();
    let reloader = livereload.reloader();

    // Read before the port is bound: a bad pattern is a reason to stop, not
    // to serve.
    let mut from_ignore_file = 0;
    let mut ignored = None;
    if args.watches() {
        let file = ignore::read_file(&static_dir)?;
        from_ignore_file = file.len();
        ignored = Some(ignore::Ignored::from(&args.ignore, &file)?);
    }

    // One look, shared with the banner: two looks could disagree, and a page
    // that went between them would be announced twice.
    let no_app_page = args.spa && !static_dir.join(serve::INDEX_FILE).is_file();

    let app = serve::app(
        &static_dir,
        args.spa,
        args.lists(),
        args.caching(),
        args.watches().then_some(livereload),
        no_app_page,
    );

    let listener = listen::listen(args.host, args.exact_port()).await?;

    // With `--port 0` the system picks the port, so ask the listener.
    let bound = listener
        .local_addr()
        .context("cannot tell which address the server is listening on")?;
    // While polling, the address goes up first: the first look reads every
    // file, which on the folders `--poll` is for takes a while. The port is
    // bound by now, so a browser arriving meanwhile waits rather than being
    // refused.
    if args.poll {
        banner::print(bound, &args, &static_dir, no_app_page, from_ignore_file);
    }

    if let Some(ignored) = ignored {
        watch::start(&static_dir, args.poll, ignored, reloader)?;
    }

    // Otherwise the watch goes on first, so that a watch which fails does not
    // follow a banner saying live reload is on.
    if !args.poll {
        banner::print(bound, &args, &static_dir, no_app_page, from_ignore_file);
    }

    if args.open {
        browser::open(&banner::url(bound));
    }

    axum::serve(listener, app)
        .await
        .context("the server stopped")?;

    Ok(())
}

fn resolve_dir(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_relative() {
        std::env::current_dir()
            .map_err(|error| cannot_reach("cannot read the current directory".to_string(), &error))?
            .join(path)
    } else {
        path.to_path_buf()
    };

    // Name the path, so a typo shows.
    absolute
        .canonicalize()
        .map_err(|error| cannot_reach(format!("cannot serve {}", absolute.display()), &error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finished_site_stays_on_its_port() {
        let port = |line: &[&str]| {
            Args::try_parse_from(line.iter().copied())
                .expect("the arguments should parse")
                .exact_port()
        };

        assert_eq!(port(&["servio"]), None);
        assert_eq!(port(&["servio", "--port", "8080"]), Some(8080));
        assert_eq!(port(&["servio", "--production"]), Some(DEFAULT_PORT));
        assert_eq!(
            port(&["servio", "--production", "--port", "8080"]),
            Some(8080)
        );
    }
}
