# servio

An HTTP server for static files, with live reload, built with Axum. While you
work on a site, files are not cached and the browser refreshes when they
change. `--production` serves the finished site instead.

## Features

- Live reload, debounced by 200 ms
- A production mode for serving a finished site
- Gzip and Brotli compression
- Single-page app (SPA) fallback
- File lists for folders, with or without an `index.html`
- Optional long-term caching for versioned assets
- Safe defaults: localhost only, no caching, hidden files blocked
- Protection against serving files through symlinks that lead outside the chosen
  directory or to hidden files
- Security headers including `X-Content-Type-Options`, `X-Frame-Options`, and
  `Referrer-Policy`

## Install

Rust 1.88 or newer is required.

```bash
cargo install servio
```

To install the latest development version:

```bash
cargo install --git https://github.com/alexylon/servio
```

To install from a local clone:

```bash
git clone https://github.com/alexylon/servio
cd servio
cargo install --path .
```

## Usage

### Working on a site

Run `servio` in the directory you want to serve, then open the address shown
in the terminal. Saving a file refreshes the browser.

```bash
# Serve the current directory at http://localhost:3030
servio

# Serve another directory on port 8080, and open it in the browser
servio --dir site_public --port 8080 --open
```

### Testing on other devices

```bash
servio --dir site_public --host 0.0.0.0 --no-list
```

Phones and other computers on the same network open this machine's address,
such as `http://192.168.1.20:3030`, and still refresh when a file changes.
`--no-list` keeps the file lists from them.

### Serving a finished site

```bash
servio --dir site_public --port 3030 --production
```

`--production` turns off live reload and file lists, and lets browsers keep
files as long as they check for changes first. Give the port a proxy or service
expects: without `--port`, servio picks the next free port when 3030 is busy,
and the proxy keeps sending requests to 3030. The default host, `127.0.0.1`,
suits a proxy on the same machine.

### Single-page apps

```bash
# While working on the app
servio --dir dist --spa

# The finished app, with versioned file names under /assets/
servio --dir dist --port 3030 --production --spa --cache-assets
```

