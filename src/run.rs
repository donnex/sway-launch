//! The `--layout`/`--template` run: `run_steps()`'s per-step loop, the
//! `--concurrent` staging, the run lock and `--rollback-on-error`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::{process, time};

use crate::{
    check_compositor_version, dry_run_step, exit_on_write_failure_with_status, fail,
    fail_with_rollback, input_label, is_stdin, join_ids, layout, print_dry_run_steps, print_line,
    print_line_deferring_failure, stderr_label, step_action_records_json, sway_launch,
    unmatched_windows_json, write_line, Args, DryRunStep, PartialProgress, RollbackOutcome,
    Severity,
};

/// Runs every step sequentially, in the order they appear, stopping at the
/// first error — the same `set -eu`-chained behavior as the shell-script
/// examples `--layout`/`--template` are meant to replace. Exits the process
/// directly (success or failure) rather than returning, matching the rest
/// of main()'s error-handling style. Shared by `run_layout()` and
/// `run_template()`, since a resolved template is just more layout steps.
pub(crate) fn run_steps(
    steps: &[layout::LayoutStep],
    build: sway_launch::BuildLocation,
    concurrent: bool,
    lock_source: &str,
    args: &Args,
) -> ! {
    // Checked before the --dry-run/--validate branches so all three modes
    // agree. A file with no steps parses fine (every field of a layout is
    // optional), so without this it "succeeded" silently: exit 0, no output,
    // nothing launched — indistinguishable from a run that worked, and the
    // likely causes (steps lost to a bad merge, or commented out) are
    // exactly the ones worth surfacing.
    if steps.is_empty() {
        fail(
            args.json,
            &format!("no steps found in {}", steps_source(args)),
        );
    }

    if args.dry_run {
        run_steps_dry_run(steps, build, args);
    }
    if args.validate {
        run_steps_validate(steps, args);
    }

    // The whole file is checked before anything opens (#179): a mistake in
    // a later step would otherwise surface only after the earlier steps'
    // windows were open, and --validate would reject what a real run had
    // half-built. The per-step plan_step() below still runs, since it
    // resolves target_id against real container ids.
    if let Err(error) = plan_steps_offline(steps, args) {
        fail(args.json, &error);
    }

    // Held until the process exits, so a second run of the same layout on
    // the same workspace can't interleave its windows with this one's, and a
    // launching run of any other layout in this Sway session waits for this
    // one rather than taking focus from it (#156).
    let launches = steps.iter().any(|step| step.command.is_some());
    let _run_lock = match acquire_run_lock(build, launches, lock_source, args) {
        Ok(lock) => lock,
        Err(error) => fail(args.json, &format!("{}: {}", steps_source(args), error)),
    };
    let _session_lock = match acquire_session_lock(launches, args) {
        Ok(lock) => lock,
        Err(error) => fail(args.json, &format!("{}: {}", steps_source(args), error)),
    };

    // Asked once, before whatever first talks to the compositor: here when
    // the run launches or moves focus, else before the first step that
    // needs it (a run of bare con_id steps never does).
    let mut version_checked = launches || !build.commands().is_empty();
    if version_checked {
        check_compositor_version(args.verbose);
    }

    if let Err(error) = build.apply(args.verbose) {
        fail(args.json, &format!("{}: {}", steps_source(args), error));
    }
    // Recorded once, here, and every launch checked against it (#156): focus
    // is one value for the whole session, so a step can't trust it to still
    // be where this run put it.
    let build_workspace = if launches {
        match build.anchor(args.verbose) {
            Ok(build_workspace) => Some(build_workspace),
            Err(error) => fail(args.json, &format!("{}: {}", steps_source(args), error)),
        }
    } else {
        None
    };
    // Only a run that launches windows builds a shape; one that only
    // retargets existing windows never needs to read the tree here.
    let mut warnings = if launches {
        occupied_workspace_warning(args)
    } else {
        Vec::new()
    };

    let mut progress = RunProgress::default();
    // Every action across every step, each tagged with its 1-based step
    // number (steps.len() can be several, unlike a single direct-CLI
    // invocation) — see step_action_records_json()'s doc comment.
    let mut actions = Vec::new();
    let mut stdout_failure = None;
    // --continue-on-timeout's record: the steps whose window never appeared,
    // the ids they would have resolved, and the later steps skipped because
    // they target one of those.
    let mut timed_out_steps = Vec::new();
    let mut missing_ids = HashSet::new();
    let mut skipped_steps = Vec::new();
    // A timed-out launch's window that opens later is moved out of the
    // layout's way rather than left to join it (#181).
    let mut late_windows = sway_launch::LateWindowWatch::default();
    let mut late_container_ids = Vec::new();

    // From here on windows get launched, so a SIGINT/SIGTERM/SIGHUP fails the run
    // through fail_step(), with rollback and partial progress, rather than
    // killing it outright (#15).
    if let Err(error) = sway_launch::interrupt::install() {
        diagnostic!(
            "{}couldn't catch SIGINT/SIGTERM/SIGHUP ({}); an interrupted run won't roll back",
            stderr_label(Severity::Warning),
            error
        );
    }

    let mut staged = match (&build_workspace, concurrent) {
        (Some(build_workspace), true) => Some(stage_launches(steps, build_workspace, args)),
        _ => None,
    };
    if let Some((index, error)) = staged
        .as_mut()
        .and_then(|staged| take_staging_failure(&mut staged.windows, args.continue_on_timeout))
    {
        // As a sequential launch's failure is: a window it caught late, or
        // one its filter rejected that carries its marker, is its progress,
        // brought back from the staging workspace and rolled back when it's
        // this run's.
        for container_id in progress.record_failed(&steps[index], &error) {
            if let Some(staged) = staged.as_ref() {
                bring_back(&staged.staging, container_id, args.verbose);
            }
        }
        progress.fail(&mut staged, steps, args, index, error);
    }

    for (index, step) in steps.iter().enumerate() {
        record_late_windows(
            late_windows.take_new(),
            args,
            &mut progress,
            &mut timed_out_steps,
            &mut late_container_ids,
        );
        if let Err(message) = sway_launch::interrupt::check() {
            progress.fail(
                &mut staged,
                steps,
                args,
                index,
                format!("{} before it started", message).into(),
            );
        }

        if let Some(target_id) = step
            .target_id
            .as_deref()
            .filter(|target_id| missing_ids.contains(*target_id))
        {
            diagnostic!(
                "{}{}",
                stderr_label(Severity::Warning),
                sway_launch::escape_control_characters(
                    &step_error(
                        args,
                        index,
                        &format!(
                            "skipped: target_id {:?} names a step whose window never appeared",
                            target_id
                        )
                    ),
                    true
                )
            );
            skipped_steps.push(serde_json::json!({ "step": index + 1, "target_id": target_id }));
            if let Some(id) = step.id.as_deref() {
                missing_ids.insert(id.to_string());
            }
            continue;
        }

        let sway_launch = match plan_step(step, args, &progress.resolved_ids) {
            Ok(sway_launch) => sway_launch::SwayLaunch {
                build_workspace: build_workspace.clone(),
                warn_on_misplaced_launch: step.command.is_some(),
                // A timeout that --continue-on-timeout carries on past is
                // watched for the rest of the run instead (LateWindowWatch).
                catch_late_window: args.rollback_on_error && !args.continue_on_timeout,
                excluded_containers: staged
                    .as_ref()
                    .map(|staged| staged_window_owners(&staged.windows, Some(index)))
                    .unwrap_or_default()
                    .into_keys()
                    .chain(late_container_ids.iter().copied())
                    .collect(),
                ..sway_launch
            },
            Err(error) => progress.fail(&mut staged, steps, args, index, error.into()),
        };
        if !version_checked && sway_launch.needs_compositor() {
            version_checked = true;
            check_compositor_version(args.verbose);
        }

        let staged_window = staged
            .as_mut()
            .and_then(|staged| staged.windows.remove(&index));
        let result = match (staged_window, staged.as_ref()) {
            (Some(window), Some(staged)) => {
                arrange_staged(&staged.staging, window, sway_launch, args.verbose)
            }
            (None, Some(staged)) => match sway_launch.target {
                sway_launch::Target::ConId(container_id) => {
                    match staged_window_owners(&staged.windows, None).get(&container_id) {
                        Some(owner) => Err(staged_window_taken_error(container_id, *owner)),
                        None => sway_launch.run(),
                    }
                }
                _ => sway_launch.run(),
            },
            _ => sway_launch.run(),
        };

        match result {
            Ok(outcome) => {
                if !args.json {
                    print_line_deferring_failure(
                        &outcome.container_id.to_string(),
                        &mut stdout_failure,
                    );
                }
                progress.record(step, outcome.container_id, outcome.launch_ownership);
                if outcome.launch_ownership != Some(sway_launch::LaunchOwnership::Launched)
                    && args.verbose
                    && step.command.is_some()
                {
                    diagnostic!(
                        "step {}: container {} was adopted rather than confirmed as launched by \
                         this run, so --rollback-on-error will leave it open",
                        index + 1,
                        outcome.container_id
                    );
                }
                for warning in &outcome.warnings {
                    let warning = step_error(args, index, warning);
                    if !args.json {
                        diagnostic!(
                            "{}{}",
                            stderr_label(Severity::Warning),
                            sway_launch::escape_control_characters(&warning, true)
                        );
                    }
                    warnings.push(warning);
                }
                actions.extend(step_action_records_json(index, outcome.actions));
            }
            Err(error) if args.continue_on_timeout && error.window_timed_out => {
                let message = step_error(args, index, &error.message);
                diagnostic!(
                    "{}{} (continuing: --continue-on-timeout)",
                    stderr_label(Severity::Warning),
                    sway_launch::escape_control_characters(&message, true)
                );
                let mut entry = serde_json::json!({ "step": index + 1, "error": message });
                if !error.unmatched_windows.is_empty() {
                    entry["unmatched_windows"] = unmatched_windows_json(&error.unmatched_windows);
                }
                timed_out_steps.push(entry);
                if let Some(id) = step.id.as_deref() {
                    missing_ids.insert(id.to_string());
                }
                // A --concurrent launch's late window opens on the staging
                // workspace instead, reported as an unresolved launch (#126).
                if let (None, Some(token)) = (&staged, error.launch_token.as_deref()) {
                    let already_reported = error
                        .unmatched_windows
                        .iter()
                        .map(|window| window.identity.container_id)
                        .collect();
                    if let Err(watch_error) = late_windows.watch(index, token, already_reported) {
                        diagnostic!(
                            "{}{}",
                            stderr_label(Severity::Warning),
                            sway_launch::escape_control_characters(
                                &step_error(
                                    args,
                                    index,
                                    &format!(
                                        "can't watch for its window opening later ({}), so it \
                                         may still open inside the layout",
                                        watch_error
                                    )
                                ),
                                true
                            )
                        );
                    }
                }
            }
            Err(error) => {
                // A step can launch its window and then fail on a later
                // action, so that window joins the earlier ones: reported as
                // progress, and rolled back when this run can prove it
                // launched it — see RunError's doc comment. Only a window the
                // step launched counts as progress: one it merely retargeted
                // may never have existed (a stale con_id is exactly such a
                // failure), so listing it among the open windows would assert
                // something this run never saw. error.container_id still
                // names it either way.
                progress.record_failed(step, &error);
                record_late_windows(
                    late_windows.finish(),
                    args,
                    &mut progress,
                    &mut timed_out_steps,
                    &mut late_container_ids,
                );
                progress.fail(&mut staged, steps, args, index, error)
            }
        }
    }

    record_late_windows(
        late_windows.finish(),
        args,
        &mut progress,
        &mut timed_out_steps,
        &mut late_container_ids,
    );

    // Each timed-out step was already reported as it happened; the status is
    // what tells a calling script the layout is incomplete, so a closed
    // stdout below exits with it too.
    let status = if timed_out_steps.is_empty() { 0 } else { 1 };
    let unresolved_launches = staged.map(|staged| staged.unresolved).unwrap_or_default();
    if !args.json {
        for launch in &unresolved_launches {
            diagnostic!(
                "{}{}",
                stderr_label(Severity::Warning),
                sway_launch::escape_control_characters(&launch.message(), true)
            );
        }
    }
    if args.json {
        let mut result = serde_json::json!({
            "container_ids": progress.container_ids,
            "containers": progress.resolved_ids,
            "actions": actions,
        });
        if !warnings.is_empty() {
            result["warnings"] = serde_json::json!(warnings);
        }
        if !timed_out_steps.is_empty() {
            result["timed_out_steps"] = serde_json::json!(timed_out_steps);
        }
        if !skipped_steps.is_empty() {
            result["skipped_steps"] = serde_json::json!(skipped_steps);
        }
        if !unresolved_launches.is_empty() {
            result["unresolved_launches"] = unresolved_launches
                .iter()
                .map(UnresolvedLaunch::json)
                .collect();
        }
        if let Err(error) = write_line(&result.to_string()) {
            exit_on_write_failure_with_status(&error, status);
        }
    }

    if let Some(error) = stdout_failure {
        exit_on_write_failure_with_status(&error, status);
    }
    process::exit(status);
}

