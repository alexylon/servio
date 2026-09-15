//! Watching the served directory, and telling the browser when it changed.

use crate::ignore::Ignored;
use crate::refresh::{refresh_once_quiet, wait_for_change};
use anyhow::Result;
use file_id::FileId;
use notify_debouncer_full::notify::ErrorKind as WatchError;
use notify_debouncer_full::notify::event::{AccessKind, AccessMode, MetadataKind};
use notify_debouncer_full::notify::{
    self, EventKind, PollWatcher, RecommendedWatcher, RecursiveMode, event::ModifyKind,
};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, NoCache, new_debouncer_opt,
};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tower_livereload::Reloader;

/// Long enough to group the writes one save makes, short enough that the
/// refresh still feels immediate.
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);

/// How often `--poll` looks at the files. A change waits up to this long to be
/// noticed, and every look reads every file, on the very folders where reading
/// is slow.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How often to check that the served directory is still the one being
/// watched, and how long to wait between attempts to watch it again.
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// How long to wait before trying to watch again after the system refused.
/// Every try walks the whole directory, so not too often.
const TRIES_AGAIN_AFTER: Duration = Duration::from_secs(1);

/// How many unreadable paths are worth naming. Past that it goes quiet.
const MOST_PROBLEMS: usize = 20;

/// The watcher, with the debouncing that groups the writes of one save.
type Debounced<W> = Debouncer<W, NoCache>;

/// Why the watcher has to be set up again. Both mean changes were missed;
/// they differ only in what to call it.
enum Rewatch {
    /// The name now leads to a different directory: a build deleted the old
    /// one, or renamed it away.
    Replaced,
    /// The watcher itself failed, dropping whatever it had not reported.
    WatchFailed,
}

/// Starts watching `root` and keeps it watched for as long as the program
/// runs. The watcher lives on the thread this spawns.
///
/// With `poll`, the files are looked at over and over rather than waited on.
/// Slower, and more work, but the only way to notice a change the system says
/// nothing about: a network share, a folder shared with a virtual machine, a
/// directory handed to a container.
pub(crate) fn start(root: &Path, poll: bool, ignored: Ignored, reloader: Reloader) -> Result<()> {
    let (failed, failures) = mpsc::channel();
    let (changed, changes) = mpsc::channel();
    std::thread::spawn(move || {
        refresh_once_quiet(
            |until| wait_for_change(&changes, until),
            |worth_a_line| {
                if worth_a_line {
                    println!("  File changed, reloading...");
                }
                reloader.reload();
            },
        );
    });

    let rebuilds = Rebuilds::new(poll);
    let report = report_changes(
        root.to_path_buf(),
        poll,
        ignored,
        rebuilds.clone(),
        changed.clone(),
        failed,
    );

    // No record of file numbers: it only tells a rename from a removal and a
    // creation, which refresh the browser either way, and filling it walks the
    // whole directory, links included, every time the watch goes on.
    if poll {
        let debouncer = new_debouncer_opt::<_, PollWatcher, _>(
            DEBOUNCE_DELAY,
            None,
            report,
            NoCache::new(),
            // The contents, not just the write time: a poll keeps that only to
            // the whole second, so two saves within one second of each other
            // would look like one and the second would never reach the browser.
            //
            // Links are not followed. The server hands out nothing outside the
            // served directory, so a link leading out cannot change what the
            // browser sees, and one leading back in points at files this look
            // reads anyway. Following them reads everything twice, and a link
            // to a directory above makes a walk with no end; `node_modules`
            // has those.
            notify::Config::default()
                .with_poll_interval(POLL_INTERVAL)
                .with_compare_contents(true)
                .with_follow_symlinks(false),
        )
        .map_err(|error| cannot_watch_here(root, &error))?;

        keep_watching(debouncer, root, failures, rebuilds, changed)
    } else {
        let debouncer = new_debouncer_opt::<_, RecommendedWatcher, _>(
            DEBOUNCE_DELAY,
            None,
            report,
            NoCache::new(),
            notify::Config::default(),
        )
        .map_err(|error| cannot_watch_here(root, &error))?;

        keep_watching(debouncer, root, failures, rebuilds, changed)
    }
}

