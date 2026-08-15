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
