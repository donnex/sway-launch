//! Correlating a launched window back to the process this invocation spawned.
//!
//! Sway broadcasts window events to every IPC connection, so matching a `New`
//! window on app_id/class alone can pick up a window some other process
//! launched. The launched command's environment is tagged with a token, and
//! `/proc/<pid>/environ` is what confirms a window's process carries it. See
//! `confirmation.rs`'s `run_wait_matching_exec_event()` for how the signal is
//! used, including what happens when it can't be confirmed.

use std::time;

/// Environment variable name `run_wait_matching_exec_event()` tags a
/// launched command's environment with, to correlate a matching `New`
/// window event back to the specific process this invocation spawned. See
/// that function's doc comment for the full mechanism.
pub(super) const PID_MARKER_VAR: &str = "SWAY_LAUNCH_PID_MARKER";

/// How long `run_wait_matching_exec_event()` waits for a PID-marker-
/// confirmed match after seeing a content-matching-but-unconfirmed one,
/// before giving up and using that fallback candidate — independent of the
/// overall `--timeout` (though still capped by it, via `deadline.min(...)`,
/// for a short `--timeout`), so a genuinely ambiguous case adds a bounded
/// delay rather than the full timeout. Live testing under concurrent load
/// showed a shorter cap (500ms) occasionally forces a fallback before the
/// real PID-marker-confirmed match — which is still coming, just slightly
/// delayed by system load — arrives, causing exactly the wrong-container-id
/// collision this mechanism exists to prevent; 2s comfortably clears that
/// without meaningfully slowing the genuinely-ambiguous (single-instance
/// application) case, which resolves via `MarkerTracker::is_gone()` well
/// before this cap in practice. It's also how a launch whose marker is never
/// observed at all (a launcher that drops the environment, a non-dumpable
/// program, no `/proc`) ends: as `LaunchOwnership::Adopted`, after this cap.
pub(super) const PID_MARKER_FALLBACK_GRACE: time::Duration = time::Duration::from_millis(2000);

/// A random-enough per-invocation token for `PID_MARKER_VAR`: this process's
/// own pid (unique across concurrently-running `sway-launch` invocations,
/// which is all that actually matters here) plus a nanosecond timestamp (so
/// a single process running several `Exec` actions in sequence — e.g. a
/// multi-step `--layout` — doesn't reuse the same token for each).
pub(super) fn generate_pid_marker_token() -> String {
    let nanos = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{}", std::process::id(), nanos)
}

/// How often `run_wait_matching_exec_event()` re-scans `/proc` for the
/// marker while it still needs an answer: until the marked process has been
/// seen at all, and then, once a fallback candidate is being held, until it's
/// gone. The scan costs 0.5-16ms depending on process count (see
/// `scan_marker_presence()`), so this keeps it to a small fraction of one core
/// in the worst case, while still catching a marked process that lives only a
/// few dozen milliseconds — a single-instance application's forwarding client
/// lives far longer than that.
pub(super) const MARKER_SCAN_INTERVAL: time::Duration = time::Duration::from_millis(25);

/// What reading one process's `/proc/<pid>/environ` said about the marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarkerRead {
    /// The marker is in the process's environment.
    Present,
    /// The process exists without the marker, or doesn't exist (any more).
    Absent,
    /// The process exists, but its environment couldn't be read — most often a
    /// non-dumpable process (`prctl(PR_SET_DUMPABLE, 0)`, setuid/setgid, file
    /// capabilities), whose `environ` is owned by root even for its own user.
    /// Says nothing either way about the marker.
    Unreadable,
}

/// What a whole-`/proc` scan says about whether the marked process (or a
/// marked descendant) is still running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MarkerPresence {
    /// These processes carry the marker.
    Present(Vec<i32>),
    /// Nothing carries the marker, and no process previously seen with it is
    /// hiding behind an unreadable `environ`.
    Absent,
    /// Nothing readable carries the marker, but a process previously seen
    /// with it now can't be read, so its absence can't be concluded.
    Unknown,
    /// `/proc` itself can't be listed, so nothing can be concluded at all.
    Unavailable,
}

/// Reads `/proc/<pid>/environ` and looks for exactly `<var_name>=<expected_value>`
/// as one of its NUL-separated entries.
///
/// Only a missing process (`NotFound`, or `ESRCH` from a process exiting mid-read)
/// counts as `Absent`; any other read failure is `Unreadable`, since a process
/// whose environment can't be read may well be carrying the marker.
pub(super) fn read_process_marker(pid: i32, var_name: &str, expected_value: &str) -> MarkerRead {
    const ESRCH: i32 = 3;
    let environ = match std::fs::read(format!("/proc/{}/environ", pid)) {
        Ok(environ) => environ,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(ESRCH) =>
        {
            return MarkerRead::Absent;
        }
        Err(_) => return MarkerRead::Unreadable,
    };
    let needle = format!("{}={}", var_name, expected_value);
    if environ
        .split(|&byte| byte == 0)
        .any(|entry| entry == needle.as_bytes())
    {
        MarkerRead::Present
    } else {
        MarkerRead::Absent
    }
}

/// Whether `/proc/<pid>/environ` provably carries the marker. Used to confirm
/// a window's own process; anything short of `MarkerRead::Present` is "not
/// confirmed", which sends the window down the fallback path.
pub(super) fn process_has_env_var(pid: i32, var_name: &str, expected_value: &str) -> bool {
    read_process_marker(pid, var_name, expected_value) == MarkerRead::Present
}

