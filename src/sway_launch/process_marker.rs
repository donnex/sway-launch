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
/// application) case, which resolves via `any_process_has_env_var()`
/// well before this cap in practice.
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

/// Whether `/proc/<pid>/environ` contains exactly `<var_name>=<expected_value>`
/// as one of its NUL-separated entries. Returns `false` for any I/O error
/// (pid already gone, no permission, `/proc` unavailable) rather than
/// erroring — this is a best-effort correlation signal for
/// `run_wait_matching_exec_event()`, never a hard requirement.
pub(super) fn process_has_env_var(pid: i32, var_name: &str, expected_value: &str) -> bool {
    let Ok(environ) = std::fs::read(format!("/proc/{}/environ", pid)) else {
        return false;
    };
    let needle = format!("{}={}", var_name, expected_value);
    environ
        .split(|&byte| byte == 0)
        .any(|entry| entry == needle.as_bytes())
}

/// Whether any currently-running process still carries `<var_name>=<expected_value>`
/// in its environment. Used by `run_wait_matching_exec_event()` to tell
/// whether the command it spawned (or a descendant that inherited the
/// marker) might still be about to create the matching window, versus
/// having already exited — e.g. a single-instance application that forwards
/// a request to an already-running instance and exits immediately, with no
/// further marker-confirmed match ever coming.
///
/// This scans every process, which looks alarming and has been raised as a
/// performance concern; measured before acting on it. A full miss — the worst
/// case, where every `/proc/<pid>/environ` is read — costs 0.5ms on a 26-process
/// system and 16ms on a 525-process one. It runs only in the already-ambiguous
/// path, at most a handful of times per invocation (the "gone" answer is cached
/// by the caller), inside a window that already budgets
/// `PID_MARKER_FALLBACK_GRACE` (2s) for exactly this decision. Even
/// extrapolated to a few thousand processes it stays a few percent of that
/// budget.
///
/// Scanning is also not an implementation shortcut that a narrower lookup could
/// replace: `sway-launch` never spawns the process. Sway does, via
/// `exec env <marker> <command>`, so there is no child pid to inspect — which
/// is the entire reason the environment marker exists. A "just look at the pid
/// we spawned" design has nothing to look at.
///
/// Raised a second time (2026-09-02) with a correctness rather than performance
/// framing: on a restricted `/proc` an unreadable process is indistinguishable
/// from one without the marker, so confirmation degrades into the content-match
/// fallback. True, and deliberate — the fallback exists precisely so an
/// unconfirmable case still resolves rather than timing out. What it must not
/// do is quietly *claim* the window: the degraded path yields
/// `LaunchOwnership::Adopted`, which keeps `--rollback-on-error` from killing
/// it, and the `read_dir` failure below is logged under `--verbose`. Same
/// conclusion as the first raise: no change.
pub(super) fn any_process_has_env_var(var_name: &str, expected_value: &str, verbose: bool) -> bool {
    // A per-process /proc/<pid>/environ read failing (the process already
    // exited) is expected and common, and needs no signal — but read_dir on
    // /proc itself failing is a different, environment-level condition
    // (restricted /proc, unusual containerization) worth surfacing to a
    // --verbose caller debugging a wrong-container-id report. Not unit-
    // tested: reliably making /proc unreadable from within a test isn't
    // portable/safe to simulate (it would mean altering process
    // permissions), so this branch is expected to stay uncovered.
    let entries = match std::fs::read_dir("/proc") {
        Ok(entries) => entries,
        Err(_) => {
            if verbose {
                eprintln!(
                    "Could not read /proc — PID-marker correlation is degraded on this system"
                );
            }
            return false;
        }
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .any(|pid| self::process_has_env_var(pid, var_name, expected_value))
}

#[cfg(test)]
mod tests {
    use super::*;

    // generate_pid_marker_token / process_has_env_var / any_process_has_env_var

    #[test]
    fn generate_pid_marker_token_starts_with_this_processes_id() {
        let token = generate_pid_marker_token();
        let expected_prefix = format!("{}-", std::process::id());
        assert!(
            token.starts_with(&expected_prefix),
            "token {:?} should start with {:?}",
            token,
            expected_prefix
        );
    }

    #[test]
    fn process_has_env_var_true_for_this_processes_own_environment() {
        let pid = std::process::id() as i32;
        let path = std::env::var("PATH").expect("PATH should be set in the test environment");
        assert!(process_has_env_var(pid, "PATH", &path));
    }

    #[test]
    fn process_has_env_var_false_for_wrong_value() {
        let pid = std::process::id() as i32;
        assert!(!process_has_env_var(
            pid,
            "PATH",
            "definitely-not-the-real-path-value"
        ));
    }

    #[test]
    fn process_has_env_var_false_for_nonexistent_pid() {
        assert!(!process_has_env_var(i32::MAX, "PATH", "anything"));
    }

    #[test]
    fn any_process_has_env_var_true_when_this_process_has_it() {
        let path = std::env::var("PATH").expect("PATH should be set in the test environment");
        assert!(any_process_has_env_var("PATH", &path, false));
    }

    #[test]
    fn any_process_has_env_var_false_for_a_value_nothing_has() {
        assert!(!any_process_has_env_var(
            "SWAY_LAUNCH_DEFINITELY_UNUSED_TEST_VAR_XYZ",
            "nope",
            false
        ));
    }
}