/// Reports each window `LateWindowWatch` caught: a warning on stderr, its
/// step's `timed_out_steps` entry's `late_container_id` (the first, should
/// an application open several), excluded from later steps' `Existing`
/// resolution, and recorded for `--rollback-on-error`, since its process
/// carries this run's marker. It isn't part of the layout, so it stays out
/// of `container_ids`/`containers`.
fn record_late_windows(
    caught: Vec<sway_launch::LateWindow>,
    args: &Args,
    progress: &mut RunProgress,
    timed_out_steps: &mut [serde_json::Value],
    late_container_ids: &mut Vec<i64>,
) {
    for window in caught {
        let placement = match &window.move_error {
            None => "moved it to the scratchpad, out of the layout's way".to_string(),
            Some(error) => format!("couldn't move it to the scratchpad: {}", error),
        };
        diagnostic!(
            "{}{}",
            stderr_label(Severity::Warning),
            sway_launch::escape_control_characters(
                &step_error(
                    args,
                    window.step,
                    &format!(
                        "its window (con_id {}) opened after the step timed out; {}",
                        window.container_id, placement
                    )
                ),
                true
            )
        );
        if let Some(entry) = timed_out_steps
            .iter_mut()
            .find(|entry| entry["step"] == window.step + 1)
        {
            if entry.get("late_container_id").is_none() {
                entry["late_container_id"] = serde_json::json!(window.container_id);
            }
        }
        late_container_ids.push(window.container_id);
        progress.record_late(window.container_id);
    }
}

