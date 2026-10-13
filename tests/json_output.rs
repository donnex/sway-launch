// --con-id alone never touches the Sway IPC socket (it's returned directly,
// no connection needed), so these exercise main()'s actual output
// formatting against the compiled binary without requiring a live Sway
// session.
//
// A test that needs an IPC call to *fail* says so explicitly, via
// sway_launch_without_a_compositor() (tests/common/), rather than assuming
// the machine running it has no Sway session.

mod common;

use common::sway_launch_without_a_compositor;
use std::process::Command;

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
fn con_id_json_error_reports_the_resolved_container_id() {
    // A run that resolved its container and then failed on a later action
    // used to drop the id, leaving a caller no way to find a window that was
    // really on screen. --con-id resolves without IPC and --floating then
    // fails on the socket, which is the same shape headlessly.
    let output = sway_launch_without_a_compositor()
        .args(["--con-id", "42", "--floating", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    let last_line = stderr.lines().next_back().unwrap_or_default();
    let error: serde_json::Value = serde_json::from_str(last_line)
        .unwrap_or_else(|_| panic!("the last stderr line should be JSON, got {stderr:?}"));
    assert_eq!(error["container_id"], serde_json::json!(42));
}

#[test]
fn con_id_plain_error_names_the_resolved_container_on_stderr_only() {
    let output = sway_launch_without_a_compositor()
        .args(["--con-id", "42", "--floating"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "a failed run must leave stdout empty"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.contains("container 42 was the target when this failed"),
        "expected the resolved container on stderr, got {stderr:?}"
    );
    assert!(
        stderr
            .lines()
            .next_back()
            .unwrap_or_default()
            .starts_with("error: floating enable: "),
        "the error message should still be the last line: {stderr:?}"
    );
}

#[test]
fn plain_error_is_labelled_and_uncolored_when_stderr_is_not_a_terminal() {
    // Labelled like clap's own usage errors, so the line that explains why a
    // layout script stopped stands out from --verbose diagnostics. stderr is
    // a pipe here, so the label must carry no escape codes.
    let output = sway_launch_without_a_compositor()
        .args(["--con-id", "42", "--floating"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    let last_line = stderr.lines().next_back().unwrap_or_default();
    assert!(
        last_line.starts_with("error: "),
        "expected an error: label, got {stderr:?}"
    );
    assert!(
        !stderr.contains('\x1b'),
        "a piped stderr must not be colored: {stderr:?}"
    );
}

#[test]
fn json_error_omits_container_id_when_nothing_was_resolved() {
    // --existing fails while resolving, before there is any container to
    // report, so the field must be absent rather than null or a guess.
    let output = sway_launch_without_a_compositor()
        .args(["--existing", "--app-id", "no-such-app", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    let last_line = stderr.lines().next_back().unwrap_or_default();
    let error: serde_json::Value = serde_json::from_str(last_line)
        .unwrap_or_else(|_| panic!("the last stderr line should be JSON, got {stderr:?}"));
    assert!(error.get("error").is_some());
    assert!(
        error.get("container_id").is_none(),
        "no container was resolved, so none should be reported: {error:?}"
    );
}

#[test]
fn a_failing_action_names_itself_in_the_error() {
    // An invocation with several actions used to report a bare failure that
    // couldn't be traced to any one of them; every action's error now leads
    // with its own Sway command verb. Made to fail on the socket, so it runs
    // with or without a compositor.
    for (flags, prefix) in [
        (&["--floating"][..], "error: floating enable: "),
        (
            &["--workspace", "web"][..],
            "error: move workspace \"web\": ",
        ),
        (
            &["--mark", "m", "--floating"][..],
            "error: floating enable: ",
        ),
    ] {
        let output = sway_launch_without_a_compositor()
            .args(["--con-id", "42"])
            .args(flags)
            .output()
            .expect("failed to run sway-launch binary");

        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
        let last_line = stderr.lines().next_back().unwrap_or_default();
        assert!(
            last_line.starts_with(prefix),
            "expected {flags:?}'s error to start with {prefix:?}, got {stderr:?}"
        );
    }
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
            stderr.contains("cannot reach Sway IPC"),
            "{flag}'s error should name the failed connection: {stderr:?}"
        );
    }
}

#[test]
fn a_launch_with_an_unreachable_socket_names_sway_ipc_and_the_socket() {
    // A launch subscribes to events before any other IPC call, so it is the
    // path most people hit first outside a Sway session; it used to report
    // only the bare OS error ("No such file or directory (os error 2)").
    let output = sway_launch_without_a_compositor()
        .arg("foot")
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.contains(
            "cannot reach Sway IPC at /nonexistent/sway-launch-test-socket (from SWAYSOCK)"
        ),
        "{stderr:?}"
    );
    assert!(stderr.contains("inside a Sway session"), "{stderr:?}");
}

#[test]
fn a_launch_with_neither_socket_variable_set_says_so() {
    // With neither variable set, swayipc falls back to `sway --get-socketpath`
    // and, when that finds nothing, connects to an empty path and reports
    // "Invalid argument (os error 22)". An empty PATH makes that discovery
    // fail the same way regardless of whether this machine runs Sway.
    for args in [&["foot"][..], &["--existing", "--app-id", "foot"][..]] {
        let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
            .args(args)
            .env_remove("SWAYSOCK")
            .env_remove("I3SOCK")
            .env("PATH", "")
            .output()
            .expect("failed to run sway-launch binary");

        assert!(!output.status.success(), "{args:?}");
        let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
        assert!(
            stderr.contains("cannot reach Sway IPC: neither SWAYSOCK nor I3SOCK is set"),
            "{args:?}: {stderr:?}"
        );
        assert!(stderr.contains("export SWAYSOCK"), "{args:?}: {stderr:?}");
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
fn zero_timeout_is_rejected_before_touching_the_socket() {
    // A zero timeout used to send the command and only then report a
    // timeout, leaving the change applied but the run failed. It's now a
    // parse error, so it fails before anything reaches Sway -- shown by using
    // a binary pointed at no compositor and getting clap's error, not an IPC one.
    for timeout in ["0", "18446744073709551615"] {
        let output = sway_launch_without_a_compositor()
            .args(["--con-id", "42", "--floating", "--timeout", timeout])
            .output()
            .expect("failed to run sway-launch binary");

        assert_eq!(output.status.code(), Some(2), "--timeout {timeout}");
        let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
        assert!(stderr.contains("--timeout"), "{stderr}");
        assert!(!stderr.contains("Sway IPC"), "{stderr}");
    }
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
        "{\"steps\":[{\"actions\":[{\"command\":\"floating enable\",\"kind\":\"floating\"}],\"target\":\"target existing container\"}]}"
    );
}

#[test]
fn dry_run_json_names_every_action_by_its_flag_not_its_command() {
    // "kind" is --json's stable identity for an action: the flag that
    // produced it, never the Sway command text (which carries user data and
    // quoting). Every action flag is set here, so each one's kind is checked
    // to be its own long flag name with '-' as '_'.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--con-id",
            "42",
            "--new-column",
            "--new-row",
            "--workspace",
            "3",
            "--output",
            "HDMI-A-1",
            "--split",
            "h",
            "--floating",
            "--sticky",
            "--fullscreen",
            "--focus",
            "--height",
            "300px",
            "--width",
            "40ppt",
            "--position",
            "center",
            "--mark",
            "pinned",
            "--scratchpad",
            "--dry-run",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "sway-launch --dry-run --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be a JSON object");
    let kinds: Vec<&str> = json["steps"][0]["actions"]
        .as_array()
        .expect("a dry-run step should carry an actions array")
        .iter()
        .map(|action| action["kind"].as_str().expect("every action has a kind"))
        .collect();
    assert_eq!(
        kinds,
        [
            "new_column",
            "new_row",
            "workspace",
            "output",
            "split",
            "floating",
            "sticky",
            "fullscreen",
            "focus",
            "height",
            "width",
            "position",
            "mark",
            "scratchpad",
        ]
    );
    assert_eq!(
        json["steps"][0]["actions"][12],
        serde_json::json!({ "kind": "mark", "command": "mark \"pinned\"" })
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

#[test]
fn a_closed_stderr_does_not_abort_a_verbose_run() {
    // `eprintln!` panics on a failed write, so `-v ... 2>&1 | head -n1` used
    // to exit 101 on the next diagnostic, stopping between two actions. The
    // read end is closed before the process starts, so the very first
    // diagnostic is the one that fails.
    let (stderr_reader, stderr_writer) = std::io::pipe().expect("failed to create a pipe");
    drop(stderr_reader);

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42", "--verbose"])
        .stderr(stderr_writer)
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "expected exit 0, got {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn a_command_sway_would_split_is_rejected_before_anything_runs() {
    // Sway's command parser splits on an unquoted `,`/`;`, so `foot -T a,b`
    // used to launch `foot -T a` and then fail on `b`, and `foot; mark x`
    // ran `mark x` against the focused window. Rejected at parse time, so
    // this never reaches the (missing) compositor.
    for command in ["foot -T a,b", "foot -T x; mark INJECTED", "foot\nmark x"] {
        let output = sway_launch_without_a_compositor()
            .arg(command)
            .output()
            .expect("failed to run sway-launch binary");

        assert!(!output.status.success(), "{command:?} should be rejected");
        let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
        assert!(
            stderr.contains("Sway"),
            "the error should explain Sway's splitting for {command:?}: {stderr}"
        );
        assert!(
            !stderr.contains("socket"),
            "{command:?} should fail validation, not reach IPC: {stderr}"
        );
    }
}

#[test]
fn a_quoted_separator_in_a_command_is_accepted() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--dry-run", "foot -T 'a,b; c'"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(stdout.trim(), "1. launch foot -T 'a,b; c'");
}
