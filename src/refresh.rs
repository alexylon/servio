//! When to refresh the browser after the watcher reports changes: once they
//! stop for a moment, so a build that writes many files refreshes it only once.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How long the files have to stay quiet before the browser is refreshed. The
/// watcher hands over what it has every few hundredths of a second, and a
/// build writes for longer than that; refreshed at each handover, the browser
/// would reload a dozen times for one build.
const QUIET_FOR: Duration = Duration::from_millis(150);

/// How long files that never go quiet can put a refresh off: a log written
/// every moment, a build that runs for minutes. Past this the browser is
/// refreshed anyway, and again as the writing goes on.
const PUT_OFF_AT_MOST: Duration = Duration::from_secs(1);

/// What came of waiting for a change.
pub(crate) enum Heard {
    /// A change, and whether it is worth a line in the terminal.
    Change { at: Instant, worth_a_line: bool },
    /// Nothing came before the wait ended.
    Quiet,
    /// The watcher stopped, so nothing more will come.
    Gone,
}

/// Changes that end in one refresh.
struct Burst {
    began: Instant,
    last: Instant,
    worth_a_line: bool,
}

impl Burst {
    fn began(at: Instant, worth_a_line: bool) -> Burst {
        Burst {
            began: at,
            last: at,
            worth_a_line,
        }
    }

    /// True once the burst has lasted [`PUT_OFF_AT_MOST`], so the refresh is
    /// due now.
    fn add(&mut self, at: Instant, worth_a_line: bool) -> bool {
        self.last = at;
        self.worth_a_line |= worth_a_line;
        at.duration_since(self.began) >= PUT_OFF_AT_MOST
    }

    /// When the burst is over, if nothing more arrives.
    fn quiet_at(&self) -> Instant {
        self.last + QUIET_FOR
    }
}

/// Calls `refresh` once for each burst of changes: when none has come for
/// [`QUIET_FOR`], or at a change once the burst has lasted [`PUT_OFF_AT_MOST`].
/// `next` waits for a change until the time it is given, or for as long as it
/// takes when given none.
pub(crate) fn refresh_once_quiet(
    mut next: impl FnMut(Option<Instant>) -> Heard,
    mut refresh: impl FnMut(bool),
) {
    let mut burst: Option<Burst> = None;
    loop {
        match next(burst.as_ref().map(Burst::quiet_at)) {
            Heard::Change { at, worth_a_line } => {
                if let Some(open) = burst.as_mut() {
                    if open.add(at, worth_a_line) {
                        refresh(open.worth_a_line);
                        burst = None;
                    }
                } else {
                    burst = Some(Burst::began(at, worth_a_line));
                }
            }
            Heard::Quiet => {
                if let Some(done) = burst.take() {
                    refresh(done.worth_a_line);
                }
            }
            Heard::Gone => {
                if let Some(done) = burst.take() {
                    refresh(done.worth_a_line);
                }
                return;
            }
        }
    }
}

