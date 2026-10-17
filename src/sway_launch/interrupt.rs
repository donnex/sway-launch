//! SIGINT/SIGTERM during a `--layout`/`--template` run (#15).
//!
//! Left to the default disposition, either signal killed the process on the
//! spot: every window launched so far stayed open, `--rollback-on-error` never
//! ran, and `--json` reported nothing. `install()` turns a signal into a flag
//! instead, and every wait that can last long checks it at least every
//! `CHECK_INTERVAL`, so the run fails through its ordinary error path, with
//! rollback and partial progress, a moment later.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::{io, thread, time};

/// The longest a received signal goes unnoticed by a wait.
const CHECK_INTERVAL: time::Duration = time::Duration::from_millis(50);

struct Received {
    any: Arc<AtomicBool>,
    signal: Arc<AtomicUsize>,
}

static RECEIVED: OnceLock<Received> = OnceLock::new();

/// Catches SIGINT and SIGTERM from here on. A second one, of either kind,
/// exits at once with the usual `128 + signal` status, skipping cleanup, so a
/// rollback that is itself stuck can still be cut short.
pub fn install() -> io::Result<()> {
    let received = RECEIVED.get_or_init(|| Received {
        any: Arc::new(AtomicBool::new(false)),
        signal: Arc::new(AtomicUsize::new(0)),
    });
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        // Registered first, so it runs before this signal sets `any` and
        // only sees an earlier one.
        signal_hook::flag::register_conditional_shutdown(
            signal,
            128 + signal,
            Arc::clone(&received.any),
        )?;
        signal_hook::flag::register(signal, Arc::clone(&received.any))?;
        signal_hook::flag::register_usize(signal, Arc::clone(&received.signal), signal as usize)?;
    }
    Ok(())
}

/// The signal received since `install()`, if any.
pub fn received() -> Option<i32> {
    let received = RECEIVED.get()?;
    match received.signal.load(Ordering::SeqCst) {
        0 => None,
        signal => i32::try_from(signal).ok(),
    }
}

/// The exit status a run interrupted by a signal ends with: `128 + signal`,
/// as a shell reports a process the signal killed.
pub fn exit_status() -> Option<i32> {
    received().map(|signal| 128 + signal)
}

pub(super) fn signal_name(signal: i32) -> &'static str {
    match signal {
        signal_hook::consts::SIGINT => "SIGINT",
        signal_hook::consts::SIGTERM => "SIGTERM",
        _ => "a signal",
    }
}

/// `Err` with the error a run reports once interrupted.
pub fn check() -> Result<(), String> {
    match received() {
        Some(signal) => Err(format!("interrupted by {}", signal_name(signal))),
        None => Ok(()),
    }
}

/// `thread::sleep()`, cut short by a signal.
pub(super) fn sleep(duration: time::Duration) -> Result<(), String> {
    let end = time::Instant::now() + duration;
    loop {
        check()?;
        let left = end.saturating_duration_since(time::Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        thread::sleep(left.min(CHECK_INTERVAL));
    }
}

/// Why `recv()` returned without a value.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum WaitEnd {
    Timeout,
    Disconnected,
    Interrupted(String),
}

/// `Receiver::recv_timeout()`, cut short by a signal.
pub(super) fn recv<T>(receiver: &mpsc::Receiver<T>, timeout: time::Duration) -> Result<T, WaitEnd> {
    let end = time::Instant::now() + timeout;
    loop {
        check().map_err(WaitEnd::Interrupted)?;
        let left = end.saturating_duration_since(time::Instant::now());
        match receiver.recv_timeout(left.min(CHECK_INTERVAL)) {
            Ok(value) => return Ok(value),
            Err(mpsc::RecvTimeoutError::Timeout) if time::Instant::now() >= end => {
                return Err(WaitEnd::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(WaitEnd::Disconnected),
        }
    }
}

#[cfg(test)]
mod tests;