/// What tells a rebuild apart from a change of its own, shared between the
/// watcher and the check.
#[derive(Clone)]
struct Rebuilds {
    /// When a rebuild was last announced. The watcher's own account of the
    /// same rebuild arrives a moment later, and this is how it is recognised
    /// as the echo it is. Where the watcher gets there first instead, what it
    /// says is dropped by comparing `watching` with the directory at the name.
    announced: Arc<Mutex<Option<Instant>>>,
    /// Which directory the watch is on, by the system's number for it. The
    /// watch follows the directory and not the name, so the watcher compares
    /// this with what the name leads to now before reporting anything.
    watching: Arc<Mutex<Option<FileId>>>,
    /// How long the watcher's account of a rebuild goes on arriving after the
    /// announcement.
    same_rebuild: Duration,
}

impl Rebuilds {
    fn new(poll: bool) -> Rebuilds {
        Rebuilds {
            announced: Arc::new(Mutex::new(None)),
            watching: Arc::new(Mutex::new(None)),
            same_rebuild: same_rebuild(poll),
        }
    }
}

/// How long after the watch goes on the files themselves are asked whether
/// anything happened. Long enough for a system that takes its time to have
/// caught up.
const SETTLING_IN: Duration = Duration::from_secs(3);

/// What the watcher calls when files changed, and when it could not read them.
fn report_changes(
    root: PathBuf,
    polling: bool,
    ignored: Ignored,
    rebuilds: Rebuilds,
    changed: mpsc::Sender<bool>,
    failed: mpsc::Sender<()>,
) -> impl FnMut(DebounceEventResult) + Send + 'static {
    // What has already been said about an unreadable path, so that a look does
    // not say it again.
    let mut told: Vec<String> = Vec::new();

    // When the watch goes on, by both clocks: one to compare write times
    // against, one to measure the first moments by.
    let watch_went_on = SystemTime::now();
    let watching_since = Instant::now();

    move |result| match result {
        Ok(events) => {
            // A rebuild is the check's to report, once, when the new
            // directory is there. Nothing is refreshed on the way: the page
            // cannot load from a directory that is not there, and the one on
            // screen is still the last that could.
            if replaced_under_the_watch(&rebuilds.watching, &root) {
                return;
            }

            // A poll has its own account of what the files were, so it never
            // reports one that was there all along.
            let settling_in = !polling && watching_since.elapsed() < SETTLING_IN;

            if events.iter().any(|event| {
                is_change(&ignored, &root, event, polling)
                    && !(settling_in && from_before_the_watch(event, watch_went_on))
            }) {
                // Whether this is worth a line is decided now, while a
                // rebuild just announced is still fresh. The refresh itself
                // waits for the files to go quiet.
                let _ = changed.send(!just_announced(&rebuilds.announced, rebuilds.same_rebuild));
            }
        }
        // One path a look could not read, not a watch that went down: the rest
        // of the tree was read and nothing was dropped, so there is nothing to
        // put right. Setting the watch up again would read that path again, and
        // refresh the browser every time round. A directory replaced underneath
        // is still noticed by the check.
        Err(errors) if polling => {
            for problem in errors
                .iter()
                .filter_map(|error| cannot_look(&ignored, &root, error))
            {
                // Every look meets the same path, and once is enough.
                if !told.contains(&problem) && told.len() < MOST_PROBLEMS {
                    eprintln!("  {problem}");
                    told.push(problem);
                }
            }
        }
        Err(errors) => {
            let (out_of_watches, down): (Vec<_>, Vec<_>) = errors
                .iter()
                .partition(|error| matches!(error.kind, WatchError::MaxFilesWatch));

            // A directory created just now could not be watched, and the rest
            // still is. Setting the watch up again would run out partway
            // through and leave less watched than now.
            for error in out_of_watches {
                let problem = format!(
                    "Cannot watch {}: {}. New directories are not watched from here on",
                    short_name(&root, error.paths.first().map_or(&root, PathBuf::as_path)),
                    cannot_watch(error)
                );
                if !told.contains(&problem) && told.len() < MOST_PROBLEMS {
                    eprintln!("  {problem}");
                    told.push(problem);
                }
            }

            // Otherwise the watcher dies quietly while the banner still says
            // reloads are on.
            if !down.is_empty() {
                for error in down {
                    eprintln!("  Cannot watch for changes: {}", cannot_watch(error));
                }
                let _ = failed.send(());
            }
        }
    }
}