/// Folds one scan's per-process reads into a `MarkerPresence`. Split out of
/// `scan_marker_presence()` so the decision is testable without arranging real
/// processes.
///
/// An unreadable process only blocks an `Absent` answer when it's one already
/// seen carrying the marker (`previously_seen`). Treating *every* unreadable
/// process that way would make `Absent` unreachable on an ordinary desktop:
/// root's processes, and the user's own non-dumpable ones (ssh-agent,
/// gpg-agent), are unreadable on every scan regardless of what this run
/// launched.
pub(super) fn classify_marker_reads(
    reads: impl IntoIterator<Item = (i32, MarkerRead)>,
    previously_seen: &[i32],
) -> MarkerPresence {
    let mut carriers = Vec::new();
    let mut seen_process_unreadable = false;
    for (pid, read) in reads {
        match read {
            MarkerRead::Present => carriers.push(pid),
            MarkerRead::Unreadable if previously_seen.contains(&pid) => {
                seen_process_unreadable = true;
            }
            MarkerRead::Unreadable | MarkerRead::Absent => {}
        }
    }
    if !carriers.is_empty() {
        MarkerPresence::Present(carriers)
    } else if seen_process_unreadable {
        MarkerPresence::Unknown
    } else {
        MarkerPresence::Absent
    }
}

/// Scans every process for `<var_name>=<expected_value>`. Used by
/// `run_wait_matching_exec_event()` to tell whether the command it spawned
/// (or a descendant that inherited the marker) is running, has not started
/// yet, or has already exited — e.g. a single-instance application that
/// forwards a request to an already-running instance and exits, with no
/// further marker-confirmed match ever coming.
///
/// `Absent` on its own does **not** mean "exited": it's equally the answer
/// before the marked process exists (Sway's `exec` returns once it has
/// forked) and for a launcher that drops the environment before the real
/// program starts. The caller only reads it as "gone" after an earlier scan
/// returned `Present`; see that function's doc comment.
///
/// This scans every process, which looks alarming and has been raised as a
/// performance concern; measured before acting on it. A full miss — the worst
/// case, where every `/proc/<pid>/environ` is read — costs 0.5ms on a 26-process
/// system and 16ms on a 525-process one. It runs only while the caller still
/// needs an answer, at most every `MARKER_SCAN_INTERVAL`, inside a window
/// bounded by `PID_MARKER_FALLBACK_GRACE` once a fallback candidate exists.
///
/// Scanning is also not an implementation shortcut that a narrower lookup could
/// replace: `sway-launch` never spawns the process. Sway does, via
/// `exec export <marker> && <command>`, so there is no child pid to inspect — which
/// is the entire reason the environment marker exists. A "just look at the pid
/// we spawned" design has nothing to look at.
///
/// On a restricted `/proc` the scan can't conclude anything, and says so
/// (`Unavailable`, logged under `--verbose`) rather than answering `Absent`:
/// confirmation then degrades into the content-match fallback, which yields
/// `LaunchOwnership::Adopted` and so keeps `--rollback-on-error` from killing
/// a window this run never proved was its own.
pub(super) fn scan_marker_presence(
    var_name: &str,
    expected_value: &str,
    previously_seen: &[i32],
) -> MarkerPresence {
    // Not unit-tested: reliably making /proc unreadable from within a test
    // isn't portable/safe to simulate, so this branch stays uncovered.
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return MarkerPresence::Unavailable;
    };
    classify_marker_reads(
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
            .map(|pid| (pid, read_process_marker(pid, var_name, expected_value))),
        previously_seen,
    )
}

/// What `run_wait_matching_exec_event()` has learned about its marked
/// process so far, accumulated across `scan_marker_presence()` calls. Kept
/// apart from the scanning and the event loop so the "seen, then gone" rule is
/// testable on its own.
#[derive(Debug, Default)]
pub(super) struct MarkerTracker {
    seen: Vec<i32>,
    gone: bool,
    unavailable: bool,
}

impl MarkerTracker {
    /// Every pid a scan has found carrying the marker.
    pub(super) fn previously_seen(&self) -> &[i32] {
        &self.seen
    }

    /// Whether the marked process was seen running and has since gone —
    /// the only state in which nothing marker-confirmed can still arrive.
    /// Never true for a marker that was simply never observed: that's also
    /// what "not started yet" and "a launcher dropped the environment" look
    /// like, and in both a foreign window would otherwise be claimed.
    pub(super) fn is_gone(&self) -> bool {
        self.gone
    }

    /// Whether another scan could still change the outcome: before the marker
    /// has been seen, and, while a fallback candidate is held, until it's gone.
    /// Never once `/proc` turned out to be unavailable.
    pub(super) fn needs_scan(&self, holding_fallback: bool) -> bool {
        !self.gone && !self.unavailable && (self.seen.is_empty() || holding_fallback)
    }

    /// Records one scan. Returns `true` the first time `/proc` is found
    /// unavailable, so the caller can report it once.
    pub(super) fn observe(&mut self, presence: MarkerPresence) -> bool {
        match presence {
            MarkerPresence::Present(pids) => {
                for pid in pids {
                    if !self.seen.contains(&pid) {
                        self.seen.push(pid);
                    }
                }
                false
            }
            MarkerPresence::Absent => {
                self.gone = !self.seen.is_empty();
                false
            }
            MarkerPresence::Unknown => false,
            MarkerPresence::Unavailable => !std::mem::replace(&mut self.unavailable, true),
        }
    }
}

#[cfg(test)]
mod tests;