/// The layout half of a run lock's identity: the file by its canonical path,
/// so `./x.toml` and its absolute path agree, or the built-in template's
/// name. Everything piped in on stdin is called `-`, so a layout read from
/// there is told apart by its contents instead (#129): two different
/// generated layouts don't block each other, and the same one still does.
pub(crate) fn run_lock_source(path: &Path, contents: &str) -> String {
    if is_stdin(path) {
        format!("<stdin>\0{}", contents)
    } else {
        std::fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
            .to_string()
    }
}

/// What a `--layout`/`--template` run's lock is keyed on: its source
/// (`run_lock_source()`) and the workspace it builds on, through
/// `WorkspaceTarget::run_lock_key()`, so every spelling of one workspace
/// takes the same lock (#129). Two runs of the same layout on different
/// workspaces don't block each other; `None` locks on the source alone.
fn run_lock_identity(source: &str, workspace: Option<sway_launch::WorkspaceTarget>) -> String {
    let workspace = workspace
        .map(|workspace| workspace.run_lock_key())
        .unwrap_or_default();
    format!("{}\0{}", source, workspace)
}

/// The lock file's name for an identity: a stable FNV-1a hash, so any path
/// or workspace name maps to a plain file name.
fn run_lock_file_name(identity: &str) -> String {
    let hash = identity
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    format!("{:016x}.lock", hash)
}

/// Takes the run's lock (#55): an exclusive lock on a file under
/// `$XDG_RUNTIME_DIR/sway-launch/`, held for the life of the process and
/// released by the kernel when it exits, even on a crash. It is taken before
/// `BuildLocation::apply()`, so a refused run changes nothing; the workspace
/// is the one the build location names, or for a launching run the one it
/// will build on, read from the tree: the focused workspace, or with only
/// `--on-output` the workspace that output shows. `None` when locking is off
/// (`--no-lock`) or there is no runtime directory to put it in.
fn acquire_run_lock(
    build: sway_launch::BuildLocation,
    launches: bool,
    lock_source: &str,
    args: &Args,
) -> Result<Option<std::fs::File>, String> {
    let Some(directory) = lock_directory(args)? else {
        return Ok(None);
    };
    // A run that only retargets existing windows makes no IPC call of its
    // own here, so it locks on the file alone.
    let unnamed_workspace = match build.workspace {
        None if launches => sway_launch::unnamed_build_workspace_name(build.output)
            .ok()
            .flatten(),
        _ => None,
    };
    let workspace = build.workspace.or(unnamed_workspace
        .as_deref()
        .map(sway_launch::WorkspaceTarget::Name));
    let path = directory.join(run_lock_file_name(&run_lock_identity(
        lock_source,
        workspace,
    )));
    let file = open_lock_file(&path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(run_lock_held_message(
            &workspace
                .map(|workspace| workspace.to_string())
                .unwrap_or_default(),
        )),
        Err(std::fs::TryLockError::Error(error)) => {
            Err(format!("run lock: {}: {}", path.display(), error))
        }
    }
}

/// Where the run locks live, `$XDG_RUNTIME_DIR/sway-launch/`, created when
/// missing. `None` when locking is off (`--no-lock`) or there is no runtime
/// directory, with a `--verbose` note for the second.
fn lock_directory(args: &Args) -> Result<Option<std::path::PathBuf>, String> {
    if args.no_lock {
        return Ok(None);
    }
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty())
    else {
        if args.verbose {
            diagnostic!("XDG_RUNTIME_DIR is not set; running without a run lock");
        }
        return Ok(None);
    };
    let directory = Path::new(&runtime_dir).join("sway-launch");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("run lock: {}: {}", directory.display(), error))?;
    Ok(Some(directory))
}

fn open_lock_file(path: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|error| format!("run lock: {}: {}", path.display(), error))
}

/// The session lock's identity for the Sway listening at `socket`: led by a
/// NUL, which no path holds, so it can never equal a `run_lock_identity()`.
fn session_lock_identity(socket: &Path) -> String {
    format!("\0session\0{}", socket.display())
}

/// Takes the session-wide lock (#156), after the run's own lock: one per
/// Sway session (`sway_launch::session_socket()`), taken by every launching
/// `--layout`/`--template` run, so runs started together (several `exec
/// sway-launch --template ...` lines in a Sway config) build one after
/// another instead of taking focus from each other. A run waits for it up to
/// its own `--timeout` (`wait_for_lock()`), then gives up with
/// `session_lock_timeout_message()`. `None` when the run doesn't launch,
/// locking is off, or there's no runtime directory or socket (the run then
/// fails on the socket itself).
fn acquire_session_lock(launches: bool, args: &Args) -> Result<Option<std::fs::File>, String> {
    if !launches {
        return Ok(None);
    }
    let Some(socket) = sway_launch::session_socket() else {
        return Ok(None);
    };
    let Some(directory) = lock_directory(args)? else {
        return Ok(None);
    };
    let path = directory.join(run_lock_file_name(&session_lock_identity(&socket)));
    let file = open_lock_file(&path)?;
    let timeout = time::Duration::from_secs(args.timeout);
    let waiting = || {
        if args.verbose {
            diagnostic!("Waiting for another layout running in this Sway session to finish");
        }
    };
    match wait_for_lock(&file, timeout, waiting) {
        Ok(true) => Ok(Some(file)),
        Ok(false) => Err(session_lock_timeout_message(args.timeout)),
        Err(error) => Err(format!("run lock: {}: {}", path.display(), error)),
    }
}

