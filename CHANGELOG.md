# Changelog

## [Unreleased]

### Added
- A folder with no `index.html` shows the files in it, and `?list` added to any
  folder's address shows them where it has one. Folders come first, a folder's
  link on a list asked for keeps `?list`, and hidden files, and links leading
  out of the served directory or to anything hidden, are left off, since none
  of them is ever served
- `--no-list` turns the lists off, for a site other devices can reach. The
  banner warns when other devices can reach the server and the lists are on
- `--production` serves a finished site: no live reload and no file lists, and
  browsers may keep files as long as they check for changes before using them.
  With `--cache-assets` as well, files under `/assets/` are still kept for a
  year
- Each GitHub release comes with `SHA256SUMS`, to check the downloaded
  archives against, and the README explains how to install them

### Fixed
- The banner wrote the codes for its colours and its link into files and
  pipes, where they are noise. It now writes them only to a terminal, and
  leaves the colours out when `NO_COLOR` is set
- With `--cache-assets`, a page at a folder's own address under `/assets/`,
  such as `/assets/docs/`, was kept for a year, though a folder's name carries
  no hash

### Changed
- A folder with no `index.html` answers with the list of its files rather than
  404, unless `--no-list` is given. With `--spa`, a browser opening one still
  gets the app

### Security
- An address starting with `//` that named a served folder, such as
  `//example.com` with a folder of that name, was redirected to
  `//example.com/`, which a browser follows to that site. Slashes in front of
  an address, and backslashes among them, now count as one
- A folder's address answered with its `index.html` even when that was a link
  leading out of the served directory, though the same file asked for by name
  was refused; with `--spa`, so did every route of the app. Both are refused
  now
- A link inside the served directory to a hidden file or folder, such as
  `docs` leading to `.git`, handed out what it led to, since only the address
  was checked for hidden names. Where a link leads is now checked as well

## [0.6.2] - 2026-09-04

### Fixed
- A build that writes for longer than a moment refreshed the browser once for
  every few hundredths of a second it went on. The browser is now refreshed
  once the files have been quiet for a moment, one refresh for one build. A
  file written without pause, a log say, puts a refresh off by a second at
  most
- With `--poll`, a directory replaced and taken away again within one look
  was announced as replaced while there was nothing there, and its return
  passed for nothing new
- Running out of file watches while a build creates directories tore the
  watch down and set it up again, which ran out partway and left directories
  watched before unwatched. The watch that is on now stays, and one line
  names the directory that could not be watched
- A change made while a script had moved the served directory aside was
  never refreshed once the directory was back. The check now says "Directory
  back" and refreshes
- Running out of open files, which on Linux the file watchers of every editor
  and dev server count against, was reported as a numbered error instead of
  being explained, whether the port or the watcher was what ran out
- On macOS and Windows, every time the watch went on the whole served
  directory was walked, following links out of it, for a record that was never
  needed. It no longer is
- When the system dropped file events, as it can under a build that writes
  more files than it can report, the notice saying so was ignored and the
  browser was not refreshed for what was missed. It now refreshes
- A rebuild that brought a directory this program may not read left the watch
  off without a word, while the banner still said live reload was on. It now
  says which directory and why, tries again every second, and announces the
  rebuild once the watch can go on
- Refusing to start over a directory it may not read now names that directory
  rather than the served one, and says that `--poll` watches what it may and
  `--no-reload` serves without watching
- On macOS, a run announced a change and refreshed the browser once shortly
  after starting, for files that were written just before it began watching.
  For the first moments of a run, a file that has not been written since is no
  longer called a change

## [0.6.1] - 2026-09-04

### Fixed
- A rebuild was announced twice, and the browser refreshed to an error page on
  the way, when the watcher's account of the old directory going reached it
  before the check had seen the new one. What the watcher reports about a
  directory no longer at the name is now left to the check
- Without `--poll`, a directory taken away refreshed the browser to an error
  page. The page on screen is now kept, as it is with `--poll`, until the
  directory is back

## [0.6.0] - 2026-09-04

