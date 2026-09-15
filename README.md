# servio

servio serves a website directly from a folder on your computer. While you
work, it automatically refreshes open pages when files change and tells
browsers not to store copies of your files. Use `--production` to serve a
finished site.

Changes on the main branch become available on crates.io and in the downloads
with the next release. Until then, [CHANGELOG.md](CHANGELOG.md) lists them
under Unreleased. To try changes before they are released,
[install the development version with Cargo](#with-cargo).

## Features

- Automatic browser refresh, called live reload, with file changes grouped
  over 200 milliseconds to reduce repeated refreshes
- A production mode for serving finished sites
- Smaller downloads through Gzip and Brotli compression
- Support for single-page apps that handle navigation in the browser
- Browsable file lists, including for folders that have an `index.html`
- Optional reuse of files under `/assets/` for a year with `--cache-assets`;
  changed files must get new names, which servio does not check
- Access from your own computer only by default, with browsers told not to
  store served files and hidden files blocked except for `.well-known`
- Checks that block links on disk leading outside the served folder or to
  hidden files
- Browser security settings sent with each response, including
  `X-Content-Type-Options`, `X-Frame-Options`, and `Referrer-Policy`

## Install

### Download a ready-to-run program

Download the archive for your system from the
[releases page](https://github.com/alexylon/servio/releases). Each archive
contains just the servio program; you do not need Rust to run it.

The archive names below apply to releases after 0.6.2. For releases up to
0.6.2, the Linux names use `-gnu` in place of `-musl`.

| System | Archive |
| --- | --- |
| Linux, x86-64 | `servio-x86_64-unknown-linux-musl.tar.gz` |
| Linux, arm64 | `servio-aarch64-unknown-linux-musl.tar.gz` |
| macOS, Apple silicon | `servio-aarch64-apple-darwin.tar.gz` |
| macOS, Intel | `servio-x86_64-apple-darwin.tar.gz` |
| Windows, x86-64 | `servio-x86_64-pc-windows-msvc.zip` |

The Linux `-musl` programs include the C library they need, so they do not
depend on the one installed on your system. The older `-gnu` programs require
glibc, a system library: version 2.39 or newer for servio 0.6.0–0.6.2, and
version 2.34 or newer for earlier releases.

On Linux or macOS, choose your archive name and download it:

```bash
archive=servio-aarch64-apple-darwin.tar.gz   # Replace with the archive for your system
curl -fLO https://github.com/alexylon/servio/releases/latest/download/$archive
```

Releases after 0.6.2 also include `SHA256SUMS`, which lets you check that the
download matches the published archive. For those releases, run the following
commands and confirm that the check reports `OK` before continuing. Skip this
step for releases up to 0.6.2, which do not include the file.

```bash
curl -fLO https://github.com/alexylon/servio/releases/latest/download/SHA256SUMS
grep " $archive\$" SHA256SUMS | shasum -a 256 -c   # or sha256sum -c on Linux
```

Extract the program and install it in `/usr/local/bin`. This folder should be
on your `PATH`, the list of folders your terminal searches for commands:

```bash
tar xzf $archive
sudo mkdir -p /usr/local/bin && sudo mv servio /usr/local/bin/
servio --version
```

On Windows, download `servio-x86_64-pc-windows-msvc.zip`. If the release includes
`SHA256SUMS`, download that too and run
`Get-FileHash servio-x86_64-pc-windows-msvc.zip` in PowerShell. Compare the
result with the ZIP file's entry in `SHA256SUMS`; letter case does not matter.
Once they match, extract `servio.exe` into a folder on your `PATH`. For a
release without `SHA256SUMS`, extract the program directly.

On macOS, a browser download may be blocked because the program has not passed
Apple's automated check for downloaded software. To remove that download
restriction, run `xattr -d com.apple.quarantine servio` in the folder where
you extracted the program, before moving it to `/usr/local/bin`. Downloads
made with `curl` usually do not have this restriction.

### With Cargo

You need Rust 1.88 or newer. To install the latest published version, run:

```bash
cargo install --locked servio
```

`--locked` uses the exact versions of supporting libraries recorded in
servio's `Cargo.lock` file.

To install the latest development version:

```bash
cargo install --locked --git https://github.com/alexylon/servio
```

To install from a local clone:

```bash
git clone https://github.com/alexylon/servio
cd servio
cargo install --locked --path .
```

## Usage

### Working on a site

Run `servio` in the folder you want to serve, then open the address shown in
the terminal. Pages opened through servio refresh when you save changes.

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

On a phone or another computer on the same network, open the serving
computer's network address, such as `http://192.168.1.20:3030`. Pages still
refresh when files change. `--no-list` hides the folder contents from browsing;
individual files remain accessible by their addresses.

### Serving a finished site

```bash
servio --dir site_public --port 3030 --production
```

`--production` turns off live reload and file lists. Browsers may store files,
but must check with the server before using a saved copy. Adding
`--cache-assets` changes this rule for files under `/assets/`; see
[Caching](#caching).

If another server forwards requests to servio, choose the port that server
expects. Production mode uses the specified port, or 3030 if none is given,
and exits with an error if it cannot use that port. The default host,
`127.0.0.1`, accepts connections only from the same computer, which suits a
forwarding server running there.

### Single-page apps

```bash
# While working on the app
servio --dir dist --spa

# The finished app, with file names under /assets/ that change on each update
servio --dir dist --port 3030 --production --spa --cache-assets
```

With `--spa`, opening a page address such as `/users/123` loads `index.html`
when no file matches. The app then decides what to display for that address.
Use `--cache-assets` only if your build gives each file under `/assets/` a new
name whenever its contents change; see [Caching](#caching).

## Options

| Option | Default | Description |
| --- | --- | --- |
| `-d, --dir <DIR>` | `.` | Folder to serve |
| `-p, --port <PORT>` | `3030` | Port to use |
| `--host <HOST>` | `127.0.0.1` | Address to listen on |
| `--spa` | off | Serve `index.html` for page requests that match no file, except under `/assets/` |
| `--production` | off | Turn off live reload and file lists, keep a fixed port, and require browsers to check saved copies for changes unless `--cache-assets` applies |
| `--no-list` | off | Disable all folder file lists, including those requested with `?list` |
| `--no-reload` | off | Stop watching for file changes and disable automatic browser refreshes |
| `--poll` | off | Check file contents for changes at a one-second interval |
| `--cache-assets` | off | Allow browsers to reuse files under `/assets/` for a year without checking; each file must get a new name when its contents change |
| `--open` | off | Open the server's address in a browser |
| `--ignore <PATTERN>` | none | Ignore matching changes when deciding whether to refresh; repeat the option to add more patterns |

### Choosing a port

During development, if you leave out `--port`, servio tries ports 3030 through
3039 and prints the address it selected. If you specify a port, servio uses
that port or exits with an error. With `--production`, it uses 3030 when no
port is specified and does not try alternatives.

### Opening a browser

`--open` opens the server's address in your default browser. Set `BROWSER` to
choose a different browser or supply a command. Use `%s` to mark where the
address should go; if you omit it, servio adds the address at the end:

```bash
BROWSER="firefox --new-window %s" servio --open
```

You can list several browser commands, separated by `:` on Linux and macOS
or `;` on Windows. servio tries them in order. If a browser cannot start or
exits immediately with an error, it tries the next one. Put paths and option
values containing spaces in quotes:

```bash
BROWSER="firefox:'/Applications/Firefox.app/Contents/MacOS/firefox' %s" servio --open
```

Quote marks are recognised anywhere in a command word. If a path contains an
apostrophe, enclose the whole path in double quotes inside `BROWSER`.

If no browser starts, or the selected browser exits soon after starting with
an error, servio reports the problem and keeps serving. With `--poll`, it
opens the browser after the first file scan finishes.

### Terminal output

In a supported terminal, the startup message uses colours and a clickable
address. Output saved to a file or passed to another program is plain text.
The same applies when `TERM` is `dumb`, as in some Emacs shells, or when
`TERM` is unset on a system other than Windows.

Set `NO_COLOR` to any non-empty value to turn off colours. Clickable addresses
remain available in terminals that support them.

## Live reload

Live reload refreshes pages opened through servio when files change. By
default, changes in hidden files and folders, `target`, `node_modules`, and
common editor temporary files do not cause a refresh. `.well-known` is an
exception: servio does not treat this folder as hidden.

### Ignoring changes

Use `--ignore` to prevent changes to selected files or folders from refreshing
the browser. For example, a build may write logs or temporary data alongside
your pages:

```bash
servio --ignore "*.log" --ignore cache
```

Write paths relative to the served folder, using `/` between folder names.
Patterns also apply to each parent folder within that path, so ignoring a
folder ignores changes anywhere inside it.

| Pattern | Changes it ignores |
| --- | --- |
| `*.log` | Files or folders whose names end in `.log`, at any depth |
| `cache` | Any file or folder named `cache`, at any depth |
| `/cache` | Only the file or folder named `cache` directly inside the served folder |
| `build/*.log` | Names ending in `.log` directly inside the top-level `build` folder |
| `tmp/**` | The top-level `tmp` folder and everything inside it |

A pattern without `/` matches a name at any depth, as in `.gitignore`. A path
with `/` starts at the served folder. `*` does not cross a `/`; `**` can match
across any number of folders. On macOS and Windows, pattern matching ignores
letter case, so `Build.LOG` and `build.log` match the same patterns.

Ignoring a file does not ignore its parent folder. For example, with `*.log`,
creating a new `out/` folder still causes a refresh even if it contains only
log files. Use `out/**` to ignore the folder and everything inside it.

These patterns affect automatic refreshes only. Matching files remain
available by their addresses, and `--poll` still reads them.

### Saving ignore patterns

To reuse patterns on every run, create `.servioignore` in the served folder.
Put one pattern on each line. Blank lines and lines beginning with `#` are
ignored:

```
# Files created by the build alongside the pages
*.log
cache
```

servio reads `.servioignore` once at startup when live reload is enabled.
Restart the server after changing its patterns. The file itself is hidden,
so it is not served and editing it does not refresh the browser. Its patterns
apply alongside those from `--ignore`; the startup message shows how many
patterns were loaded from the file.

Live reload also recovers when a build replaces the served folder or moves it
away and back. On macOS, changing file permissions may cause an extra refresh
because of how the operating system reports changes.

### When saving a file does not refresh the browser

Live reload normally relies on change notifications from the operating system.
Some folders do not provide reliable notifications, including network shares
and folders shared with a virtual machine or container. Use `--poll` to check
the files directly at a one-second interval:

```bash
servio --dir /mnt/share --poll
```

Each scan reads file contents throughout the served folder. The comparison
includes contents because the polling library records modification times only
in whole seconds and could otherwise miss a second save within that second.

Polling can take longer to notice changes and uses more processing and disk
access, even while you are not editing. Large folders or slow storage can make
each scan take longer than the one-second interval. Use it when ordinary live
reload is unreliable, and serve the folder containing your built site to keep
scans small. If you serve the whole project, each scan also reads folders such
as `node_modules`, even though their changes do not cause refreshes.

On Windows, a build that changes thousands of files at once can overwhelm the
system's change notifications. The watcher can then stop reporting changes
without an error, leaving live reload inactive until servio restarts.
`--poll` avoids relying on those notifications.

Polling does not follow links on disk. Links outside the served folder cannot
supply files to the browser, and links pointing back inside refer to files
already covered by the scan.

You cannot combine `--poll` or `--ignore` with `--no-reload` or `--production`,
because those options turn off file watching.

## Page handling for single-page apps

A single-page app handles navigation in the browser. With `--spa`, a request
for an HTML page that would otherwise return 404 (not found) receives the
served folder's `index.html`. The app can then display the right page for the
requested address.

The request must accept HTML for this to happen. Normal browser requests for
missing scripts, stylesheets, or images still return 404. Anything missing
under `/assets/` also returns 404, even when requested as a page. servio uses
the request's accepted content type to make this choice, rather than the file
extension. Hidden paths and links to files the server blocks remain blocked.

If `index.html` is missing at startup, servio warns in the terminal. If it
disappears later, for example while a build clears the site folder, servio
reports the problem when a request needs that page. It reports each observed
period of absence once, so repeated requests do not flood the terminal.

Without `--spa`, an address with no matching file or folder returns 404.

## File lists

By default, opening a folder with no `index.html` shows a list of its files.
If the folder has an `index.html`, that page opens instead. Add `?list` to a
folder's address to request its file list:

| Address | Shows |
| --- | --- |
| `http://localhost:3030/` | `index.html`, if present; otherwise the file list |
| `http://localhost:3030/?list` | Files in the served folder |
| `http://localhost:3030/docs/?list` | Files in `docs` |

Folders appear first, followed by files, with each group sorted by name.
Files show their size and last modification time; folders show their last
modification time. File links open the file itself, including `index.html`.
When you request a list with `?list`, links to child and parent folders keep
`?list`, so you can continue browsing their contents.

Hidden files and links to blocked locations are omitted. `.well-known`
remains visible. With live reload enabled, file lists refresh when files
change.

With `--spa`, a browser opening a folder without its own `index.html` receives
the app's page if it is available and the address is outside `/assets/`.
While file lists are enabled, `?list` still requests the folder's contents.

### Turning off file lists

Anyone who can reach the server can browse enabled file lists. Use `--no-list`
to turn them off; `--production` also turns them off. In either case, `?list`
no longer requests a list. A folder without `index.html` returns 404 unless
`--spa` can serve the app's page. Individual files remain accessible to anyone
who knows their addresses.

Turn off file lists whenever other devices can reach the server, including
through a server that forwards requests. The startup message warns when
servio accepts connections from other devices with file lists enabled. It
cannot detect another server forwarding requests to a local address.

## Caching

Caching means storing downloaded files so they can be reused. servio controls
this with two options:

| Options | Files under `/assets/` | Everything else |
| --- | --- | --- |
| Neither | Do not store | Do not store |
| `--production` | Store, but check before reuse | Store, but check before reuse |
| `--cache-assets`, with or without `--production` | Reuse for a year without checking | Store, but check before reuse |

The server sends these instructions in `Cache-Control`:

- `no-store`: the browser must not store the response. servio also ignores
  requests to check a saved copy and sends the file again, so an old copy does
  not hide your edits.
- `no-cache`: the browser may store the response, but must ask the server
  whether it has changed before reusing it. If the saved copy still matches,
  servio confirms this without sending the file again.
- `public, max-age=31536000, immutable`: the browser, or another service
  storing responses, may reuse the file for one year without checking for
  changes.

### Checking saved copies

With either caching option enabled, servio uses a version marker sent in the
`ETag` response header to check saved copies. It combines the file's size and
last modification time with the system's identifier for that file, when
available. Times include fractions of a second where the storage system
records them. Browsers send the marker back to check their saved copy.
Restoring a file with an older modification time is detected as a change too.

Restarting the same servio build preserves markers while the file details
above remain unchanged. Rebuilding servio with a different Rust version can
change the markers. File identifiers can also change when a folder is mounted
again, for example in a new container, even if its contents stay the same.

Two servers may therefore assign different markers to identical site files.
If a browser checks a saved copy against a server with a different marker,
the server sends the file again.

These checks rely on those file details and can miss a change when:

- A file is rewritten in place with the same size and modification time,
  such as when a build assigns a fixed time to every file.
- The storage system records only whole seconds and a file is rewritten in
  place with the same size within that second.
- The system cannot identify individual files and a replacement has the same
  size and modification time as the old file.

If a request checks only the modification date, without a version marker,
the comparison is limited to whole seconds.

### Keeping files for a year

Use `--cache-assets` only if your build gives each file under `/assets/` a
new name whenever its contents change, such as `app-3f9a1c.js`. servio does not
check filenames for this. If a file changes but keeps its name, browsers that
already have it may keep using the old version for a year.

Missing files and folder addresses such as `/assets/docs/` still require a
check before reuse. This also applies to file lists under `/assets/`. It
prevents a temporary missing-file response or a folder's page from being
reused for a year. A direct file address such as `/assets/docs/index.html`
does receive the year-long setting, so its name must change when its contents
do.

## Serving on a network

When other devices can reach servio, serve only the folder containing the
site you intend to share. Files and folders whose names begin with `.` are
blocked, except for `.well-known`, which is used for tasks such as certificate
renewal. Hidden names inside `.well-known` are still blocked. Links on disk
must lead to allowed files or folders within the served folder.

With live reload enabled, watching a large project can use substantial system
resources. On Linux, each folder needs a separate watch, so a large folder
tree can reach the operating system's watch limit. Serve your site's output
folder to reduce this load.

## Development

servio is written in Rust and built with Axum. Run the test suite with:

```bash
cargo test
```

See [CHANGELOG.md](CHANGELOG.md) for version history and
[RELEASE.md](RELEASE.md) for the release process.

## License

MIT — see [LICENSE](LICENSE).
