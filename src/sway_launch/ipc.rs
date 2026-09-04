//! Talking to Sway: connections, command dispatch, and the event stream.
//!
//! This is the transport layer and nothing above it. It knows how to reach the
//! compositor, how long to wait for it, how to phrase a command safely and how
//! to report a failure — but nothing about what any particular command means.
//! Everything here needs a live compositor, which is why it is exempt from the
//! coverage target (see CLAUDE.md's Rust conventions); the exceptions are the
//! three pure helpers at the bottom, which are unit-tested.

use std::{fmt, time};
use swayipc::{Connection, EventStream, EventType};

/// How long a single Sway IPC request/response round trip may take before the
/// socket read or write gives up (see `new_connection()`).
///
/// Deliberately *not* `--timeout`, and not derived from it. The two bound
/// different things: `--timeout` is how long to wait for a window *event*,
/// which legitimately takes seconds because it's waiting on an application to
/// map a window. This bounds one request/response exchange with the compositor,
/// which on a healthy system is sub-millisecond regardless of `--timeout`.
/// Tying them together would mean `--timeout 1` making ordinary tree reads
/// fail on a merely-slow machine, and `--timeout 60` re-opening a minute-long
/// hang.
///
/// 10s is therefore far above any legitimate round trip while still turning a
/// wedged compositor into a prompt, clear failure instead of an indefinite
/// block.
pub(super) const IPC_ROUND_TRIP_TIMEOUT: time::Duration = time::Duration::from_secs(10);

/// A request/response Sway IPC connection whose reads and writes are bounded
/// by `IPC_ROUND_TRIP_TIMEOUT`, so a compositor that accepts a connection and
/// then stops answering fails instead of blocking forever.
///
/// Without this, `--timeout` bounded only the wait for a confirmation *event*,
/// not the IPC round trips around it — every `get_tree()`, `get_outputs()` and
/// `run_command()` was an unbounded blocking read. Confirmed by pointing
/// `SWAYSOCK` at a socket that accepts and never replies:
/// `sway-launch --con-id 42 --floating --timeout 2` hung until killed
/// externally at 15s, because the first thing it does is read the tree.
///
/// `swayipc`'s own `Connection::new()` gives no way to configure the socket,
/// but it does expose `From<UnixStream>`, so the stream is built here instead.
/// Socket discovery is `I3SOCK`/`SWAYSOCK`, matching `swayipc`'s own order;
/// when neither is set it falls back to `Connection::new()`, whose remaining
/// discovery step shells out to `sway --get-socketpath`. That fallback is
/// unbounded, which is accepted: it only applies when the environment doesn't
/// name a socket at all, and the subprocess it runs is not the wedged
/// compositor's IPC socket.
pub(super) fn new_connection() -> Result<Connection, String> {
    if let Some(path) = socket_path() {
        let stream = std::os::unix::net::UnixStream::connect(path)
            .map_err(|error| format!("failed to connect to the Sway IPC socket: {}", error))?;
        // Applied to both directions: a wedged compositor can stall a write
        // (socket buffer full, nothing draining it) as readily as a read.
        stream
            .set_read_timeout(Some(IPC_ROUND_TRIP_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(IPC_ROUND_TRIP_TIMEOUT)))
            .map_err(|error| format!("failed to set a Sway IPC socket timeout: {}", error))?;
        return Ok(Connection::from(stream));
    }

    match Connection::new() {
        Ok(connection) => Ok(connection),
        Err(error) => Err(error.to_string()),
    }
}

/// Renders a `swayipc` error, turning the socket-timeout case into something
/// that names the actual problem.
///
/// A read or write that hits `IPC_ROUND_TRIP_TIMEOUT` surfaces as a bare
/// `Resource temporarily unavailable (os error 11)`, which tells a user
/// nothing about what happened or which knob (if any) relates to it. Every
/// other error keeps `swayipc`'s own wording, which is already specific —
/// `command failed with 'No matching node.'` and friends.
pub(super) fn ipc_error(error: swayipc::Error) -> String {
    if let swayipc::Error::Io(io_error) = &error {
        if matches!(
            io_error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            return format!(
                "Sway IPC did not respond within {} sec — the compositor accepted the connection \
                 but stopped answering (this bounds one request/response exchange, and is \
                 separate from --timeout)",
                IPC_ROUND_TRIP_TIMEOUT.as_secs()
            );
        }
    }
    error.to_string()
}

