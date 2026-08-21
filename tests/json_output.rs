// --con-id alone never touches the Sway IPC socket (it's returned directly,
// no connection needed), so these exercise main()'s actual output
// formatting against the compiled binary without requiring a live Sway
// session.

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
    assert_eq!(stdout.trim(), "{\"container_id\":42}");
}

#[test]
fn con_id_json_error_output_is_a_structured_object() {
    // --con-id alone never touches the socket, but combining it with an
    // action flag does (already_at_target()'s find_container_node() call),
    // which reliably fails headlessly since there's no Sway instance
    // running here — exercises --json's error-output shape
    // (fail()/fail_with_rollback() in main.rs) without needing a live
    // compositor.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42", "--floating", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    // fail_with_rollback() always writes our JSON as the last line, but on a
    // machine with a real `sway` binary on PATH and no live compositor,
    // swayipc's own socket-path fallback (`sway --get-socketpath`) prints an
    // inherited "sway socket not detected." diagnostic to stderr first — so
    // check the last line rather than requiring stderr to start with '{'.
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