/// Puts the watch on, and hands it to the thread that keeps it there.
fn keep_watching<W: notify::Watcher + Send + 'static>(
    mut debouncer: Debounced<W>,
    root: &Path,
    failures: Receiver<()>,
    rebuilds: Rebuilds,
    changed: mpsc::Sender<bool>,
) -> Result<()> {
    // Look first, then watch. The other way round, a directory replaced in
    // between would leave the watch on the old one and the number remembered
    // for the new one, and nothing would ever notice.
    let first_look = Watched::at(root);
    debouncer
        .watch(root, RecursiveMode::Recursive)
        .map_err(|error| cannot_watch_here(root, &error))?;

    // Looked at again afterwards, as `watch_again` does: a poll reports a
    // watch on a directory it could not read as success, and then hears
    // nothing ever after. When the two looks disagree, nothing is known to be
    // watched, and the check puts that right.
    let first_look = first_look.filter(|it| Some(it.id) == directory_id(root));
    remember(&rebuilds.watching, &first_look);

    let root = root.to_path_buf();
    std::thread::spawn(move || supervise(debouncer, root, failures, first_look, rebuilds, changed));

    Ok(())
}

/// Keeps the watch on the directory. The watch follows the directory, not
/// its name: a build that deletes and recreates it leaves the watch on
/// something nobody can reach. Only Linux reports that in the file events,
/// so look at the directory itself.
fn supervise<W: notify::Watcher>(
    mut debouncer: Debounced<W>,
    root: PathBuf,
    failures: Receiver<()>,
    mut watched: Option<Watched>,
    rebuilds: Rebuilds,
    changed: mpsc::Sender<bool>,
) {
    // Whether the last look found no directory at the name.
    let mut was_missing = false;

    loop {
        let reason = match failures.recv_timeout(CHECK_INTERVAL) {
            Ok(()) => Rewatch::WatchFailed,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Missing for the moment means a build is between removing
                // the directory and writing the new one.
                let Some(now) = directory_id(&root) else {
                    was_missing = true;
                    continue;
                };

                match watched.as_ref().map(|it| it.id) {
                    Some(before) if before != now => Rewatch::Replaced,
                    // Away for a moment and back as it was: a script that
                    // moves the directory aside to work on it. Whatever was
                    // written meanwhile was reported under a name that led
                    // nowhere, and left to this check.
                    Some(_) if was_missing => {
                        was_missing = false;
                        announce(&rebuilds.announced);
                        println!("  Directory back, reloading...");
                        let _ = changed.send(false);
                        continue;
                    }
                    Some(_) => continue,
                    // Nothing to compare against: the directory could not
                    // be reached when the watch went on, so the watch may be
                    // on one that has since been replaced. Put it back on the
                    // name, quietly: nothing is known to have changed.
                    None => {
                        watched = watch_again(&mut debouncer, &root, &failures, None);
                        remember(&rebuilds.watching, &watched);
                        continue;
                    }
                }
            }
        };

        // Marked before the rewatch, once the old watch is off, and after: a
        // look may be in progress when the old watch comes off, reading the
        // new directory takes as long as its files do, and the window is only
        // so wide.
        let rebuild = matches!(reason, Rewatch::Replaced).then_some(&*rebuilds.announced);
        if let Some(at) = rebuild {
            announce(at);
        }

        watched = watch_again(&mut debouncer, &root, &failures, rebuild);
        remember(&rebuilds.watching, &watched);
        was_missing = false;

        // Either way the page may be out of date: whatever was written while
        // there was no watch went unnoticed, and nothing else will announce
        // it.
        match reason {
            Rewatch::Replaced => {
                announce(&rebuilds.announced);
                println!("  Directory replaced, reloading...");
            }
            Rewatch::WatchFailed => println!("  Watching again, reloading..."),
        }
        let _ = changed.send(false);
    }
}

