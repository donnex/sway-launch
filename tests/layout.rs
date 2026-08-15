// con_id-only steps never touch the Sway IPC socket, so these exercise
// --layout end to end (file reading, TOML parsing, step iteration, output
// formatting) against the compiled binary without requiring a live Sway
// session.

use std::fs;
use std::process::Command;

fn write_temp_toml(name: &str, contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("sway-launch-test-{}.toml", name));
    fs::write(&path, contents).expect("failed to write temp layout file");
    path
}

#[test]
fn layout_plain_output_prints_one_container_id_per_step() {
    let path = write_temp_toml(
        "plain-output",
        "[[step]]\ncon_id = 42\n\n[[step]]\ncon_id = 91\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(stdout, "42\n91\n");
}

#[test]
fn layout_json_output_is_a_single_array() {
    let path = write_temp_toml(
        "json-output",
        "[[step]]\ncon_id = 42\n\n[[step]]\ncon_id = 91\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(stdout.trim(), "{\"container_ids\":[42,91]}");
}

#[test]
fn layout_conflicts_with_a_per_window_flag() {
    let path = write_temp_toml("conflicts", "[[step]]\ncon_id = 42\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--floating"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn layout_missing_file_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", "/nonexistent-sway-launch-test-layout.toml"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("nonexistent-sway-launch-test-layout.toml"));
}

#[test]
fn layout_malformed_toml_errors() {
    let path = write_temp_toml("malformed", "this is not toml [[[");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn layout_step_without_a_target_errors_naming_the_step() {
    let path = write_temp_toml("no-target", "[[step]]\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("step 1"));
}
