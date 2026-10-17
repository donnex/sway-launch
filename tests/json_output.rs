// --con-id alone never touches the Sway IPC socket (it's returned directly,
// no connection needed), so these exercise main()'s actual output
// formatting against the compiled binary without requiring a live Sway
// session.
//
// A test that needs an IPC call to *fail* says so explicitly, via
// sway_launch_without_a_compositor() (tests/common/), rather than assuming
// the machine running it has no Sway session.

mod common;

use common::{assert_conflict_error, sway_launch_without_a_compositor, TestTempDir};
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
            .starts_with("error: cannot reach Sway IPC"),
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
fn con_id_with_actions_is_looked_up_before_the_first_action() {
    // With actions to apply, --con-id is checked against the tree first, so
    // a wrong id fails with a clear reason rather than inside an action (#107).
    // Here that lookup is what fails on the socket; no action runs, including
    // NewColumn/NewRow's relocation check (execute_plan's own unit tests cover
    // that check failing). Each action's own error prefix is pinned live, by
    // position_errors_clearly_for_a_tiled_window.
    for flags in [
        &["--floating"][..],
        &["--workspace", "web"],
        &["--new-column"],
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
            last_line.starts_with("error: cannot reach Sway IPC"),
            "expected {flags:?} to fail on the lookup, got {stderr:?}"
        );
    }
}

#[test]
fn bare_con_id_is_not_looked_up() {
    // Documented: with no actions, nothing needs the socket, so the id is
    // printed back unchecked and this works outside a Sway session.
    let output = sway_launch_without_a_compositor()
        .args(["--con-id", "42"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");
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
    // --dry-run's whole point: a real run asks the compositor
    // (current_move_skip_reason()) before sending each NewColumn/NewRow, but
    // --dry-run never runs anything, so it never asks -- confirmed here by
    // combining --con-id (which alone never touches the socket) with
    // --new-column/--new-row, which definitely would without --dry-run.
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
        "1. target existing container\n2. floating enable, mark \"pinned\""
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
        json["steps"][0]["actions"][11],
        serde_json::json!({ "kind": "mark", "command": "mark \"pinned\"" })
    );

    // --workspace can't be combined with --new-column/--new-row (#45), so
    // it's checked on its own, together with --output's combined form.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--con-id",
            "42",
            "--workspace",
            "3",
            "--output",
            "HDMI-A-1",
            "--dry-run",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be a JSON object");
    assert_eq!(
        json["steps"][0]["actions"],
        serde_json::json!([
            { "kind": "workspace", "command": "move workspace \"3\"" },
            { "kind": "output", "command": "move workspace to output \"HDMI-A-1\"" },
        ])
    );
}

#[test]
fn workspace_conflicts_with_new_column_and_new_row() {
    for flag in ["--new-column", "--new-row"] {
        let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
            .args(["--con-id", "42", "--workspace", "3", flag, "--dry-run"])
            .output()
            .expect("failed to run sway-launch binary");
        assert_conflict_error(&output, "--workspace", flag);
    }
}

#[test]
fn continue_on_timeout_without_layout_or_template_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--continue-on-timeout", "foot"])
        .output()
        .expect("failed to run sway-launch binary");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.contains("--continue-on-timeout requires --layout or --template"),
        "{stderr}"
    );
}

#[test]
fn on_output_without_layout_or_template_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--on-output", "DP-1", "foot"])
        .output()
        .expect("failed to run sway-launch binary");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.contains("--on-output requires --layout or --template"),
        "{stderr}"
    );
}

#[test]
fn on_workspace_without_layout_or_template_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--on-workspace", "9", "foot"])
        .output()
        .expect("failed to run sway-launch binary");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.contains("--on-workspace requires --layout or --template"),
        "{stderr}"
    );
}

