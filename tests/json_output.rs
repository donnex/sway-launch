// --con-id alone never touches the Sway IPC socket (it's returned directly,
// no connection needed), so these exercise main()'s actual output
// formatting against the compiled binary without requiring a live Sway
// session.
//
// A test that needs an IPC call to *fail* says so explicitly, via
// sway_launch_without_a_compositor() below, rather than assuming the machine
// running it has no Sway session.

use std::process::Command;

/// The binary, pointed at a socket path that cannot exist.
///
/// Some tests here assert that an IPC-touching invocation fails. Left to the
/// ambient environment, that assertion holds on a bare CI runner and breaks in
/// a terminal inside a Sway session — which is where this project is developed,
/// and where CLAUDE.md's own "run `cargo test` before committing" gate runs. It
/// was worse than a false failure for the `--con-id 42` cases: with a session
/// reachable, whether the command failed depended on whether container id 42
/// happened to exist, and if it did, the test floated a real window.
///
/// `I3SOCK` is removed as well as `SWAYSOCK` set, since `socket_path()` reads
/// it first. Both together mean the connect attempt fails on a missing path
/// rather than reaching a compositor, no matter what the environment holds.
fn sway_launch_without_a_compositor() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sway-launch"));
    command
        .env("SWAYSOCK", "/nonexistent/sway-launch-test-socket")
        .env_remove("I3SOCK");
    command
}

#[test]
fn con_id_plain_output_is_a_bare_container_id() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn con_id_json_output_is_a_clean_json_object() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(stdout.trim(), "{\"actions\":[],\"container_id\":42}");
}

#[test]
fn con_id_json_error_output_is_a_structured_object() {
    // --con-id alone never touches the socket, but combining it with an
    // action flag does (the container-state read behind already_at_target()),
    // which is made to fail here by pointing the binary at a socket that
    // cannot exist — exercises --json's error-output shape
    // (fail()/fail_with_rollback() in main.rs) deterministically, on a machine
    // with or without a live compositor.
    let output = sway_launch_without_a_compositor()
        .args(["--con-id", "42", "--floating", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    // Checked as the last line rather than the whole of stderr: the JSON error
    // object is always written last, but a failure that reaches swayipc's own
    // socket-path fallback can print an inherited diagnostic ahead of it.
    let last_line = stderr.lines().next_back().unwrap_or_default();
    assert!(
        last_line.starts_with('{') && last_line.contains("\"error\""),
        "expected the last stderr line to be a JSON error object, got {stderr:?}"
    );
    assert!(
        stderr.contains("\"rolled_back\":[]"),
        "expected an empty rolled_back array outside --rollback-on-error: {stderr:?}"
    );
}

#[test]
fn new_column_and_new_row_report_the_relocation_checks_error() {
    // build_actions() consults relocates_to_another_output() before including
    // NewColumn/NewRow, which needs a live get_outputs()/get_tree() call even
    // to build the plan. When that read fails, the error has to surface rather
    // than the guard being silently skipped and the action silently included.
    //
    // This replaces two unit tests that asserted the same propagation by
    // assuming the machine running them had no reachable Sway socket at all —
    // true on a CI runner, false in a Sway session, where both failed
    // outright. Pointing the binary at a socket that cannot exist states the
    // precondition instead of inheriting it.
    for flag in ["--new-column", "--new-row"] {
        let output = sway_launch_without_a_compositor()
            .args(["--con-id", "42", flag])
            .output()
            .expect("failed to run sway-launch binary");

        assert!(
            !output.status.success(),
            "{flag} should fail when the relocation check can't read the tree"
        );
        let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
        assert!(
            stderr.contains("Sway IPC socket"),
            "{flag}'s error should name the failed connection: {stderr:?}"
        );
    }
}

#[test]
fn rollback_on_error_without_layout_or_template_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42", "--rollback-on-error"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("--rollback-on-error requires --layout or --template"));
}

#[test]
fn dry_run_never_touches_the_socket_even_for_new_column_and_new_row() {
    // --dry-run's whole point: NewColumn/NewRow normally need a live
    // get_outputs()/get_tree() call (relocates_to_another_output()) even
    // just to decide whether to include them, but --dry-run skips that
    // check entirely -- confirmed here by combining --con-id (which alone
    // never touches the socket) with --new-column/--new-row, which
    // definitely would without --dry-run's check_relocation: false.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42", "--new-column", "--new-row", "--dry-run"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "sway-launch --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        "1. target existing container\n2. move right\n3. move down"
    );
}