/// Puts the watch back on whatever the name leads to now, and returns what
/// that was. A build can take a while between removing the directory and
/// writing the new one, so this waits rather than gives up.
///
/// With `rebuild`, the rebuild is marked as announced again once the old watch
/// is off: a look that was in progress has ended by then, and what it found
/// arrives shortly after.
fn watch_again<W: notify::Watcher>(
    debouncer: &mut Debounced<W>,
    root: &Path,
    failures: &Receiver<()>,
    rebuild: Option<&Mutex<Option<Instant>>>,
) -> Option<Watched> {
    // What has been said about a watch the system refused, so it is said once.
    let mut told = None;

    loop {
        if !root.is_dir() {
            std::thread::sleep(CHECK_INTERVAL);
            continue;
        }

        // Let go of the old directory first: after a rename the watch is
        // still on it, reporting changes under its new name.
        let _ = debouncer.unwatch(root);
        if let Some(at) = rebuild {
            announce(at);
        }

        // Anything waiting now is about the watcher just taken off, and this
        // recovery answers all of it. Drained before the new watch exists, so
        // a failure of that one can ask for a recovery of its own.
        while failures.try_recv().is_ok() {}

        // Looked at before watching again, for the same reason as at the
        // start.
        let looked_at = Watched::at(root);
        let watched = debouncer.watch(root, RecursiveMode::Recursive);

        match watched {
            Ok(()) if same_directory(looked_at.as_ref(), directory_id(root)) => {
                return looked_at;
            }
            // Gone or replaced meanwhile: the next time round starts afresh.
            Ok(()) => {}
            // Windows wraps every failure the same way, so the directory is
            // asked as well.
            Err(error) if was_not_there(&error) || !root.is_dir() => {}
            // Most often a directory below that this program may not read.
            // Said once, since the watch is off until the system allows it
            // and the banner still says live reload is on.
            Err(error) => {
                let problem = format!(
                    "Cannot watch {}: {}. Nothing will refresh until the watch can go on",
                    short_name(root, error.paths.first().map_or(root, PathBuf::as_path)),
                    cannot_watch(&error)
                );
                if told.as_ref() != Some(&problem) {
                    eprintln!("  {problem}");
                    told = Some(problem);
                }
                std::thread::sleep(TRIES_AGAIN_AFTER);
                continue;
            }
        }

        std::thread::sleep(CHECK_INTERVAL);
    }
}

/// True when the directory looked at before the watch went on is still the
/// one there after it. A poll takes a watch on a name that leads nowhere
/// without complaint, and finds nothing ever after.
///
/// Nothing does not agree with nothing: a directory gone between the two looks
/// would otherwise be announced as replaced while there is nothing there, and
/// its return would pass for nothing new.
fn same_directory(before: Option<&Watched>, after: Option<FileId>) -> bool {
    before.is_some_and(|it| Some(it.id) == after)
}

/// True when everything the event names is still there with a write time from
/// before the watch went on, so nothing has happened to it since.
///
/// macOS hands over a file written just before the watch went on as though it
/// had been written after, and can be a second or more late about it. That
/// reads as a change nobody made, one line and one refresh into every run.
///
/// Worth asking only in the first moments. Later a write time is no judge: a
/// copy that keeps the times of the files it copies writes new contents with
/// old times.
fn from_before_the_watch(event: &DebouncedEvent, watch_went_on: SystemTime) -> bool {
    // Dropped events name nothing to ask, or on macOS the directory they were
    // under, which says nothing about what was dropped.
    !event.need_rescan()
        && !event.paths.is_empty()
        && event.paths.iter().all(|path| {
            std::fs::metadata(path)
                .and_then(|about| about.modified())
                .is_ok_and(|written| written <= watch_went_on)
        })
}

/// True when the browser should refresh.
///
/// Reading a file is an event of its own on Linux, so serving a page would
/// count as a change, the browser would reload, and that reload would read
/// the file again — forever.
fn is_change(ignored: &Ignored, root: &Path, event: &DebouncedEvent, polling: bool) -> bool {
    // The system dropped events on the way, so anything may have changed.
    // Nothing is named, so this comes before the rules about paths.
    if event.need_rescan() {
        return true;
    }

    let written = match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any => true,
        // How a poll says a file was written, where the write time moved; where
        // it did not, the changed contents arrive as a write of their own.
        // Reading a file leaves the write time alone, so there is no loop here.
        //
        // Not for a directory: its own write time moves whenever anything in it
        // is created or removed, an ignored file included, and the served
        // directory is never itself ignored. Whatever changed has its own event.
        EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)) => {
            polling && event.paths.iter().any(|path| !path.is_dir())
        }
        // The bytes did not change. Reading a file updates its access time,
        // and counting that would start the loop again. Only Linux says this
        // plainly; macOS can report a permission change as if the file was
        // written.
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Modify(_) => true,
        // A file open for writing has just been closed: a finished save.
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) | EventKind::Other => false,
    };

    written && event.paths.iter().any(|path| !ignored.contains(root, path))
}

/// The directory the watcher is attached to.
struct Watched {
    /// Held open on Unix so the system cannot give this directory's number to
    /// the next one. Without it, a rebuild that lands in the same spot on disk
    /// looks like no change at all. A directory this program may not open
    /// still has its number remembered, only without that protection.
    #[cfg(unix)]
    _open: Option<std::fs::File>,
    id: FileId,
}