/// Waits for the watcher's next change, up to the given time if there is one.
/// A change already waiting counts even when that time has passed.
pub(crate) fn wait_for_change(changes: &Receiver<bool>, until: Option<Instant>) -> Heard {
    let received = match until {
        None => changes.recv().map_err(|_| RecvTimeoutError::Disconnected),
        Some(until) => changes.recv_timeout(until.saturating_duration_since(Instant::now())),
    };

    match received {
        Ok(worth_a_line) => Heard::Change {
            at: Instant::now(),
            worth_a_line,
        },
        Err(RecvTimeoutError::Timeout) => Heard::Quiet,
        Err(RecvTimeoutError::Disconnected) => Heard::Gone,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::VecDeque;

    /// What the watcher does, in milliseconds from the start: sends a change,
    /// and whether it is worth a line, or stops.
    enum Step {
        Change(u64, bool),
        Stops(u64),
    }

    /// The refreshes `steps` lead to, as milliseconds from the start and
    /// whether each was worth a line. The clock jumps to the next step instead
    /// of waiting. The times are when changes arrive here, which on a busy
    /// machine can be later than when the files were written.
    fn refreshes(steps: &[Step]) -> Vec<(u64, bool)> {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let now = Cell::new(start);
        let mut steps: VecDeque<&Step> = steps.iter().collect();
        let mut refreshes = Vec::new();

        refresh_once_quiet(
            |until| {
                let next = steps.front().map(|step| match step {
                    Step::Change(ms, _) | Step::Stops(ms) => at(*ms),
                });
                match (next, until) {
                    // A step due the moment the wait ends comes after it.
                    (Some(next), Some(until)) if next >= until => {
                        now.set(until);
                        Heard::Quiet
                    }
                    (Some(next), _) => {
                        now.set(next);
                        match steps.pop_front() {
                            Some(Step::Change(_, worth_a_line)) => Heard::Change {
                                at: next,
                                worth_a_line: *worth_a_line,
                            },
                            _ => Heard::Gone,
                        }
                    }
                    (None, Some(until)) => {
                        now.set(until);
                        Heard::Quiet
                    }
                    (None, None) => Heard::Gone,
                }
            },
            |worth_a_line| {
                let ms = now.get().duration_since(start).as_millis() as u64;
                refreshes.push((ms, worth_a_line));
            },
        );

        refreshes
    }

    /// `count` changes, `gap` milliseconds apart.
    fn every(gap: u64, count: u64) -> Vec<Step> {
        (0..count).map(|n| Step::Change(n * gap, true)).collect()
    }

    #[test]
    fn one_change_refreshes_once_the_files_go_quiet() {
        assert_eq!(refreshes(&[Step::Change(0, true)]), [(150, true)]);
    }

    #[test]
    fn a_build_writing_faster_than_the_quiet_time_refreshes_once() {
        assert_eq!(refreshes(&every(50, 12)), [(700, true)]);
    }

    #[test]
    fn a_pause_as_long_as_the_quiet_time_splits_the_refresh() {
        let paused_just_short = [Step::Change(0, true), Step::Change(149, true)];
        assert_eq!(refreshes(&paused_just_short), [(299, true)]);

        let paused_long_enough = [Step::Change(0, true), Step::Change(150, true)];
        assert_eq!(refreshes(&paused_long_enough), [(150, true), (300, true)]);
    }

    #[test]
    fn changes_that_never_stop_refresh_once_a_second() {
        assert_eq!(
            refreshes(&every(100, 26)),
            [(1000, true), (2100, true), (2650, true)]
        );
    }

    #[test]
    fn a_refresh_is_put_off_a_whole_second_and_no_longer() {
        let mut just_short = every(100, 10);
        just_short.push(Step::Change(999, true));
        assert_eq!(refreshes(&just_short), [(1149, true)]);

        let mut a_whole_second = every(100, 10);
        a_whole_second.push(Step::Change(1000, true));
        assert_eq!(refreshes(&a_whole_second), [(1000, true)]);
    }

    #[test]
    fn a_refresh_is_worth_a_line_when_any_of_its_changes_is() {
        let one_of_three = [
            Step::Change(0, false),
            Step::Change(50, true),
            Step::Change(100, false),
        ];
        assert_eq!(refreshes(&one_of_three), [(250, true)]);

        let none = [Step::Change(0, false), Step::Change(50, false)];
        assert_eq!(refreshes(&none), [(200, false)]);
    }

    #[test]
    fn changes_still_waiting_are_refreshed_when_the_watcher_stops() {
        assert_eq!(
            refreshes(&[Step::Change(0, true), Step::Stops(50)]),
            [(50, true)]
        );
        assert_eq!(refreshes(&[Step::Stops(50)]), []);
    }

    #[test]
    fn the_channel_tells_a_change_from_quiet_and_from_a_stopped_watcher() {
        let (send, changes) = std::sync::mpsc::channel();

        send.send(true).unwrap();
        assert!(matches!(
            wait_for_change(&changes, None),
            Heard::Change {
                worth_a_line: true,
                ..
            }
        ));

        // Already waiting, so it counts even though the wait is over.
        send.send(false).unwrap();
        assert!(matches!(
            wait_for_change(&changes, Some(Instant::now())),
            Heard::Change {
                worth_a_line: false,
                ..
            }
        ));

        assert!(matches!(
            wait_for_change(&changes, Some(Instant::now())),
            Heard::Quiet
        ));

        drop(send);
        assert!(matches!(wait_for_change(&changes, None), Heard::Gone));
    }
}