/// How often `wait_for_lock()` tries a lock another run holds.
const LOCK_RETRY_INTERVAL: time::Duration = time::Duration::from_millis(50);

/// Takes `file`'s exclusive lock, trying again every `LOCK_RETRY_INTERVAL`
/// while another process holds it, for up to `timeout`: `Ok(false)` once that
/// runs out. A blocking `File::lock` has no limit, so a stuck run would
/// hold every later one up forever. `waiting` runs once, the first time the
/// lock is found held.
fn wait_for_lock(
    file: &std::fs::File,
    timeout: time::Duration,
    waiting: impl FnOnce(),
) -> std::io::Result<bool> {
    let deadline = time::Instant::now() + timeout;
    let mut waiting = Some(waiting);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(true),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
        if let Some(waiting) = waiting.take() {
            waiting();
        }
        let left = deadline.saturating_duration_since(time::Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        std::thread::sleep(left.min(LOCK_RETRY_INTERVAL));
    }
}

fn session_lock_timeout_message(timeout_seconds: u64) -> String {
    format!(
        "another layout is running in this Sway session, and was still running after {}s \
         (--timeout); layouts started together build one after another, so raise --timeout to \
         wait longer, or pass --no-lock to run alongside it",
        timeout_seconds
    )
}

fn run_lock_held_message(workspace: &str) -> String {
    let place = if workspace.is_empty() {
        String::new()
    } else {
        format!(" on workspace {:?}", workspace)
    };
    format!(
        "another run of this layout{} is still in progress; pass --no-lock to run it anyway",
        place
    )
}

/// The warning a `--layout`/`--template` run gives when the workspace it's
/// about to build on already holds tiled windows: its shapes assume an
/// empty workspace, so they come out distorted around what's there (the
/// terminal the command was typed into, typically). Printed to stderr and
/// returned for `--json`'s `"warnings"`. A failure to read the tree only
/// skips the warning: the run itself will report a compositor that isn't
/// answering.
fn occupied_workspace_warning(args: &Args) -> Vec<String> {
    let Ok(Some((workspace, tiled_windows))) = sway_launch::focused_workspace_occupancy() else {
        return Vec::new();
    };
    if tiled_windows == 0 {
        return Vec::new();
    }
    let warning = occupied_workspace_message(&workspace, tiled_windows);
    if !args.json {
        diagnostic!("{}{}", stderr_label(Severity::Warning), warning);
    }
    vec![warning]
}

fn occupied_workspace_message(workspace: &str, tiled_windows: usize) -> String {
    format!(
        "workspace {:?} already has {} tiled window(s); the layout will be built around them — \
         run it on an empty workspace (--on-workspace) for the shape it describes",
        workspace, tiled_windows
    )
}

/// `run_steps()`'s `--dry-run` handling: plans every step via
/// `plan_steps_offline()` (see there for how `target_id` resolves with
/// nothing launched) and prints its planned actions (via
/// `print_dry_run_steps()`), continuing the numbering across steps, without
/// ever calling `.run()` or touching Sway IPC.
fn run_steps_dry_run(
    steps: &[layout::LayoutStep],
    build: sway_launch::BuildLocation,
    args: &Args,
) -> ! {
    match plan_steps_offline(steps, args) {
        Ok(planned) => {
            let dry_run_steps: Vec<DryRunStep> = planned.iter().map(dry_run_step).collect();
            print_dry_run_steps(&build.commands(), &dry_run_steps, args.json);
            process::exit(0);
        }
        Err(error) => fail(args.json, &error),
    }
}

/// `--validate`'s handling for `--layout`/`--template`: converts every step
/// to a `SwayLaunch` — exercising exactly the same parsing/validation
/// `to_sway_launch()` already does for a real run (height/width/position
/// formats, target-field consistency, `target_id` resolution) — without
/// ever calling `.run()` or touching Sway IPC. A `--template`'s own
/// `--bindings`/`--apps` resolution (slot count, duplicate slots, binding
/// correctness) already happens unconditionally before `run_steps()` is
/// ever reached (`run_template()`'s own `template::resolve()` call), so
/// `--validate` doesn't need to repeat that separately — by the time
/// `steps` reaches here, that part has already succeeded. Planned via
/// `plan_steps_offline()`, the same as `run_steps_dry_run()`, and through
/// the same `plan_step()` a real run uses.
///
/// Success output names the `--layout`/`--template` argument as given
/// (`args.layout`/`args.template`, whichever is set — exactly one is
/// guaranteed by the `--validate` guard before `run_steps()` is ever
/// reached), not a canonicalized path, the same "echo back what was passed"
/// convention `--show-template`'s `"name"` field already uses: `valid:
/// <source> (N step(s))`, or `{"source": ..., "steps": N, "valid": true}`
/// under `--json`.
fn run_steps_validate(steps: &[layout::LayoutStep], args: &Args) -> ! {
    let source = steps_source(args);
    if let Err(error) = plan_steps_offline(steps, args) {
        fail(args.json, &error);
    }

    if args.json {
        print_line(
            &serde_json::json!({ "source": source, "valid": true, "steps": steps.len() })
                .to_string(),
        );
    } else {
        print_line(&format!("valid: {} ({} step(s))", source, steps.len()));
    }
    process::exit(0);
}

/// One step's checks and conversion to a `SwayLaunch`: the duplicate-`id`
/// check, then `to_sway_launch()` against the ids resolved so far. Shared by
/// a real run (`run_steps()`) and the offline modes (`plan_steps_offline()`),
/// so a rule added here applies to all three — `--validate` can't call a file
/// valid that a real run would reject, which is the point of `--validate`.
/// Returns the bare message; the caller adds the `<source>: step N:` prefix
/// (`step_error()`), since a real run reports through `fail_step()` instead.
fn plan_step<'a>(
    step: &'a layout::LayoutStep,
    args: &Args,
    resolved_ids: &HashMap<String, i64>,
) -> Result<sway_launch::SwayLaunch<'a>, String> {
    if let Some(id) = step.id.as_deref() {
        if resolved_ids.contains_key(id) {
            return Err(format!("id {:?} was already used by an earlier step", id));
        }
    }
    step.to_sway_launch(
        time::Duration::from_secs(args.timeout),
        time::Duration::from_millis(args.wait_time),
        args.verbose,
        resolved_ids,
        sway_launch::DirectoryCheck::for_offline(args.dry_run || args.validate),
    )
}