/// The Sway IPC socket path from the environment, in `swayipc`'s own
/// precedence order. `None` when neither variable is set, which is what sends
/// `new_connection()` down its unbounded fallback.
fn socket_path() -> Option<std::path::PathBuf> {
    std::env::var_os("I3SOCK")
        .or_else(|| std::env::var_os("SWAYSOCK"))
        .map(std::path::PathBuf::from)
}

/// The connection `event_loop()` subscribes on, deliberately *without* a read
/// timeout.
///
/// A subscription's whole purpose is to block until an event arrives, so a
/// socket read timeout would surface a perfectly normal quiet period as an
/// error. It doesn't need one either: the blocking read happens on a reader
/// thread whose output the caller collects with `recv_timeout()`, so the
/// invocation is already bounded by `--timeout` no matter what the socket
/// does. Only the reader thread can be left blocked, which is the bounded,
/// measured behaviour documented at `run_wait_matching_events()`'s
/// `thread::spawn`.
pub(super) fn new_event_connection() -> Result<Connection, String> {
    match Connection::new() {
        Ok(connection) => Ok(connection),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn event_loop(subscriptions: &[EventType]) -> Result<EventStream, String> {
    match new_event_connection()?.subscribe(subscriptions) {
        Ok(event_iterator) => Ok(event_iterator),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn run_sway_command(command: &str) -> Result<(), String> {
    let outcomes = match new_connection()?.run_command(command) {
        Ok(outcomes) => outcomes,
        Err(error) => return Err(ipc_error(error)),
    };

    first_outcome_error(outcomes, command)
}

/// Kills `container_id` via `[con_id] kill`. Used by `main.rs`'s
/// `--rollback-on-error`: best-effort cleanup of a window this invocation
/// itself launched earlier in the same `--layout`/`--template` run, once a
/// later step fails.
pub fn kill_container(container_id: i64) -> Result<(), String> {
    run_sway_command(&format!("[con_id={}] kill", container_id))
}

/// Sway splits a command string into multiple sub-commands on unquoted
/// `,`/`;`, so `run_command()` can return more than one outcome for a single
/// call. Report the first failure found among all of them, rather than only
/// the first outcome — an early success must not hide a later failure.
pub(super) fn first_outcome_error<E: fmt::Display>(
    outcomes: Vec<Result<(), E>>,
    command: &str,
) -> Result<(), String> {
    // Every SwayAction::sway_command() builds a non-empty string, and
    // swayipc always returns at least one outcome for one, so this branch
    // isn't known to be reachable in practice — it's defensive against a
    // theoretical empty reply rather than a case this crate can construct
    // a test for without mocking swayipc.
    if outcomes.is_empty() {
        return Err(format!("{} command failed", command));
    }

    for outcome in outcomes {
        if let Err(error) = outcome {
            return Err(error.to_string());
        }
    }

    Ok(())
}

/// Quotes a value for safe interpolation into a Sway IPC command string.
/// Sway's command parser splits on `,`/`;` and whitespace outside quotes, so
/// an unquoted value containing one of those could inject additional
/// commands; wrapping it in escaped double quotes forces it to be read back
/// as a single literal argument.
pub(super) fn quote_sway_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    // quote_sway_string

    #[test]
    fn quote_sway_string_wraps_plain_value() {
        assert_eq!(quote_sway_string("foo"), "\"foo\"");
    }

    #[test]
    fn quote_sway_string_escapes_embedded_quotes() {
        assert_eq!(quote_sway_string("foo\"bar"), "\"foo\\\"bar\"");
    }

    #[test]
    fn quote_sway_string_escapes_backslashes() {
        assert_eq!(quote_sway_string("foo\\bar"), "\"foo\\\\bar\"");
    }

    #[test]
    fn quote_sway_string_wraps_a_value_containing_a_newline() {
        // Regression test: confirmed live (this project's security review,
        // 2026-08-21) that a literal newline embedded in a --mark value
        // can't break out of the quoting either -- Sway's own parser treats
        // it as part of the quoted literal, not a command separator, the
        // same as the comma/semicolon case below. quote_sway_string()
        // itself needs no special handling for `\n` (only `\`/`"` are
        // escaped) since it's neither of those; this test just pins that
        // the newline survives untouched inside the quotes rather than
        // being stripped or otherwise mishandled. See
        // mark_with_special_characters_is_stored_literally_not_executed in
        // tests/live_sway.rs for the live-Sway proof this is actually safe.
        let injected = "foo\nexec malicious-command";
        let quoted = quote_sway_string(injected);
        assert_eq!(quoted, "\"foo\nexec malicious-command\"");
    }

    #[test]
    fn quote_sway_string_neutralizes_command_separators() {
        // Regression test: an unquoted mark containing a command separator
        // used to let extra Sway commands be injected into the same call.
        let injected = "foo, exec malicious-command";
        let quoted = quote_sway_string(injected);
        assert_eq!(quoted, "\"foo, exec malicious-command\"");
        assert!(!quoted.trim_matches('"').contains('"'));
    }

    // ipc_error

    #[test]
    fn ipc_error_explains_a_socket_timeout() {
        // A read that hits IPC_ROUND_TRIP_TIMEOUT arrives as a bare
        // "Resource temporarily unavailable (os error 11)", which says nothing
        // about what happened. Both kinds are mapped: Linux reports a timed-out
        // socket read as WouldBlock, other platforms as TimedOut.
        for kind in [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut] {
            let error = ipc_error(swayipc::Error::Io(std::io::Error::from(kind)));
            assert!(
                error.contains("did not respond") && error.contains("10 sec"),
                "should name the condition and the bound, got {error:?}"
            );
            assert!(
                error.contains("--timeout"),
                "should say which knob this is *not*, got {error:?}"
            );
        }
    }

    #[test]
    fn ipc_error_leaves_other_errors_alone() {
        // swayipc's own wording is already specific for these; rewriting them
        // would lose the compositor's actual complaint.
        let error = ipc_error(swayipc::Error::CommandFailed(
            "No matching node.".to_string(),
        ));
        assert!(
            error.contains("No matching node."),
            "should keep swayipc's own message, got {error:?}"
        );
        assert!(!error.contains("did not respond"));
    }

    #[test]
    fn ipc_error_leaves_a_non_timeout_io_error_alone() {
        let error = ipc_error(swayipc::Error::Io(std::io::Error::from(
            std::io::ErrorKind::ConnectionRefused,
        )));
        assert!(!error.contains("did not respond"), "got {error:?}");
    }

    // first_outcome_error

    #[test]
    fn first_outcome_error_ok_when_all_succeed() {
        let outcomes: Vec<Result<(), String>> = vec![Ok(()), Ok(())];
        assert_eq!(first_outcome_error(outcomes, "cmd"), Ok(()));
    }

    #[test]
    fn first_outcome_error_fails_when_empty() {
        let outcomes: Vec<Result<(), String>> = vec![];
        assert_eq!(
            first_outcome_error(outcomes, "cmd"),
            Err("cmd command failed".to_string())
        );
    }

    #[test]
    fn first_outcome_error_surfaces_a_leading_failure() {
        let outcomes: Vec<Result<(), String>> = vec![Err("boom".to_string()), Ok(())];
        assert_eq!(
            first_outcome_error(outcomes, "cmd"),
            Err("boom".to_string())
        );
    }

    #[test]
    fn first_outcome_error_surfaces_a_trailing_failure() {
        // Regression test: a prior version only inspected the first outcome
        // and returned Ok(()) here, silently dropping this failure.
        let outcomes: Vec<Result<(), String>> = vec![Ok(()), Err("boom".to_string())];
        assert_eq!(
            first_outcome_error(outcomes, "cmd"),
            Err("boom".to_string())
        );
    }
}
