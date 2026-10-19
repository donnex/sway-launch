//! `--continue-on-timeout`'s watch for a timed-out launch's window (#181).
//!
//! A slow application usually isn't a missing one: its command keeps running
//! after its step gives up, and its window maps a few seconds later, next to
//! whatever a later step just placed, taking focus. `LateWindowWatch` keeps
//! looking for a window carrying such a launch's marker for the rest of the
//! run, on an event subscription of its own plus a tree scan whenever the
//! run asks, and moves each one it finds to the scratchpad, out of the
//! shape being built, for `run_steps()` to report and roll back.

use super::ipc::{event_loop, run_single_sway_command, spawn_event_forwarder};
use super::process_marker::{abandon_launch, launch_evidence};
use super::query::window_processes_now;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::{thread, time};
use swayipc::{Event, EventType, WindowChange};

/// How often the watch's thread wakes to see whether the run is over.
const STOP_CHECK_INTERVAL: time::Duration = time::Duration::from_millis(50);

/// A window a timed-out launch opened after its step gave up, and what
/// moving it to the scratchpad came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LateWindow {
    /// The timed-out step's 0-based index.
    pub step: usize,
    pub container_id: i64,
    /// Why the window couldn't be moved to the scratchpad, if it couldn't:
    /// it's still caught, reported and rolled back either way.
    pub move_error: Option<String>,
}

/// A timed-out launch being watched: its step, its marker token, and the
/// windows it had already opened by the timeout, which its step reported
/// as `unmatched_windows` and which stay where they are.
#[derive(Debug, Clone)]
struct WatchedLaunch {
    step: usize,
    token: String,
    already_reported: Vec<i64>,
}

#[derive(Debug, Default)]
struct WatchState {
    launches: Vec<WatchedLaunch>,
    caught: Vec<LateWindow>,
    /// How many of `caught` `take_new()` has handed out.
    reported: usize,
}

/// See the module doc. Nothing runs until the first `watch()`, so a run
/// whose steps all open their windows in time never subscribes to anything.
#[derive(Debug, Default)]
pub struct LateWindowWatch {
    state: Arc<Mutex<WatchState>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl LateWindowWatch {
    /// Starts watching for the window of step `step`'s timed-out launch,
    /// whose marker is `token`; `already_reported` are the windows it
    /// opened before the timeout. Later launches stop treating a window
    /// carrying `token` as a fallback candidate from here on. Errs when the
    /// watch can't subscribe or read the tree, which leaves the launch
    /// unwatched but changes nothing else.
    pub fn watch(
        &mut self,
        step: usize,
        token: &str,
        already_reported: Vec<i64>,
    ) -> Result<(), String> {
        abandon_launch(token);
        lock(&self.state).launches.push(WatchedLaunch {
            step,
            token: token.to_string(),
            already_reported,
        });
        if self.thread.is_none() {
            let events = spawn_event_forwarder(event_loop(&[EventType::Window])?);
            let state = Arc::clone(&self.state);
            let stop = Arc::clone(&self.stop);
            self.thread = Some(thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match events.recv_timeout(STOP_CHECK_INTERVAL) {
                        Ok(Ok(Event::Window(window))) if window.change == WindowChange::New => {
                            if let Some(pid) = window.container.pid {
                                catch(&state, window.container.id, pid);
                            }
                        }
                        Ok(Ok(_)) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            }));
        }
        // The subscription above only sees windows from now on; one that
        // mapped between the step's timeout and here is already in the tree.
        self.scan()
    }

    /// Every window caught since the last call, after one more tree scan,
    /// so a window whose event the watch's thread hasn't handled yet isn't
    /// missed. Empty, without reading anything, while nothing is watched.
    pub fn take_new(&mut self) -> Vec<LateWindow> {
        if lock(&self.state).launches.is_empty() {
            return Vec::new();
        }
        // A tree that can't be read leaves the thread's catches to report.
        let _ = self.scan();
        let mut state = lock(&self.state);
        let new = state.caught[state.reported..].to_vec();
        state.reported = state.caught.len();
        new
    }

    /// Ends the watch: stops its thread, then hands out what `take_new()`
    /// would.
    pub fn finish(mut self) -> Vec<LateWindow> {
        self.stop_thread();
        self.take_new()
    }

    fn scan(&self) -> Result<(), String> {
        for (container_id, pid) in window_processes_now()? {
            catch(&self.state, container_id, pid);
        }
        Ok(())
    }

    fn stop_thread(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for LateWindowWatch {
    fn drop(&mut self) {
        self.stop_thread();
    }
}

fn lock(state: &Mutex<WatchState>) -> MutexGuard<'_, WatchState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Moves `container_id` to the scratchpad and records it, when its process
/// (`pid`) carries a watched launch's marker. Held under the lock
/// throughout, so the thread and a scan can't both catch one window.
fn catch(state: &Mutex<WatchState>, container_id: i64, pid: i32) {
    let mut state = lock(state);
    let Some(step) = watched_step(&state.launches, &state.caught, container_id, |token| {
        launch_evidence(pid, token).is_some()
    }) else {
        return;
    };
    let move_error =
        run_single_sway_command(&format!("[con_id={}] move scratchpad", container_id)).err();
    state.caught.push(LateWindow {
        step,
        container_id,
        move_error,
    });
}

/// Which watched launch `container_id` is a late window of, if any: the
/// first whose marker `carries` says its process has, unless the window is
/// already caught or was reported by its step at the timeout. Pure but for
/// `carries`, which reads `/proc` in `catch()`.
fn watched_step(
    launches: &[WatchedLaunch],
    caught: &[LateWindow],
    container_id: i64,
    mut carries: impl FnMut(&str) -> bool,
) -> Option<usize> {
    if caught
        .iter()
        .any(|window| window.container_id == container_id)
    {
        return None;
    }
    launches
        .iter()
        .find(|launch| !launch.already_reported.contains(&container_id) && carries(&launch.token))
        .map(|launch| launch.step)
}

#[cfg(test)]
mod tests;
