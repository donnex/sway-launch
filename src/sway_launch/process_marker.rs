//! Correlating a launched window back to the process this invocation spawned.
//!
//! Sway broadcasts window events to every IPC connection, so matching a `New`
//! window on app_id/class alone can pick up a window some other process
//! launched. The launched command's environment is tagged with a token, and
//! `/proc/<pid>/environ` is what confirms a window's process carries it, or,
//! for a program that never saw it, an ancestor's (`launch_evidence()`). See
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
/// observed at all ends, as `LaunchOwnership::Adopted` after this cap, when
/// `launch_evidence()` finds no ancestry either: a program that detached
/// from the launch's shell, or no `/proc`.
pub(super) const PID_MARKER_FALLBACK_GRACE: time::Duration = time::Duration::from_millis(2000);

/// A random-enough per-invocation token for `PID_MARKER_VAR`: this process's
/// own pid (unique across concurrently-running `sway-launch` invocations),
/// a nanosecond timestamp (so a pid reused later doesn't repeat one), and a
/// counter unique within this process. The counter is what keeps
/// `--concurrent`'s launches (#73) apart: they generate their tokens on
/// separate threads at once, and two in the same clock tick would otherwise
/// share one, each then confirming the other's window as its own.
pub(super) fn generate_pid_marker_token() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{}-{}", std::process::id(), nanos, sequence)
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

/// How far `launch_evidence()` walks up from a window's process before giving
/// up: a launch's own shell, a wrapper or two, and the program itself are a
/// handful of generations, and the walk reads two small files per step.
const MAX_ANCESTRY_DEPTH: usize = 32;

/// Why a window's process counts as this launch's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LaunchEvidence {
    /// Its own environment carries the marker.
    OwnEnvironment,
    /// It descends from this process, which carries the marker in its
    /// environment, or is the shell Sway started for this launch.
    Ancestor(i32),
}

/// Whether the window owned by `pid` was opened by this launch, and why: its
/// own environment carries the marker, or one of its ancestors is this
/// launch's (#96).
///
/// The ancestry half is what recognises a window whose program never sees
/// the marker: a wrapper that unsets it (`env -u`), a launcher that starts
/// the program with a clean environment (`env -i`), or a process whose
/// `environ` can't be read. `/proc/<pid>/stat` stays readable for all of
/// them. The walk's anchor is the `sh -c "export <marker> && ..."` Sway
/// starts for the launch, recognised by its command line, since `export`
/// never shows in a shell's own `environ`; `ExecAction::sway_command()`
/// keeps that shell alive whichever shell `sh` is. A window from a process
/// that detached to init, or from a server another process asked (a
/// client/server launcher), has no such ancestor and still goes down the
/// fallback path. Confirmed live on Sway 1.9 and 1.11, with dash and bash.
pub(super) fn launch_evidence(pid: i32, token: &str) -> Option<LaunchEvidence> {
    if process_has_env_var(pid, PID_MARKER_VAR, token) {
        return Some(LaunchEvidence::OwnEnvironment);
    }
    marked_ancestor(pid, MAX_ANCESTRY_DEPTH, |ancestor| {
        let marked = launch_tokens(ancestor)
            .iter()
            .any(|carried| carried == token);
        (marked, parent_pid(ancestor))
    })
    .map(LaunchEvidence::Ancestor)
}

/// Tokens of every launch started since `ConcurrentLaunches::begin()`,
/// while its group lasts; `None` outside one.
static CONCURRENT_LAUNCHES: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

