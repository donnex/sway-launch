//! Talking to Sway: connections, command dispatch, and the event stream.
//!
//! This is the transport layer and nothing above it. It knows how to reach the
//! compositor, how long to wait for it, how to phrase a command safely and how
//! to report a failure — but nothing about what any particular command means.
//! Everything here needs a live compositor, which is why it is exempt from the
//! coverage target (see CLAUDE.md's Rust conventions); the exceptions are the
//! pure helpers `preferred_socket_path()`, `ipc_error()`,
//! `connect_error_at()`, `undiscovered_socket_error()`,
//! `first_outcome_error()` and `quote_sway_string()`, which are unit-tested.

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
/// Socket discovery is `socket_path()` (`SWAYSOCK`, then `I3SOCK`); when
/// neither is set it falls back to `Connection::new()`, whose remaining
/// discovery steps shell out to `i3 --get-socketpath` and then
/// `sway --get-socketpath`. That fallback is unbounded, which is accepted: it
/// only applies when the environment doesn't name a socket at all, and the
/// subprocess it runs is not the wedged compositor's IPC socket.
///
/// A connection that can't be made at all is reported through
/// `connect_error_at()`/`undiscovered_socket_error()`, which name Sway IPC,
/// the socket and where it came from: outside a Sway session (cron, a
/// systemd unit, ssh) the raw error is an OS message that never mentions Sway
/// — `No such file or directory (os error 2)`, or, with neither variable set,
/// `Invalid argument (os error 22)` from `swayipc` connecting to the empty
/// path a failed `sway --get-socketpath` printed.
pub(super) fn new_connection() -> Result<Connection, String> {
    match socket_path() {
        Some(socket) => connect_stream(&socket, IPC_ROUND_TRIP_TIMEOUT).map(Connection::from),
        None => Connection::new().map_err(undiscovered_socket_error),
    }
}

/// A socket path and the environment variable it was read from, so a
/// connection failure can say which one to check.
#[derive(Debug, PartialEq)]
struct SocketPath {
    path: std::path::PathBuf,
    variable: &'static str,
}