/// `--concurrent`'s launched windows (#73), each step's waiting on the
/// staging workspace until its turn comes in `run_steps()`.
struct StagedLaunches {
    staging: sway_launch::Staging,
    windows: StagedWindows,
    /// The launches whose window was never resolved (`unresolved_steps()`),
    /// reported when the run ends however it ends.
    unresolved: Vec<UnresolvedLaunch>,
}

/// A `--concurrent` launch that timed out or was interrupted with no window
/// resolved for it (`RunError::unresolved_launch`, #126). Sway opens a
/// window on the workspace its `exec` ran on, recreating it if it's gone, so
/// an application slower than the run opens its window on the staging
/// workspace, out of sight; nothing this run does can bring it back, so the
/// run says where to look instead: `--json`'s `unresolved_launches`, or a
/// warning on stderr.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UnresolvedLaunch {
    /// The `--layout`/`--template` argument (`steps_source()`).
    source: String,
    /// The step's 1-based number.
    step: usize,
    staging_workspace: String,
}

impl UnresolvedLaunch {
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::json!({ "step": self.step, "staging_workspace": self.staging_workspace })
    }

    pub(crate) fn message(&self) -> String {
        format!(
            "{}: step {}: its application may still open a window, on workspace {:?} where \
             --concurrent started it rather than on the build workspace",
            self.source, self.step, self.staging_workspace
        )
    }
}

/// The staged launches, in step order, that `UnresolvedLaunch` describes.
fn unresolved_steps(windows: &StagedWindows) -> Vec<usize> {
    windows
        .iter()
        .filter(|(_, result)| result.as_ref().is_err_and(|error| error.unresolved_launch))
        .map(|(index, _)| *index)
        .collect()
}

type StagedWindows =
    std::collections::BTreeMap<usize, Result<sway_launch::RunOutcome, sway_launch::RunError>>;

/// Removes and returns the first staged launch, in step order, that fails
/// the run: any failure but a timeout `--continue-on-timeout` carries on
/// past, which stays for its step to report when it's reached.
fn take_staging_failure(
    windows: &mut StagedWindows,
    continue_on_timeout: bool,
) -> Option<(usize, sway_launch::RunError)> {
    let index = *windows
        .iter()
        .find(|(_, result)| {
            result
                .as_ref()
                .is_err_and(|error| !(continue_on_timeout && error.window_timed_out))
        })?
        .0;
    let error = windows.remove(&index)?.err()?;
    Some((index, error))
}

/// Fails every staged launch whose window an earlier step's launch already
/// has: launches running at the same time can't tell such a window apart,
/// as when a single-instance application hands both requests to one process
/// and each launch then adopts its window.
fn reject_shared_windows(windows: &mut StagedWindows) {
    let mut claimed: HashMap<i64, usize> = HashMap::new();
    for (index, result) in windows.iter_mut() {
        let Ok(outcome) = result else { continue };
        if let Some(first) = claimed.get(&outcome.container_id) {
            *result = Err(format!(
                "its window (container {}) is also step {}'s: launches running at the same time \
                 couldn't be told apart, as happens with a single-instance application. Run \
                 without --concurrent",
                outcome.container_id,
                first + 1
            )
            .into());
        } else {
            claimed.insert(outcome.container_id, *index);
        }
    }
}

/// Every window still waiting on the staging workspace, mapped to the step
/// whose launch opened it, leaving out `except`'s own: a failed launch's
/// windows too, since its step may not have been reached yet.
fn staged_window_owners(windows: &StagedWindows, except: Option<usize>) -> HashMap<i64, usize> {
    let mut owners = HashMap::new();
    for (index, result) in windows {
        if Some(*index) == except {
            continue;
        }
        let container_ids = match result {
            Ok(outcome) => vec![outcome.container_id],
            Err(error) => failed_launch_windows(error)
                .into_iter()
                .map(|(container_id, _)| container_id)
                .collect(),
        };
        for container_id in container_ids {
            owners.entry(container_id).or_insert(*index);
        }
    }
    owners
}

/// The error for a step that resolved to a window a later step's launch
/// opened (#190), refused before it acts on it: `Target::Existing` leaves
/// those out, so only a `con_id` naming one gets here, and both steps would
/// otherwise report it.
fn staged_window_taken_error(container_id: i64, owner: usize) -> sway_launch::RunError {
    format!(
        "container {} is step {}'s window, launched ahead of its turn by --concurrent: a step \
         can't target a window a later step opens",
        container_id,
        owner + 1
    )
    .into()
}

/// Launches every `command` step at once (#73), without its actions, on a
/// staging workspace, and returns each one's result by step index.
/// `run_steps()` has already checked the whole plan, so every step plans
/// again here without error. Two steps ending up with the same window (a
/// single-instance application handing both requests to one process, whose
/// window each then adopts) can't be told apart, so the later one fails.
fn stage_launches(
    steps: &[layout::LayoutStep],
    build_workspace: &sway_launch::BuildWorkspace,
    args: &Args,
) -> StagedLaunches {
    let staging = match sway_launch::Staging::enter(build_workspace, args.verbose) {
        Ok(staging) => staging,
        Err(error) => fail(args.json, &format!("{}: {}", steps_source(args), error)),
    };
    let no_ids = HashMap::new();
    let launches: Vec<(usize, sway_launch::SwayLaunch)> = steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.command.is_some())
        .map(|(index, step)| {
            let planned = plan_step(step, args, &no_ids)
                .expect("run_steps() already checked every step with plan_steps_offline()");
            (index, launch_only(planned, args))
        })
        .collect();
    if args.verbose {
        diagnostic!(
            "Launching {} steps at once on workspace {:?}",
            launches.len(),
            staging.staging_workspace()
        );
    }
    let group = sway_launch::ConcurrentLaunches::begin();
    let mut windows: StagedWindows = std::thread::scope(|scope| {
        let handles: Vec<_> = launches
            .into_iter()
            .map(|(index, launch)| (index, scope.spawn(move || launch.run())))
            .collect();
        handles
            .into_iter()
            .map(|(index, handle)| {
                let result = handle
                    .join()
                    .unwrap_or_else(|_| Err("the launch's thread panicked".to_string().into()));
                (index, result)
            })
            .collect()
    });
    drop(group);
    if let Err(error) = staging.leave(args.verbose) {
        diagnostic!(
            "{}{}",
            stderr_label(Severity::Warning),
            sway_launch::escape_control_characters(&error, false)
        );
    }

    reject_shared_windows(&mut windows);
    let unresolved = unresolved_steps(&windows)
        .into_iter()
        .map(|index| UnresolvedLaunch {
            source: steps_source(args),
            step: index + 1,
            staging_workspace: staging.staging_workspace().to_string(),
        })
        .collect();
    StagedLaunches {
        staging,
        windows,
        unresolved,
    }
}