impl Watched {
    #[cfg(unix)]
    fn at(path: &Path) -> Option<Watched> {
        // Opened before the number is read, so the number cannot change hands
        // in between.
        let open = std::fs::File::open(path).ok();
        Some(Watched {
            _open: open,
            id: directory_id(path)?,
        })
    }

    #[cfg(not(unix))]
    fn at(path: &Path) -> Option<Watched> {
        Some(Watched {
            id: directory_id(path)?,
        })
    }
}

/// The system's own number for the directory this name leads to right now.
fn directory_id(path: &Path) -> Option<FileId> {
    file_id::get_file_id(path).ok()
}

/// How long the watcher's own account of a rebuild goes on arriving after the
/// rebuild was announced: as long as the watcher takes to hear about it, plus
/// the one [`CHECK_INTERVAL`] by which the check got there first.
///
/// Anything arriving inside the window goes unreported, a real change saved just
/// after a rebuild included; the page still refreshes, only the line is lost. A
/// look slower than [`POLL_INTERVAL`] carries the echo past the window instead,
/// and then one rebuild is announced twice. A lost line reads better than that,
/// so the window stays as tight as it is.
fn same_rebuild(poll: bool) -> Duration {
    // A poll knows nothing until its next look; the system's own notifications
    // are there at once, held back only by the debounce.
    let hears_within = if poll {
        POLL_INTERVAL + DEBOUNCE_DELAY
    } else {
        DEBOUNCE_DELAY
    };

    hears_within + CHECK_INTERVAL
}

/// Marks a rebuild as announced as of now, so the watcher's own account of it
/// is recognised as the echo it is.
fn announce(at: &Mutex<Option<Instant>>) {
    if let Ok(mut at) = at.lock() {
        *at = Some(Instant::now());
    }
}

/// Tells the watcher which directory the watch is on now.
fn remember(watching: &Mutex<Option<FileId>>, watched: &Option<Watched>) {
    if let Ok(mut watching) = watching.lock() {
        *watching = watched.as_ref().map(|it| it.id);
    }
}

/// True when the name no longer leads to the directory the watch is on: a
/// build has taken the served directory away, or has written the new one and
/// the check has not looked yet. Whatever the watcher reports then is that
/// rebuild, and it is the check's to announce; said here as well, one rebuild
/// would be announced twice.
///
/// A directory that is there but cannot be read is not one that went. The
/// check has no number to compare either, so what the watcher found is
/// reported as the change it is rather than going unsaid.
fn replaced_under_the_watch(watching: &Mutex<Option<FileId>>, root: &Path) -> bool {
    // The lock is let go before the look: on a network share the look takes
    // its time, and the check needs the lock to say where the watch is now.
    let Some(watched) = watching.lock().ok().and_then(|watching| *watching) else {
        return false;
    };

    match file_id::get_file_id(root) {
        Ok(now) => now != watched,
        // Not there: a build between the old directory and the new one.
        Err(trouble) => trouble.kind() == ErrorKind::NotFound,
    }
}

/// True when a rebuild was announced a moment ago, so what the watcher is
/// reporting now is that same rebuild reaching it the slower way.
fn just_announced(at: &Mutex<Option<Instant>>, within: Duration) -> bool {
    let Ok(at) = at.lock() else {
        return false;
    };

    at.is_some_and(|when| when.elapsed() < within)
}

/// The whole line to print about what one look could not read, or nothing where
/// saying anything would be noise.
fn cannot_look(
    ignored: &Ignored,
    root: &Path,
    error: &notify_debouncer_full::notify::Error,
) -> Option<String> {
    let Some(path) = error.paths.first() else {
        // Nothing to name. Not a watch that went down either: the next look
        // starts afresh.
        return Some(format!(
            "Cannot look at part of the directory: {}",
            cannot_watch(error)
        ));
    };

    // A path the browser never sees cannot change what it shows: a file in
    // node_modules this program may not read.
    if ignored.contains(root, path) {
        return None;
    }

    // Not there when the look reached it: a build removing a file and writing
    // it again, or clearing the served directory before filling it. Either it
    // stays gone, and the removal is reported as the change it is, or it is
    // already back. The check says when the directory itself returns.
    if was_not_there(error) {
        return None;
    }

    Some(format!(
        "Cannot look at {}: {}",
        short_name(root, path),
        cannot_watch(error)
    ))
}

/// A path named the short way, below the served directory the banner has
/// already named in full. That directory itself keeps its whole path, having
/// nothing left to name it by.
fn short_name(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(below) if !below.as_os_str().is_empty() => below.display().to_string(),
        _ => path.display().to_string(),
    }
}