fn concurrent_launches() -> std::sync::MutexGuard<'static, Option<Vec<String>>> {
    CONCURRENT_LAUNCHES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A group of launches run side by side (`--concurrent`, #73): from
/// `begin()` until it's dropped, each launch's token is recorded by
/// `register_launch()`, so `is_sibling_launchs_window()` can tell the
/// others' windows apart from foreign ones, including after the launch
/// that opened one has finished.
pub struct ConcurrentLaunches(());

impl ConcurrentLaunches {
    pub fn begin() -> ConcurrentLaunches {
        *concurrent_launches() = Some(Vec::new());
        ConcurrentLaunches(())
    }
}

impl Drop for ConcurrentLaunches {
    fn drop(&mut self) {
        *concurrent_launches() = None;
    }
}

/// Records a launch's token in the current `ConcurrentLaunches` group, if
/// there is one.
pub(super) fn register_launch(token: &str) {
    if let Some(tokens) = concurrent_launches().as_mut() {
        tokens.push(token.to_string());
    }
}

/// Whether the window owned by `pid` is provably another launch's in the
/// same `ConcurrentLaunches` group: `--concurrent` (#73) runs a layout's
/// launches side by side, and each one's window would otherwise look like a
/// foreign window with the right app_id to the others, to be adopted once
/// `PID_MARKER_FALLBACK_GRACE` ran out on a slow start (seen on Sway 1.9 in
/// CI). Only the group's launches count, so a hand-off to an application an
/// earlier run launched (whose process still carries that run's marker) is
/// adopted as before. Outside a group, nothing is read.
pub(super) fn is_sibling_launchs_window(pid: i32, own_token: &str) -> bool {
    let siblings: Vec<String> = concurrent_launches()
        .iter()
        .flatten()
        .filter(|token| *token != own_token)
        .cloned()
        .collect();
    if siblings.is_empty() {
        return false;
    }
    let is_sibling = |process: i32| {
        launch_tokens(process)
            .iter()
            .any(|token| siblings.contains(token))
    };
    is_sibling(pid)
        || marked_ancestor(pid, MAX_ANCESTRY_DEPTH, |ancestor| {
            (is_sibling(ancestor), parent_pid(ancestor))
        })
        .is_some()
}

/// The launch tokens a process carries: the marker in its environment, and,
/// for the shell Sway started for a launch, the token in its command line.
fn launch_tokens(pid: i32) -> Vec<String> {
    let environ = std::fs::read(format!("/proc/{}/environ", pid)).ok();
    let cmdline = std::fs::read(format!("/proc/{}/cmdline", pid)).ok();
    environ
        .as_deref()
        .and_then(environ_marker)
        .into_iter()
        .chain(cmdline.as_deref().and_then(launch_shell_token))
        .collect()
}

/// The marker's value in a NUL-separated `/proc/<pid>/environ`, if set.
pub(super) fn environ_marker(environ: &[u8]) -> Option<String> {
    let prefix = format!("{}=", PID_MARKER_VAR);
    environ.split(|&byte| byte == 0).find_map(|entry| {
        entry
            .strip_prefix(prefix.as_bytes())
            .map(|value| String::from_utf8_lossy(value).into_owned())
    })
}

/// The first ancestor of `pid` (not `pid` itself) within `max_depth`
/// generations for which `inspect` reports `true`. `inspect` answers for
/// one process: whether it's marked, and its parent's pid, if known. The
/// walk stops at pid 1, at a process with no parent left to read, and at a
/// loop, which a pid reused mid-walk could produce.
pub(super) fn marked_ancestor(
    pid: i32,
    max_depth: usize,
    mut inspect: impl FnMut(i32) -> (bool, Option<i32>),
) -> Option<i32> {
    let mut visited = vec![pid];
    let mut next = inspect(pid).1;
    for _ in 0..max_depth {
        let ancestor = next.filter(|&ancestor| ancestor > 1 && !visited.contains(&ancestor))?;
        visited.push(ancestor);
        let (marked, parent) = inspect(ancestor);
        if marked {
            return Some(ancestor);
        }
        next = parent;
    }
    None
}

/// A process's parent pid, from `/proc/<pid>/stat`, which is readable for
/// every process, a non-dumpable one included.
fn parent_pid(pid: i32) -> Option<i32> {
    parent_pid_from_stat(&std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?)
}

/// The `ppid` field of a `/proc/<pid>/stat` line. The command name before
/// it is parenthesised and may itself contain spaces and `)`, so the fields
/// are counted from the last `)`.
pub(super) fn parent_pid_from_stat(stat: &str) -> Option<i32> {
    let fields = &stat[stat.rfind(')')? + 1..];
    fields.split_whitespace().nth(1)?.parse().ok()
}

/// When a process started, in clock ticks since boot, from `/proc/<pid>/stat`.
/// Only ever compared with another process's, so the tick length doesn't
/// matter.
pub(super) fn process_start_ticks(pid: i32) -> Option<u64> {
    start_ticks_from_stat(&std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?)
}

/// The `starttime` field (the 22nd) of a `/proc/<pid>/stat` line, counted
/// from the last `)` as `parent_pid_from_stat()` does.
pub(super) fn start_ticks_from_stat(stat: &str) -> Option<u64> {
    let fields = &stat[stat.rfind(')')? + 1..];
    fields.split_whitespace().nth(19)?.parse().ok()
}

/// The token of the launch whose shell a NUL-separated `/proc/<pid>/cmdline`
/// is, if it is one: `sh -c "export <marker>=<token> && ..."`, as
/// `ExecAction::sway_command()` writes it after `exec `.
pub(super) fn launch_shell_token(cmdline: &[u8]) -> Option<String> {
    let prefix = format!("export {}=", PID_MARKER_VAR);
    cmdline.split(|&byte| byte == 0).find_map(|argument| {
        let rest = argument.strip_prefix(prefix.as_bytes())?;
        let end = rest.iter().position(|&byte| byte == b' ')?;
        rest[end..]
            .starts_with(b" && ")
            .then(|| String::from_utf8_lossy(&rest[..end]).into_owned())
    })
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
/// This scans every process, which is cheap in practice. A full miss — the worst
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
    earliest_start: Option<u64>,
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
    ///
    /// Gone alone doesn't make a waiting window this launch's: a command that
    /// fails and exits (a bad flag, a crash) looks the same as one handing
    /// its request over. `predates_launch()` tells the two apart (#124).
    pub(super) fn is_gone(&self) -> bool {
        self.gone
    }

    /// Records the start time (`process_start_ticks()`) of a process seen
    /// carrying the marker. The earliest one stands for the launch's start.
    pub(super) fn record_start(&mut self, ticks: u64) {
        self.earliest_start = Some(self.earliest_start.map_or(ticks, |known| known.min(ticks)));
    }

    /// Whether a window's process, started at `window_start`, was already
    /// running before any process carrying the marker: what a single-instance
    /// hand-off looks like, since the window then belongs to an instance that
    /// predates the launch. A process started later carries no marker and
    /// doesn't descend from the launch (or it would have been confirmed), so
    /// it isn't this launch's, whatever the launched command did (#124).
    /// Unknown start times, and a tie within one clock tick, answer `false`.
    pub(super) fn predates_launch(&self, window_start: Option<u64>) -> bool {
        matches!((window_start, self.earliest_start), (Some(window), Some(launch)) if window < launch)
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