/// Connects to `socket` with `round_trip_timeout` applied to both directions.
fn connect_stream(
    socket: &SocketPath,
    round_trip_timeout: time::Duration,
) -> Result<std::os::unix::net::UnixStream, String> {
    let stream = std::os::unix::net::UnixStream::connect(&socket.path)
        .map_err(|error| connect_error_at(socket, &error))?;
    // Applied to both directions: a wedged compositor can stall a write
    // (socket buffer full, nothing draining it) as readily as a read.
    stream
        .set_read_timeout(Some(round_trip_timeout))
        .and_then(|()| stream.set_write_timeout(Some(round_trip_timeout)))
        .map_err(|error| format!("failed to set a Sway IPC socket timeout: {}", error))?;
    Ok(stream)
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

/// The message for a socket named by the environment that couldn't be
/// connected to: what was being reached, at which path, from which variable,
/// and the likeliest cause.
fn connect_error_at(socket: &SocketPath, error: &dyn fmt::Display) -> String {
    format!(
        "cannot reach Sway IPC at {} (from {}): {} — is this running inside a Sway session?",
        socket.path.display(),
        socket.variable,
        error
    )
}

/// The message for a connection attempted with neither `SWAYSOCK` nor
/// `I3SOCK` set, after `swayipc`'s own `--get-socketpath` discovery failed.
/// `swayipc`'s error is kept at the end for completeness, but on its own it's
/// typically `Invalid argument (os error 22)` and says nothing useful.
fn undiscovered_socket_error(error: swayipc::Error) -> String {
    format!(
        "cannot reach Sway IPC: neither SWAYSOCK nor I3SOCK is set, and no running Sway could be \
         found — run sway-launch from inside a Sway session, or export SWAYSOCK ({})",
        ipc_error(error)
    )
}

/// The Sway IPC socket path from the environment. `None` when neither
/// variable is set, which is what sends `new_connection()` and `event_loop()`
/// down their unbounded fallback.
fn socket_path() -> Option<SocketPath> {
    preferred_socket_path(std::env::var_os("SWAYSOCK"), std::env::var_os("I3SOCK"))
}

/// `SWAYSOCK` wins over `I3SOCK`, matching `swaymsg` rather than `swayipc`'s
/// own `Connection::new()` (which reads `I3SOCK` first).
///
/// Sway exports both variables, pointing at the same socket, to everything it
/// spawns, so the documented way to aim a Sway tool at a different compositor
/// is to override `SWAYSOCK` alone. With `I3SOCK` first, that override was
/// silently ignored and this tool kept driving the session the terminal
/// belonged to — including `scripts/run-live-sway-tests`, which points
/// `SWAYSOCK` at a throwaway headless compositor.
fn preferred_socket_path(
    swaysock: Option<std::ffi::OsString>,
    i3sock: Option<std::ffi::OsString>,
) -> Option<SocketPath> {
    let (value, variable) = match (swaysock, i3sock) {
        (Some(value), _) => (value, "SWAYSOCK"),
        (None, Some(value)) => (value, "I3SOCK"),
        (None, None) => return None,
    };
    Some(SocketPath {
        path: std::path::PathBuf::from(value),
        variable,
    })
}

/// Opens a subscription whose `subscribe()` handshake is bounded by
/// `IPC_ROUND_TRIP_TIMEOUT`, and whose event reads afterwards are not.
///
/// `subscribe()` is a request/response exchange like any other, run on the
/// calling thread, so a compositor that accepts a connection and then stops
/// answering would block it forever: every launch subscribes before making
/// any other IPC call, so `sway-launch --timeout 2 foot` against such a socket
/// would never return (`--timeout` only starts counting once the subscription
/// exists). The handshake therefore runs with the same socket timeout
/// `new_connection()` uses.
///
/// The event reads that follow must *not* keep it: a subscription's whole
/// purpose is to block until an event arrives, so a read timeout would surface
/// a perfectly normal quiet period as an error. They don't need one either —
/// the blocking read happens on a reader thread whose output the caller
/// collects with `recv_timeout()`, so the invocation is already bounded by
/// `--timeout` no matter what the socket does. So once `subscribe()` has
/// answered, the timeout is cleared through a `try_clone()` of the stream: the
/// clone shares the original's open socket, and socket options belong to the
/// socket rather than to a descriptor. Only the reader thread can be left
/// blocked, which is the bounded, measured behaviour documented at
/// `run_wait_matching_events()`'s `thread::spawn`.
///
/// It goes through the same `socket_path()` as `new_connection()`, so the
/// subscription and the requests around it always reach the same compositor,
/// and shares its unbounded fallback when the environment names no socket.
pub(super) fn event_loop(subscriptions: &[EventType]) -> Result<EventStream, String> {
    let Some(socket) = socket_path() else {
        return Connection::new()
            .map_err(undiscovered_socket_error)?
            .subscribe(subscriptions)
            .map_err(ipc_error);
    };

    let stream = connect_stream(&socket, IPC_ROUND_TRIP_TIMEOUT)?;
    let handle = stream
        .try_clone()
        .map_err(|error| format!("failed to clone the Sway IPC socket: {}", error))?;
    let events = Connection::from(stream)
        .subscribe(subscriptions)
        .map_err(ipc_error)?;
    handle
        .set_read_timeout(None)
        .map_err(|error| format!("failed to clear a Sway IPC socket timeout: {}", error))?;
    Ok(events)
}

pub(super) fn run_sway_command(command: &str) -> Result<(), String> {
    let outcomes = match new_connection()?.run_command(command) {
        Ok(outcomes) => outcomes,
        Err(error) => return Err(ipc_error(error)),
    };

    first_outcome_error(outcomes, command)
}

/// `run_sway_command()` for a command that must reach Sway as exactly one
/// command — the launch's `exec`. `validate_exec_command()` already rejects
/// anything Sway would split, so this is the backstop: if Sway's reply still
/// carries more than one outcome, the text was split after all, and the
/// launch is reported as failed rather than as whatever its first piece did.
pub(super) fn run_single_sway_command(command: &str) -> Result<(), String> {
    let outcomes = match new_connection()?.run_command(command) {
        Ok(outcomes) => outcomes,
        Err(error) => return Err(ipc_error(error)),
    };

    single_outcome_error(outcomes, command)
}

/// The pure half of `run_single_sway_command()`: more than one outcome is an
/// error of its own, naming the split, ahead of any per-outcome failure.
pub(super) fn single_outcome_error<E: fmt::Display>(
    outcomes: Vec<Result<(), E>>,
    command: &str,
) -> Result<(), String> {
    if outcomes.len() > 1 {
        return Err(format!(
            "Sway split {:?} into {} separate commands; only the first was meant to run",
            command,
            outcomes.len()
        ));
    }

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
mod tests;