/// True when the look found nothing at that name.
fn was_not_there(error: &notify_debouncer_full::notify::Error) -> bool {
    match &error.kind {
        WatchError::Io(io) => io.kind() == ErrorKind::NotFound,
        WatchError::PathNotFound => true,
        _ => false,
    }
}

/// Why the watcher could not be set up, in plain words. The limit is the one
/// people actually meet: every directory below the served one costs a watch,
/// and `node_modules` can use them all up.
fn cannot_watch(error: &notify_debouncer_full::notify::Error) -> String {
    match &error.kind {
        WatchError::MaxFilesWatch => "the system has no file watches left — serve the build \
             output rather than the whole project, or raise the limit"
            .to_string(),
        WatchError::PathNotFound => "there is no such directory".to_string(),
        WatchError::Io(io) => crate::errors::why(io),
        _ => error.to_string(),
    }
}

/// The same, naming the directory the trouble is with, which may lie below
/// the served one. Where that one cannot be read there are two ways round,
/// and both are worth a mention.
fn cannot_watch_here(root: &Path, error: &notify_debouncer_full::notify::Error) -> anyhow::Error {
    let named = error.paths.first().map_or(root, PathBuf::as_path);
    let mut why = cannot_watch(error);
    if named != root && may_not_read(error) {
        why.push_str(" — --poll watches what it may, and --no-reload serves without watching");
    }

    anyhow::Error::msg(why).context(format!("cannot watch {} for changes", named.display()))
}