`--spa` answers a route such as `/users/123` with `index.html`.
`--cache-assets` is for a build that gives files under `/assets/` a new name
whenever they change; see [Caching](#caching).

## Options

| Option | Default | Description |
| --- | --- | --- |
| `-d, --dir <DIR>` | `.` | Directory to serve |
| `-p, --port <PORT>` | `3030` | Port to use |
| `--host <HOST>` | `127.0.0.1` | Address to listen on |
| `--spa` | off | Serve `index.html` when a page route matches no file |
| `--production` | off | Serve a finished site: no live reload, no file lists, and browsers check kept files for changes |
| `--no-list` | off | Do not show a folder's files, neither where it has no `index.html` nor with `?list` |
| `--no-reload` | off | Disable file watching and browser refreshes |
| `--poll` | off | Find changes by looking at the files, once a second |
| `--cache-assets` | off | Let browsers keep files under `/assets/` for a year; only safe when a file there gets a new name whenever it changes |
| `--open` | off | Open the address in the browser |
| `--ignore <PATTERN>` | none | Do not refresh the browser for changes matching this pattern; may be given more than once |

If you do not specify a port and 3030 is busy, servio tries the next available
port through 3039 and prints the selected address. If you specify a port,
servio uses that exact port or exits with an error.

`--open` opens the address in your usual browser, or in the one `BROWSER`
names. That variable may be a whole command, with `%s` where the address goes:

```bash
BROWSER="firefox --new-window %s" servio --open
```

It may also name several, tried in turn until one opens the address, parted by
`:` (`;` on Windows); one that stops at once with an error is passed over for
the next. Quote anything with a space in it, a path or a flag's value:

```bash
BROWSER="firefox:'/Applications/Firefox.app/Contents/MacOS/firefox' %s" servio --open
```

A quote counts anywhere in a word, as it does in a shell, so a path with an
apostrophe in it has to be quoted whole.

If no browser can be opened, or the one named stops at once with an error,
servio says so and keeps serving. With `--poll`, the browser opens once the
first look at the files is done.

## Live reload

Saving a file refreshes the browser. servio ignores changes in hidden
directories, `target`, `node_modules`, and common editor temporary files.

`--ignore` adds patterns of your own, for a build that writes logs or a cache
next to the pages:

```bash
servio --ignore "*.log" --ignore cache
```

A pattern is matched against the path below the served directory, written with
`/`, and against every directory above it, so `cache` covers everything in
`cache/`. A pattern with no `/` in it matches a name at any depth, as it does in
`.gitignore`; one with a `/` starts from the served directory, so `build/*.log`
is exactly one directory deep, and `/cache` is only the `cache` at the top. `*`
stops at a `/`; `**` crosses any number of directories, and `tmp/**` covers
`tmp` itself as well. On macOS and Windows, where the disk does not tell
`Build.LOG` from `build.log`, neither does a pattern. The files are still
served, and `--poll` still reads them: the pattern only decides what refreshes
the browser.

Naming files does not name the folder holding them. With `*.log`, a build that
creates `out/` afresh each run refreshes the browser once, for the new folder;
`out/**` covers the folder as well.

Patterns used on every run belong in a file called `.servioignore` in the
served directory, one to a line, with `#` starting a comment:

```
# what the build writes next to the pages
*.log
cache
```

The file is read once, when the server starts. Being hidden, it is never
served, and editing it does not refresh the browser. Its patterns and those
from `--ignore` apply together, and the banner says how many it added.

Live reload continues working when a build replaces the served directory, or
moves it aside and back. On
macOS, changing file permissions may also trigger one refresh because of how
the operating system reports file events.

### When saving a file changes nothing

Live reload normally waits for the operating system to report a change. Some
filesystems never report one: network shares, folders shared with a virtual
machine, and directories mounted into a container. There, `--poll` finds
changes by looking at the files itself, once a second:

```bash
servio --dir /mnt/share --poll
```

Every look reads every file in the served directory and compares its contents,
because a poll compares write times only to the whole second, and two saves
within one second of each other would otherwise look like one. So
`--poll` notices a change up to a second later than the ordinary watcher, and
costs far more while it waits. Use it only where the ordinary watcher stays
silent, and point it at the build output rather than the whole project tree, or
every look will read `node_modules` as well.

On Windows there is one more reason to poll. A build that changes thousands of
files in the same instant can overflow the room the system keeps for reporting
them. The system's watcher then stops without a word, and nothing refreshes
until servio is started again. Where builds are that large, `--poll` cannot
lose the watch.

Links are not followed. The server refuses to hand out anything outside the
served directory, so a link leading out cannot change what the browser sees,
and one leading back in points at files each look reads anyway.

Neither `--poll` nor `--ignore` can be combined with `--no-reload` or
`--production`, which turn off watching altogether.

## Single-page app fallback

With `--spa`, missing page routes fall back to `index.html` so the client-side
router can handle them. Missing scripts, stylesheets, images, and anything
under `/assets/` still return 404, making broken asset paths easy to spot.

If `index.html` disappears while the server is running, such as during a build
that clears the output directory, servio says so in the terminal. It says it
once each time the page goes missing rather than once per route, so a broken
build does not leave every address returning 404 without explanation.

Without `--spa`, an address with nothing behind it returns 404.

## File lists

Opening a folder that has no `index.html` shows the files in it. A folder that
has one shows its page; add `?list` to its address to see its files instead:

| Address | Shows |
| --- | --- |
| `http://localhost:3030/` | the page, `index.html` |
| `http://localhost:3030/?list` | the files in the served directory |
| `http://localhost:3030/docs/?list` | the files in `docs` |

Folders come first, then files, each with its size and the time it was last
written. A file's link opens the file, `index.html` included. On a list asked
for with `?list`, a folder's link keeps asking, so a walk down and back up
stays on the lists. Hidden files, and links leading out of the served directory
or to anything hidden, are left off, since none of them is ever served. With
live reload on, the list refreshes when the files change.

With `--spa`, a browser opening a folder with no `index.html` gets the app, as
it does at any other address with nothing behind it; `?list` still shows the
folder's files.

A list shows every name in a folder to anyone who can reach the server.
`--no-list` turns lists off, and so does `--production`: a folder with no
`index.html` answers 404 again, and `?list` changes nothing. The files are
still served to anyone who asks for them by name. Turn lists off whenever other
devices can reach the server, a proxy in front of it included. The banner warns
when servio listens beyond this machine with lists on, but it cannot tell when
a proxy passes requests along.

## Caching

What browsers may keep depends on two flags:

| Flags | Files under `/assets/` | Everything else |
| --- | --- | --- |
| neither | nothing | nothing |
| `--production` | kept, checked before each use | kept, checked before each use |
| `--cache-assets`, with or without `--production` | kept for a year without a check | kept, checked before each use |

With neither flag, servio sends `Cache-Control: no-store` and never answers
that a file is unchanged, so an edit shows at once. Checked before each use is
`no-cache`: the browser asks whether its copy changed, and a file that has not
changed is not sent again. A year is `public, max-age=31536000, immutable`.

Use `--cache-assets` only when the build gives a file under `/assets/` a new
name whenever its contents change, such as `app-3f9a1c.js`. servio does not
check the names, so a file that changes but keeps its name stays stale for a
year in browsers that already have it. A missing file, and a folder's own page
under `/assets/`, are still checked before each use, so a deploy caught halfway
is not remembered.

## Serving on a network

When exposing servio to a network, serve only the intended build directory.
Hidden paths are blocked apart from `.well-known`, which certificate renewal
needs, and symlinks may point only within the served directory. Each
subdirectory also uses a file watch while live reload is on, so serving a large
project tree can exhaust the operating system's watch limit.

## Development

Run the test suite with:

```bash
cargo test
```

See [CHANGELOG.md](CHANGELOG.md) for version history and
[RELEASE.md](RELEASE.md) for the release process.

## License

MIT — see [LICENSE](LICENSE).