/// A step's `SwayLaunch` reduced to its launch, for `stage_launches()`: its
/// actions run later, once the window has been arranged.
fn launch_only<'a>(
    planned: sway_launch::SwayLaunch<'a>,
    args: &Args,
) -> sway_launch::SwayLaunch<'a> {
    sway_launch::SwayLaunch {
        actions: sway_launch::ActionRequest::default(),
        // The window opens on the staging workspace wherever focus goes.
        build_workspace: None,
        warn_on_misplaced_launch: false,
        catch_late_window: args.rollback_on_error && !args.continue_on_timeout,
        ..planned
    }
}

/// Runs a staged step once its turn comes: its window is moved to the build
/// workspace and focused, as it would be had it just opened there, then the
/// step's actions run against it. The launch's ownership carries over, so
/// `--rollback-on-error` treats the window as it would a sequential launch.
fn arrange_staged(
    staging: &sway_launch::Staging,
    staged: Result<sway_launch::RunOutcome, sway_launch::RunError>,
    planned: sway_launch::SwayLaunch,
    verbose: bool,
) -> Result<sway_launch::RunOutcome, sway_launch::RunError> {
    let launched = staged?;
    let container_id = launched.container_id;
    let ownership = launched.launch_ownership;
    let with_window = |error: sway_launch::RunError| sway_launch::RunError {
        container_id: Some(container_id),
        launch_ownership: ownership,
        ..error
    };
    staging
        .bring(container_id, true, verbose)
        .map_err(|message| with_window(message.into()))?;
    sway_launch::SwayLaunch {
        target: sway_launch::Target::ConId(container_id),
        launch_setup: None,
        build_workspace: None,
        catch_late_window: false,
        ..planned
    }
    .run()
    .map(|outcome| sway_launch::RunOutcome {
        launch_ownership: ownership,
        ..outcome
    })
    .map_err(with_window)
}

/// What a run has resolved so far: every window, the named ones by `id`
/// (`--json`'s `containers`), and the ones `--rollback-on-error` may close.
/// Every step records through `record()`, so the three can't disagree about
/// a window, and every failure goes through `fail()`, so none leaves a
/// staged window behind.
#[derive(Debug, Default, PartialEq)]
struct RunProgress {
    container_ids: Vec<i64>,
    resolved_ids: HashMap<String, i64>,
    /// Only ids this invocation itself launched via `command`
    /// (`Target::Exec`), not ones it merely retargeted via
    /// con_id/existing/target_id — see `fail_step()`'s doc comment.
    launched_container_ids: Vec<i64>,
    /// `--concurrent`'s unresolved launches, once `release_staged()` has
    /// taken them from the staging.
    unresolved_launches: Vec<UnresolvedLaunch>,
}

impl RunProgress {
    /// Records a window `step` resolved: as progress, under the step's `id`
    /// when it has one, and for rollback only when this run can prove it
    /// launched it. A step that retargets an existing window reports no
    /// ownership at all, and neither does one whose window was adopted via
    /// the PID-marker fallback rather than confirmed — see
    /// `sway_launch::LaunchOwnership`.
    fn record(
        &mut self,
        step: &layout::LayoutStep,
        container_id: i64,
        ownership: Option<sway_launch::LaunchOwnership>,
    ) {
        if let Some(id) = step.id.as_deref() {
            self.resolved_ids.insert(id.to_string(), container_id);
        }
        self.record_unnamed(container_id, ownership);
    }

    fn record_unnamed(
        &mut self,
        container_id: i64,
        ownership: Option<sway_launch::LaunchOwnership>,
    ) {
        self.container_ids.push(container_id);
        if ownership == Some(sway_launch::LaunchOwnership::Launched) {
            self.launched_container_ids.push(container_id);
        }
    }

    /// Records a timed-out launch's late window (`record_late_windows()`)
    /// for rollback only: it was moved out of the layout, so it isn't
    /// progress.
    fn record_late(&mut self, container_id: i64) {
        self.launched_container_ids.push(container_id);
    }

    /// Records every window a failed step is known to have opened or
    /// resolved (`failed_launch_windows()`), the first under the step's
    /// `id`, and returns their ids.
    fn record_failed(
        &mut self,
        step: &layout::LayoutStep,
        error: &sway_launch::RunError,
    ) -> Vec<i64> {
        let windows = failed_launch_windows(error);
        for (position, &(container_id, ownership)) in windows.iter().enumerate() {
            if position == 0 {
                self.record(step, container_id, Some(ownership));
            } else {
                self.record_unnamed(container_id, Some(ownership));
            }
        }
        windows
            .into_iter()
            .map(|(container_id, _)| container_id)
            .collect()
    }

    /// Records a staged launch's result, a success or a failure, the same way
    /// a sequential step's would be, and returns the windows it holds.
    fn record_staged(
        &mut self,
        step: &layout::LayoutStep,
        result: &Result<sway_launch::RunOutcome, sway_launch::RunError>,
    ) -> Vec<i64> {
        match result {
            Ok(outcome) => {
                self.record(step, outcome.container_id, outcome.launch_ownership);
                vec![outcome.container_id]
            }
            Err(error) => self.record_failed(step, error),
        }
    }

    /// The progress a failed run reports as still open, leaving out anything
    /// rollback touched, whether it closed or not: a rolled-back window is
    /// gone, and one whose kill failed or that ignored its close request is
    /// in a state this run can't vouch for. The failed ones are still
    /// reported, under their own field — see `RollbackOutcome`.
    fn left_open(&self, attempted: &[i64]) -> PartialProgress {
        PartialProgress {
            container_ids: self
                .container_ids
                .iter()
                .copied()
                .filter(|id| !attempted.contains(id))
                .collect(),
            containers: self
                .resolved_ids
                .iter()
                .filter(|(_, id)| !attempted.contains(id))
                .map(|(name, id)| (name.clone(), *id))
                .collect(),
            unresolved_launches: self.unresolved_launches.clone(),
        }
    }

    /// Fails the run at step `index`: brings every staged window back first
    /// (`release_staged()`), then reports and rolls back (`fail_step()`).
    fn fail(
        mut self,
        staged: &mut Option<StagedLaunches>,
        steps: &[layout::LayoutStep],
        args: &Args,
        index: usize,
        error: sway_launch::RunError,
    ) -> ! {
        release_staged(staged, steps, args.verbose, &mut self);
        fail_step(args, &self, index, error)
    }
}