#[test]
fn dry_run_never_launches_the_command() {
    // A --dry-run with a real command (not --con-id/--existing) must never
    // actually exec it -- this uses a command that would create an
    // unmistakable side effect if it ran, and confirms it didn't.
    // The marker lives in a fresh private directory: this asserts it does
    // *not* exist, so one left at a predictable shared path by another run
    // (or another user) would fail it for the wrong reason.
    let temp_dir = TestTempDir::new();
    let marker_dir = temp_dir.join("marker");

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
    assert!(stderr.contains("--existing requires --app-id, --class, --mark-match or --title"));
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
    // silently never launching `foot`.
    //
    // The --debug-events conflict tests run without a compositor: if
    // the conflict were ever dropped, --debug-events would subscribe and
    // print events forever inside a Sway session, hanging the test instead
    // of failing it.
    let output = sway_launch_without_a_compositor()
        .args(["--debug-events", "foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert_conflict_error(&output, "--debug-events", "[COMMAND]");
}

#[test]
fn debug_events_accepts_json() {
    // --debug-events --json prints one JSON object per event (#85). With no
    // compositor it gets as far as subscribing, so the failure is the
    // connection, reported as a --json error object, not a usage error.
    let output = sway_launch_without_a_compositor()
        .args(["--debug-events", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let error: serde_json::Value =
        serde_json::from_str(stderr.lines().next_back().unwrap_or_default())
            .unwrap_or_else(|_| panic!("the last stderr line should be JSON: {stderr:?}"));
    assert!(error["error"]
        .as_str()
        .unwrap_or_default()
        .contains("Sway IPC"));
}

#[test]
fn debug_events_conflicts_with_a_per_window_flag() {
    let output = sway_launch_without_a_compositor()
        .args(["--debug-events", "--con-id", "42", "--floating"])
        .output()
        .expect("failed to run sway-launch binary");

    assert_conflict_error(&output, "--debug-events", "--floating");
}

#[test]
fn debug_events_rejects_the_template_and_layout_only_flags() {
    // main() dispatches --debug-events before its manual "requires
    // --template"/"requires --layout or --template" checks, so these three
    // have to be clap conflicts or they'd be silently discarded (#99).
    for flag in [
        ["--bindings", "bindings.toml"].as_slice(),
        ["--apps", "foot"].as_slice(),
        ["--rollback-on-error"].as_slice(),
    ] {
        let output = sway_launch_without_a_compositor()
            .arg("--debug-events")
            .args(flag)
            .output()
            .expect("failed to run sway-launch binary");

        assert_conflict_error(&output, "--debug-events", flag[0]);
    }
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

#[test]
fn cli_rejects_a_sway_keyword_as_a_workspace_name_before_running() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42", "--workspace", "NEXT"])
        .output()
        .expect("failed to run sway-launch binary");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("is a Sway keyword"), "{stderr}");
}

#[test]
fn list_windows_conflicts_with_a_per_window_flag_but_not_a_filter() {
    let output = sway_launch_without_a_compositor()
        .args(["--list-windows", "--floating"])
        .output()
        .expect("failed to run sway-launch binary");
    assert_conflict_error(&output, "--list-windows", "--floating");

    // A filter is accepted, so the run gets as far as the socket.
    let output = sway_launch_without_a_compositor()
        .args(["--list-windows", "--mark-match", "dropdown", "--json"])
        .output()
        .expect("failed to run sway-launch binary");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    let error: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be a JSON object");
    assert!(error["error"]
        .as_str()
        .unwrap_or_default()
        .contains("Sway IPC"));
}

#[test]
fn workspace_number_moves_by_number_in_the_plan() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42", "--workspace-number", "2", "--dry-run"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        "1. target existing container\n2. move workspace number 2"
    );
}

#[test]
fn workspace_number_conflicts_with_workspace_and_relative_moves() {
    for flag in ["--workspace", "--new-column", "--new-row"] {
        let mut arguments = vec!["--con-id", "42", "--workspace-number", "2", flag];
        if flag == "--workspace" {
            arguments.push("web");
        }
        let output = sway_launch_without_a_compositor()
            .args(&arguments)
            .output()
            .expect("failed to run sway-launch binary");
        assert_eq!(output.status.code(), Some(2), "{flag}: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cannot be used with") && stderr.contains("--workspace-number"),
            "{flag}: {stderr}"
        );
    }
}

#[test]
fn workspace_number_rejects_a_negative_or_non_numeric_value() {
    for value in ["-1", "web"] {
        let output = sway_launch_without_a_compositor()
            .args(["--con-id", "42", "--workspace-number", value])
            .output()
            .expect("failed to run sway-launch binary");
        assert_eq!(output.status.code(), Some(2), "{value}: {output:?}");
    }
}

#[test]
fn on_workspace_number_requires_layout_or_template_and_conflicts_with_on_workspace() {
    let output = sway_launch_without_a_compositor()
        .args(["--con-id", "42", "--on-workspace-number", "2"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--on-workspace-number requires --layout or --template"));

    let output = sway_launch_without_a_compositor()
        .args([
            "--template",
            "quad-grid",
            "--apps",
            "a,b,c,d",
            "--on-workspace",
            "web",
            "--on-workspace-number",
            "2",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert_conflict_error(&output, "--on-workspace", "--on-workspace-number");
}

#[test]
fn dry_run_says_an_existing_wait_waits_for_the_window() {
    let output = sway_launch_without_a_compositor()
        .args(["--existing", "--wait", "-a", "firefox", "--dry-run"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "1. wait for existing window (app_id=\"firefox\")"
    );
}

#[test]
fn dry_run_names_a_title_alongside_the_matcher() {
    let output = sway_launch_without_a_compositor()
        .args([
            "--existing",
            "-a",
            "code",
            "--title",
            "^project",
            "--dry-run",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "1. target existing window (app_id=\"code\", title=~\"^project\")"
    );
}

#[test]
fn an_invalid_title_regex_is_a_usage_error() {
    let output = sway_launch_without_a_compositor()
        .args(["--title", "(unclosed", "foot"])
        .output()
        .expect("failed to run sway-launch binary");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a valid regular expression"));
}