### Added
- `--poll` finds changes by looking at the files once a second, reading each of
  them, for network shares, folders shared with a virtual machine, and
  directories mounted into a container, where the system reports no changes at
  all. The banner says when a run is looking rather than being told
- `--open` opens the address in the browser once the server is up: the usual
  one, or whatever `BROWSER` names. That may be a whole command, with `%s` where
  the address goes, and it may name several to try in turn
- `--ignore` names files whose changes should not refresh the browser, as a
  pattern matched below the served directory: `*.log`, `cache`, `build/*.log`.
  May be given more than once, and a `.servioignore` file in the served
  directory holds the ones used on every run. The banner lists what is being
  ignored

## [0.5.3] - 2026-09-02

### Added
- With `--spa`, the console says so each time `index.html` goes missing while
  the server runs, rather than leaving every address to answer 404 in silence

### Fixed
- A served directory this program was not allowed to open was announced as
  replaced on every check, so the browser reloaded ten times a second
- A path leading below a file, such as `--dir dist/index.html/js`, was reported
  as a numbered error instead of being explained
- An address this machine does not answer to, such as a typo in `--host`, was
  reported as a numbered error instead of being explained

### Changed
- A failure now names what could not be done before it says why, as one
  sentence: `cannot serve /site: there is no such directory`

## [0.5.2] - 2026-09-02

### Fixed
- With `--spa` and `--cache-assets` together, opening an address under
  `/assets/` in a browser was answered with the app page and kept for a year,
  so the real file was never asked for again
- A hashed file the browser already had was told to check on every visit from
  then on, undoing what `--cache-assets` had asked for
- A port the system will not give out, such as one below 1024, was reported as
  a numbered error instead of being explained
- A rebuild was announced twice, once by the watcher and once by the check that
  notices the new directory
- A burst of watcher failures set the watch up again once for each of them
- A directory that could not be reached, read, or watched was reported as a
  numbered error, the way a busy port used to be. Running out of file watches
  now says so, and says what to do about it

### Changed
- With `--spa`, an address under `/assets/` no longer falls back to the app
  page: those names carry a hash of a file's contents, so one that is missing
  is missing, and a typo in a build stays visible

### Security
- A symbolic link leading out of the served directory is refused, so a link
  left in a build cannot hand out the rest of the disk

## [0.5.1] - 2026-09-02

### Added
- The next free port is used when 3030 is busy and no port was asked for; the
  banner says which one

### Fixed
- A file missing under `/assets/` was remembered as missing for a year with
  `--cache-assets`, so a visitor caught mid-deploy could never load it again
- A refused hidden file was answered without the cache header every other
  answer carries
- The directory was watched before it was looked at, so a build landing in
  between could leave live reload silently dead
- A port the system keeps for itself, as Windows does with its reserved
  ranges, now steps to the next one instead of stopping

### Changed
- One directory is told from another by the `file-id` crate, which notify
  already builds, leaving no unsafe code in this one

## [0.5.0] - 2026-09-02

### Added
- `--no-reload` and `--cache-assets`, for serving a site that is published
  rather than being worked on

### Changed
- Called an HTTP server for static files, which says both halves of what it is

## [0.4.0] - 2026-09-02

### Added
- The tests and the build run on Linux, macOS and Windows on every push
- Everything crates.io asks for, and a linked release build

### Changed
- Renamed from `serve` to `servio`, since `serve` was taken

### Fixed
- A build that replaces the served directory is noticed on macOS and Windows,
  not only on Linux

## [0.3.0] - 2026-08-31

### Added
- A test suite that starts the real binary and talks HTTP to it
- MIT licence

### Changed
- Nothing is cached: `no-store` everywhere, in place of a year on `/assets/`
- Reachable only from this machine unless `--host` says otherwise
- Serving `index.html` for an address that matches no file is now `--spa`

### Fixed
- Serving a page no longer counts as a change, so pages stop reloading forever
- Live reload survives a build that deletes or renames the served directory
- Hidden files are refused however the address is written