/// Before a `--concurrent` run fails, moves every staged window its turn
/// never came for to the build workspace, rather than leaving it on the
/// staging workspace out of sight, and counts it as progress: reported, and
/// rolled back under `--rollback-on-error` when this run launched it. The
/// launches with no window to bring back are recorded for the failure to
/// report (`UnresolvedLaunch`).
fn release_staged(
    staged: &mut Option<StagedLaunches>,
    steps: &[layout::LayoutStep],
    verbose: bool,
    progress: &mut RunProgress,
) {
    let Some(staged) = staged.take() else { return };
    progress.unresolved_launches = staged.unresolved;
    for (index, result) in &staged.windows {
        for container_id in progress.record_staged(&steps[*index], result) {
            bring_back(&staged.staging, container_id, verbose);
        }
    }
}

/// Moves a staged window to the build workspace, unfocused, warning rather
/// than failing when it can't: the run is already failing.
fn bring_back(staging: &sway_launch::Staging, container_id: i64, verbose: bool) {
    if let Err(error) = staging.bring(container_id, false, verbose) {
        diagnostic!(
            "{}{}",
            stderr_label(Severity::Warning),
            sway_launch::escape_control_characters(&error, false)
        );
    }
}

/// Every window a failed step is known to have opened or resolved, with its
/// ownership: `container_id` first, then any further window a timed-out
/// launch proved it opened (`also_launched`, #168).
fn failed_launch_windows(
    error: &sway_launch::RunError,
) -> Vec<(i64, sway_launch::LaunchOwnership)> {
    let first = match (error.container_id, error.launch_ownership) {
        (Some(container_id), Some(ownership)) => Some((container_id, ownership)),
        _ => None,
    };
    first
        .into_iter()
        .chain(
            error
                .also_launched
                .iter()
                .map(|&container_id| (container_id, sway_launch::LaunchOwnership::Launched)),
        )
        .collect()
}

/// `plan_step()` over every step without running any, for `--dry-run` and
/// `--validate`. Nothing launches, so there's no real container id for a
/// later step's `target_id` to resolve against: each named step's own index
/// stands in as a placeholder. It's never printed — `dry_run_step()` never
/// names a container id — it only keeps a legitimate `target_id` reference
/// from failing its lookup. Stops at the first error, already `<source>: step N:`
/// prefixed.
fn plan_steps_offline<'a>(
    steps: &'a [layout::LayoutStep],
    args: &Args,
) -> Result<Vec<sway_launch::SwayLaunch<'a>>, String> {
    let mut resolved_ids = HashMap::new();
    let mut planned = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        let sway_launch = plan_step(step, args, &resolved_ids)
            .map_err(|error| step_error(args, index, &error))?;
        planned.push(sway_launch);
        if let Some(id) = step.id.as_deref() {
            resolved_ids.insert(id.to_string(), index as i64);
        }
    }
    Ok(planned)
}

/// Prefixes a step's error with the `--layout`/`--template` it came from and
/// its 1-based step number (`<source>: step N: <message>`), the one format
/// every step failure is reported in — the same `<file>: ` prefix file-read
/// and parse errors carry, so a script applying several files in a row, or a
/// keybinding's log, can tell which one failed.
fn step_error(args: &Args, index: usize, message: &str) -> String {
    format!("{}: step {}: {}", steps_source(args), index + 1, message)
}

/// The `--layout`/`--template` argument as the user gave it — exactly one is
/// set by the time `run_steps()` is reached, since that's the only way to
/// get there. Not canonicalized: the same "echo back what was passed"
/// convention `--show-template`'s `"name"` field uses.
fn steps_source(args: &Args) -> String {
    args.layout
        .as_deref()
        .or(args.template.as_deref())
        .map(input_label)
        .expect("run_steps() is only reachable via --layout or --template")
}

/// Reports a step failure (`<source>: step N: <message>`) — shared by every failure
/// branch in `run_steps()`, since an id collision or a step-conversion error
/// is caught *before* that step's own `SwayLaunch::run()` ever executes, but
/// earlier steps may already have launched real windows needing the same
/// `--rollback-on-error` cleanup as a later step's own `run()` failure.
/// `RunProgress.launched_container_ids` is only ever ids a `Target::Exec` step resolved
/// *and* could prove it launched (`LaunchOwnership::Launched`) — windows this
/// invocation retargeted via con_id/existing/target_id were already open before
/// this invocation started, and a window merely adopted through the PID-marker
/// fallback was never confirmed to be ours either, so rolling either back would
/// close something this run didn't create.
///
/// Its `container_ids`/`resolved_ids` are every step that completed before this
/// failure, plus the failing step's own window if it launched one, reported
/// under `--json` so a caller can identify the windows left open — see
/// `PartialProgress`. They're the full set, not just the launched ones: a
/// retargeted window isn't this run's to close, but it *is* something the
/// caller may still want to know the id of.
///
/// Whatever container the failing step resolved, launched or retargeted, is
/// also named by `error.container_id`, so a caller can tell which window
/// belongs to the step that failed; that is cleared when rollback touched it,
/// for the same reason the lists are filtered.
fn fail_step(
    args: &Args,
    run_progress: &RunProgress,
    index: usize,
    error: sway_launch::RunError,
) -> ! {
    let rollback_outcome = if args.rollback_on_error {
        rollback(&run_progress.launched_container_ids)
    } else {
        RollbackOutcome::default()
    };
    let attempted: Vec<i64> = rollback_outcome.attempted().copied().collect();
    let progress = run_progress.left_open(&attempted);
    let container_id = error.container_id.filter(|id| !attempted.contains(id));
    fail_with_rollback(
        args.json,
        &sway_launch::RunError {
            message: step_error(args, index, &error.message),
            container_id,
            ..error
        },
        &rollback_outcome,
        &progress,
    );
}

/// How long `rollback()` waits, in total, for the windows whose close request
/// Sway accepted to actually leave the tree before reporting the remainder
/// under `rollback_failed`. A client that honours the request closes in
/// milliseconds (`foot` is gone on the first poll); one that doesn't — an
/// unsaved-changes prompt — won't close within any wait worth imposing on a
/// run that has already failed, so this only needs to cover the honest case
/// with margin.
const ROLLBACK_CLOSE_GRACE: time::Duration = time::Duration::from_secs(1);

/// How often `rollback()` re-reads the tree inside `ROLLBACK_CLOSE_GRACE`.
const ROLLBACK_CLOSE_POLL_INTERVAL: time::Duration = time::Duration::from_millis(20);