/// True when the system refused to let this program read the path.
fn may_not_read(error: &notify_debouncer_full::notify::Error) -> bool {
    matches!(&error.kind, WatchError::Io(io) if io.kind() == ErrorKind::PermissionDenied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify_debouncer_full::notify::Event;
    use notify_debouncer_full::notify::event::{CreateKind, DataChange, RemoveKind};

    fn event(kind: EventKind, paths: &[&str]) -> DebouncedEvent {
        let event = paths
            .iter()
            .fold(Event::new(kind), |event, path| event.add_path(path.into()));

        DebouncedEvent::new(event, Instant::now())
    }

    fn written(paths: &[&str]) -> DebouncedEvent {
        event(EventKind::Modify(ModifyKind::Data(DataChange::Any)), paths)
    }

    const ROOT: &str = "/site";

    fn nothing_chosen() -> Ignored {
        Ignored::from(&[], &[]).expect("no patterns to refuse")
    }

    #[test]
    fn reading_a_file_is_not_a_change() {
        let read = event(
            EventKind::Access(AccessKind::Open(AccessMode::Any)),
            &["/site/index.html"],
        );

        assert!(!is_change(&nothing_chosen(), Path::new(ROOT), &read, false));
    }

    #[test]
    fn a_finished_save_is_a_change() {
        let saved = event(
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
            &["/site/index.html"],
        );

        assert!(is_change(&nothing_chosen(), Path::new(ROOT), &saved, false));
    }

    #[test]
    fn a_poll_says_a_file_was_written_by_its_write_time() {
        // The system's own watcher reports a write time for a file that was
        // merely read, where reloading would read it again.
        let rewritten = event(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)),
            &["/site/index.html"],
        );

        assert!(is_change(
            &nothing_chosen(),
            Path::new(ROOT),
            &rewritten,
            true
        ));
        assert!(!is_change(
            &nothing_chosen(),
            Path::new(ROOT),
            &rewritten,
            false
        ));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_directorys_own_write_time_is_not_a_change() {
        // Counting it refreshed the browser for every swap file vim wrote: the
        // served directory is never itself ignored.
        let root = std::env::temp_dir().join(format!("servio-dir-time-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let touched = event(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)),
            &[root.to_str().unwrap()],
        );

        assert!(!is_change(&nothing_chosen(), &root, &touched, true));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn permissions_and_timestamps_are_not_changes() {
        let touched = event(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            &["/site/index.html"],
        );

        assert!(!is_change(
            &nothing_chosen(),
            Path::new(ROOT),
            &touched,
            false
        ));
    }

    #[test]
    fn dropped_events_are_a_change_whatever_was_ignored_and_however_new_the_watch() {
        // A build writing more files than the system can report: it says so
        // once, naming nothing, and everything may have changed.
        use notify_debouncer_full::notify::event::Flag;
        let dropped = DebouncedEvent::new(
            Event::new(EventKind::Other).set_flag(Flag::Rescan),
            Instant::now(),
        );

        assert!(is_change(
            &nothing_chosen(),
            Path::new(ROOT),
            &dropped,
            false
        ));
        assert!(
            !from_before_the_watch(&dropped, SystemTime::now()),
            "a report naming no file was taken for one about old files"
        );
    }

    #[test]
    fn writing_creating_and_deleting_are_changes() {
        let root = Path::new(ROOT);
        assert!(is_change(
            &nothing_chosen(),
            root,
            &written(&["/site/app.css"]),
            false
        ));
        assert!(is_change(
            &nothing_chosen(),
            root,
            &event(EventKind::Create(CreateKind::File), &["/site/new.css"]),
            false
        ));
        assert!(is_change(
            &nothing_chosen(),
            root,
            &event(EventKind::Remove(RemoveKind::File), &["/site/old.css"]),
            false
        ));
    }

    #[test]
    fn writing_an_ignored_file_is_not_a_change() {
        assert!(!is_change(
            &nothing_chosen(),
            Path::new(ROOT),
            &written(&["/site/app.css.swp"]),
            false
        ));
    }

    #[test]
    fn one_real_file_among_ignored_ones_is_a_change() {
        let mixed = written(&["/site/app.css.swp", "/site/app.css"]);
        assert!(is_change(&nothing_chosen(), Path::new(ROOT), &mixed, false));
    }

    #[test]
    fn polling_waits_longer_to_recognise_the_echo_of_a_rebuild() {
        // A poll hears about a rebuild a whole look later, and the window has
        // to cover that; too short, and one rebuild is announced twice.
        assert!(same_rebuild(true) >= same_rebuild(false) + POLL_INTERVAL);
    }

    fn could_not_read(path: &Path, kind: ErrorKind) -> notify_debouncer_full::notify::Error {
        notify_debouncer_full::notify::Error::io(kind.into()).add_path(path.to_path_buf())
    }

    #[test]
    fn a_path_that_was_not_there_is_not_worth_a_word() {
        // A build removes a file and writes it again, and the look lands in
        // between. Whether it is back by now makes no difference.
        let root = Path::new(ROOT);
        let rewritten = Path::new("/site/app.css");

        assert_eq!(
            cannot_look(
                &nothing_chosen(),
                root,
                &could_not_read(rewritten, ErrorKind::NotFound)
            ),
            None
        );

        // One that is there and cannot be read is worth saying, in words that
        // do not call a stylesheet a directory.
        let said = cannot_look(
            &nothing_chosen(),
            root,
            &could_not_read(rewritten, ErrorKind::PermissionDenied),
        )
        .expect("a path that cannot be read should be reported");
        assert!(said.starts_with("Cannot look at app.css:"), "{said}");
        assert!(!said.contains("directory"), "{said}");
    }

    #[test]
    fn trouble_with_no_path_is_not_a_watch_that_went_down() {
        let error = notify_debouncer_full::notify::Error::generic("a walk with no end");
        let said =
            cannot_look(&nothing_chosen(), Path::new(ROOT), &error).expect("it should be reported");

        assert!(!said.contains("Cannot watch for changes"), "{said}");
        assert!(said.contains("a walk with no end"), "{said}");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_new_directory_of_the_same_name_is_a_different_directory() {
        let path = std::env::temp_dir().join(format!("servio-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();

        let watched = Watched::at(&path).expect("could not look at the directory");
        assert_eq!(
            directory_id(&path),
            Some(watched.id),
            "the same directory read twice"
        );

        std::fs::remove_dir_all(&path).unwrap();
        assert_eq!(directory_id(&path), None, "there is no directory to read");

        std::fs::create_dir_all(&path).unwrap();
        assert_ne!(
            directory_id(&path),
            Some(watched.id),
            "a rebuilt directory read as the old one"
        );

        std::fs::remove_dir_all(&path).unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_watch_put_on_nothing_is_not_on_a_directory() {
        // Between the two looks the directory went: a poll takes the watch
        // all the same, and nothing agreeing with nothing let that pass.
        let path = std::env::temp_dir().join(format!("servio-nothing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let before = Watched::at(&path).expect("could not look at the directory");

        assert!(same_directory(Some(&before), directory_id(&path)));
        assert!(
            !same_directory(None, None),
            "nothing was taken for a directory"
        );
        assert!(!same_directory(Some(&before), None));

        std::fs::remove_dir_all(&path).unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn what_the_watcher_reports_is_left_to_the_check_once_the_directory_is_replaced() {
        let path = std::env::temp_dir().join(format!("servio-replaced-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();

        // Kept, so that the system cannot give the rebuilt directory the same
        // number.
        let watched = Watched::at(&path).expect("could not look at the directory");
        let watching = Mutex::new(Some(watched.id));
        assert!(
            !replaced_under_the_watch(&watching, &path),
            "the directory at the name is the one watched"
        );

        std::fs::remove_dir_all(&path).unwrap();
        assert!(
            replaced_under_the_watch(&watching, &path),
            "taken away, and the check says when it is back"
        );

        std::fs::create_dir_all(&path).unwrap();
        assert!(
            replaced_under_the_watch(&watching, &path),
            "rebuilt, and the check has not looked yet"
        );

        // Nothing known to be watched, so there is nothing to compare.
        assert!(!replaced_under_the_watch(&Mutex::new(None), &path));

        std::fs::remove_dir_all(&path).unwrap();
    }

    #[test]
    fn a_watcher_that_went_down_is_set_up_again_but_a_look_that_could_not_read_is_not() {
        let (changed, _changes) = mpsc::channel();
        let trouble = || Err(vec![notify::Error::generic("the watcher went down")]);

        let (failed, failures) = mpsc::channel();
        let mut report = report_changes(
            PathBuf::from(ROOT),
            false,
            nothing_chosen(),
            Rebuilds::new(false),
            changed.clone(),
            failed,
        );
        report(trouble());
        assert!(
            failures.try_recv().is_ok(),
            "a watcher that went down was left down"
        );

        // Out of watches for a directory created just now: what is watched
        // still is, and a new watch would run out partway through.
        let (failed, failures) = mpsc::channel();
        let mut report = report_changes(
            PathBuf::from(ROOT),
            false,
            nothing_chosen(),
            Rebuilds::new(false),
            changed.clone(),
            failed,
        );
        report(Err(vec![
            notify::Error::new(WatchError::MaxFilesWatch).add_path(PathBuf::from("/site/new")),
        ]));
        assert!(
            failures.try_recv().is_err(),
            "running out of watches tore down the watch that was on"
        );

        // A look reads the whole tree every time, so one path it could not
        // read is worth a word and not a new watch.
        let (failed, failures) = mpsc::channel();
        let mut report = report_changes(
            PathBuf::from(ROOT),
            true,
            nothing_chosen(),
            Rebuilds::new(true),
            changed,
            failed,
        );
        report(trouble());
        assert!(
            failures.try_recv().is_err(),
            "one path a look could not read was taken for a watch that went down"
        );
    }

    #[test]
    fn a_file_written_before_the_watch_went_on_is_not_a_change() {
        let dir = std::env::temp_dir().join(format!("servio-settling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let page = dir.join("index.html");
        std::fs::write(&page, "<html>before</html>").unwrap();

        let watch_went_on = SystemTime::now();
        let about_the_page = written(&[page.to_str().unwrap()]);
        assert!(
            from_before_the_watch(&about_the_page, watch_went_on),
            "a file nobody touched was taken for one that changed"
        );

        // The system stamps a file by a coarser clock than this one, so a
        // moment's wait keeps the two writes on either side of the moment.
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(&page, "<html>after</html>").unwrap();
        assert!(
            !from_before_the_watch(&about_the_page, watch_went_on),
            "a file written since the watch went on is a change"
        );

        // Gone, so nothing says it was there all along.
        std::fs::remove_file(&page).unwrap();
        assert!(!from_before_the_watch(&about_the_page, watch_went_on));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_that_cannot_be_read_is_not_one_that_went() {
        // Nothing else would report the change: the check gives up on a
        // number it cannot read.
        use std::os::unix::fs::PermissionsExt;

        let above = std::env::temp_dir().join(format!("servio-unreadable-{}", std::process::id()));
        let path = above.join("site");
        let _ = std::fs::remove_dir_all(&above);
        std::fs::create_dir_all(&path).unwrap();
        let mode = |mode| std::fs::set_permissions(&above, std::fs::Permissions::from_mode(mode));

        let watched = Watched::at(&path).expect("could not look at the directory");
        let watching = Mutex::new(Some(watched.id));
        mode(0o000).unwrap();
        let went = replaced_under_the_watch(&watching, &path);

        // Put back before anything can fail, so the directory can be cleared
        // afterwards.
        mode(0o755).unwrap();
        assert!(
            !went,
            "a directory that could not be read was taken for one that went"
        );

        std::fs::remove_dir_all(&above).unwrap();
    }
}