#[test]
fn dry_run_plain_output_is_a_numbered_list() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--con-id",
            "42",
            "--floating",
            "--mark",
            "pinned",
            "--dry-run",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        "1. target existing container\n2. floating enable\n3. mark \"pinned\""
    );
}

#[test]
fn dry_run_json_output_is_a_structured_steps_array() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42", "--floating", "--dry-run", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        "{\"steps\":[{\"actions\":[\"floating enable\"],\"target\":\"target existing container\"}]}"
    );
}

#[test]
fn dry_run_never_launches_the_command() {
    // A --dry-run with a real command (not --con-id/--existing) must never
    // actually exec it -- this uses a command that would create an
    // unmistakable side effect if it ran, and confirms it didn't.
    // Per-process path: this asserts a marker does *not* exist, so a stray
    // one left at a predictable shared path by another run (or another
    // user) would fail it for the wrong reason.
    let marker_dir = std::env::temp_dir().join(format!(
        "sway-launch-dry-run-test-marker-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir(&marker_dir);

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--dry-run", &format!("mkdir {}", marker_dir.display())])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    assert!(
        !marker_dir.exists(),
        "--dry-run should never actually run the given command"
    );
}

#[test]
fn dry_run_describes_an_existing_target_matched_by_mark() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--existing", "--mark-match", "dropdown-term", "--dry-run"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        "1. target existing window (mark_match=\"dropdown-term\")"
    );
}

#[test]
fn existing_without_app_id_class_or_mark_match_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--existing"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("--existing requires --app-id, --class, or --mark-match"));
}

#[test]
fn mark_match_without_existing_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--mark-match", "foo", "true"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("--mark-match requires --existing"));
}

#[test]
fn a_closed_stdout_exits_cleanly_instead_of_panicking() {
    // Rust ignores SIGPIPE, so an unguarded println! into a closed pipe
    // panics with "failed printing to stdout: Broken pipe" and exits 101.
    // Reproduced by closing the read end of the child's stdout pipe
    // immediately after spawning, which is what `| head` amounts to once
    // head has had enough.
    //
    // Best-effort by nature: if the child writes all ~30 lines before the
    // drop lands they fit the pipe buffer and no EPIPE occurs, in which case
    // this passes trivially. It can only ever fail for the right reason.
    // The deterministic version is in tests/live_sway.rs, against
    // --debug-events, the mode that actually writes until killed.
    let mut child = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--list-templates"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sway-launch binary");
    drop(child.stdout.take());

    let output = child.wait_with_output().expect("sway-launch should exit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "writing to a closed stdout should not panic: {stderr}"
    );
    assert_ne!(
        output.status.code(),
        Some(101),
        "exit 101 is a Rust panic: {stderr}"
    );
}

#[test]
fn debug_events_conflicts_with_a_command() {
    // --debug-events never acts on a window, so a command alongside it could
    // only be discarded -- it used to parse cleanly and dump events while
    // silently never launching `foot`. clap rejects this before any IPC, so
    // it stays headless-safe.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--debug-events", "foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn debug_events_rejects_json_rather_than_ignoring_it() {
    // Same reasoning as completions_rejects_json_rather_than_ignoring_it:
    // debug_events() writes plain `Event: N` lines and never sees the flag, so
    // a raw event dump has no JSON shape for --json to take. It was accepted
    // and discarded until a review noticed the inconsistency with
    // --completions, which had already been fixed for exactly this.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--debug-events", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        !output.status.success(),
        "--debug-events --json should be rejected, not silently ignored"
    );
}

#[test]
fn debug_events_conflicts_with_a_per_window_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--debug-events", "--con-id", "42", "--floating"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn validate_without_layout_or_template_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42", "--validate"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("--validate requires --layout or --template"));
}

#[test]
fn con_id_verbose_diagnostics_go_to_stderr_not_stdout() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42", "--verbose"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert_eq!(stdout.trim(), "42");
    assert!(stderr.contains("Target container id: 42"));
}