/// Best-effort `--rollback-on-error` cleanup: sends every id in
/// `launched_container_ids` a `[con_id] kill`, most-recently-launched first,
/// then waits up to `ROLLBACK_CLOSE_GRACE` for the ones Sway accepted to
/// leave the tree.
///
/// The wait is what makes `rolled_back` true: `kill` is a close *request*,
/// and Sway's success reply only says the request was sent (see
/// `RollbackOutcome`). A window still present when the wait runs out is
/// warned about and reported under `rollback_failed`, not as closed.
///
/// Killing by an id recorded earlier is not a time-of-check problem: the id
/// can't by then refer to some other window, because Sway allocates container ids monotonically and does not
/// reuse them, confirmed by six launch/kill cycles handing out 5, 6, 7, 8, 9,
/// 10 with no repeat. An id this invocation launched therefore always refers to
/// exactly the container it launched, or to nothing at all (in which case the
/// kill fails and is reported — see `RollbackOutcome`).
///
/// What remains is not an identity question but a judgement one: the user may
/// have started *using* that window since it launched, and rollback closes it
/// anyway. That is deliberate and documented in README's "Running layouts"
/// section — the feature undoes launches, and there is no way to ask a window
/// whether it has become important to someone. It is opt-in for that reason.
///
/// A kill that itself fails (e.g. the window already closed on its own in the
/// meantime) is logged to stderr and recorded rather than treated as fatal —
/// it doesn't stop the rest of the rollback, unless the compositor itself has
/// stopped answering (see `send_rollback_kills()`), and the original step
/// failure being reported by `fail_step()` stays the primary error either way. Returns both outcomes separately (see
/// `RollbackOutcome`), for `fail_with_rollback()` to report alongside that
/// error: "closed" and "tried to close and couldn't confirm it" are different
/// facts, and a caller cleaning up after a failed run needs to tell them
/// apart.
fn rollback(launched_container_ids: &[i64]) -> RollbackOutcome {
    let kills = send_rollback_kills(launched_container_ids, sway_launch::kill_container);
    if kills.compositor_unreachable {
        // Polling the tree would only wait out another IPC timeout against
        // the same compositor, so an accepted kill stays unconfirmed.
        if !kills.accepted.is_empty() {
            diagnostic!(
                "{}--rollback-on-error couldn't confirm that container(s) {} closed: the \
                 compositor stopped responding",
                stderr_label(Severity::Warning),
                join_ids(&kills.accepted)
            );
        }
        let still_open = kills.accepted.clone();
        return classify_rollback(kills.accepted, kills.failed, &still_open);
    }
    let still_open = wait_for_containers_to_close(&kills.accepted);
    for container_id in &still_open {
        diagnostic!(
            "{}--rollback-on-error asked container {} to close, but it was still open after {} ms",
            stderr_label(Severity::Warning),
            container_id,
            ROLLBACK_CLOSE_GRACE.as_millis()
        );
    }
    classify_rollback(kills.accepted, kills.failed, &still_open)
}

/// The kill half of `rollback()`, before any wait for windows to close.
struct RollbackKills {
    /// Ids whose `[con_id] kill` Sway accepted, in the order they were sent.
    accepted: Vec<i64>,
    /// Ids whose kill failed, or that were never sent one because the
    /// compositor had already stopped answering.
    failed: Vec<i64>,
    /// Whether a kill failed because the compositor couldn't be reached or
    /// didn't answer (see `sway_launch::KillError`).
    compositor_unreachable: bool,
}

/// Sends `kill` to every id in `launched_container_ids`,
/// most-recently-launched first, warning about each one that fails.
///
/// A kill Sway refuses doesn't stop the rest. One that fails because the
/// compositor didn't answer does: every remaining id goes straight into
/// `failed` under a single warning, since each further attempt would fail the
/// same way, and against a stalled compositor would first wait out another
/// full IPC timeout — ten seconds per launched window, on a run already
/// reporting a failure (#94). `kill` is a parameter so the stopping rule can
/// be tested without a compositor.
fn send_rollback_kills(
    launched_container_ids: &[i64],
    mut kill: impl FnMut(i64) -> Result<(), sway_launch::KillError>,
) -> RollbackKills {
    let mut kills = RollbackKills {
        accepted: Vec::new(),
        failed: Vec::new(),
        compositor_unreachable: false,
    };
    let mut remaining = launched_container_ids.iter().rev().copied();
    while let Some(container_id) = remaining.next() {
        match kill(container_id) {
            Ok(()) => kills.accepted.push(container_id),
            Err(error) => {
                diagnostic!(
                    "{}--rollback-on-error failed to close container {}: {}",
                    stderr_label(Severity::Warning),
                    container_id,
                    error
                );
                kills.failed.push(container_id);
                if let sway_launch::KillError::Unreachable(_) = error {
                    kills.compositor_unreachable = true;
                    let skipped: Vec<i64> = remaining.collect();
                    if !skipped.is_empty() {
                        diagnostic!(
                            "{}--rollback-on-error skipped closing {} more window(s) ({}): the \
                             compositor is not responding",
                            stderr_label(Severity::Warning),
                            skipped.len(),
                            join_ids(&skipped)
                        );
                    }
                    kills.failed.extend(skipped);
                    break;
                }
            }
        }
    }
    kills
}

/// Polls the tree until none of `container_ids` is left in it, or
/// `ROLLBACK_CLOSE_GRACE` runs out, and returns whichever are still there.
/// A tree read that fails leaves the outcome unknowable, so every id is
/// returned as still present: claiming a close this run never saw would be
/// the very error the wait exists to prevent.
fn wait_for_containers_to_close(container_ids: &[i64]) -> Vec<i64> {
    if container_ids.is_empty() {
        return Vec::new();
    }
    let deadline = time::Instant::now() + ROLLBACK_CLOSE_GRACE;
    loop {
        match sway_launch::containers_still_present(container_ids) {
            Ok(present) if present.is_empty() || time::Instant::now() >= deadline => {
                return present;
            }
            Ok(_) => std::thread::sleep(ROLLBACK_CLOSE_POLL_INTERVAL),
            Err(error) => {
                diagnostic!(
                    "{}--rollback-on-error couldn't confirm which windows closed: {}",
                    stderr_label(Severity::Warning),
                    error
                );
                return container_ids.to_vec();
            }
        }
    }
}

/// The pure half of `rollback()`: an id whose close request was accepted
/// counts as rolled back only if it isn't in `still_open`; the rest join the
/// ids whose kill failed outright.
fn classify_rollback(
    accepted: Vec<i64>,
    mut failed: Vec<i64>,
    still_open: &[i64],
) -> RollbackOutcome {
    let (still_open_ids, rolled_back): (Vec<i64>, Vec<i64>) = accepted
        .into_iter()
        .partition(|container_id| still_open.contains(container_id));
    failed.extend(still_open_ids);
    RollbackOutcome {
        rolled_back,
        failed,
    }
}

#[cfg(test)]
mod tests;
