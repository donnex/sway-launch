// con_id-only steps never touch the Sway IPC socket, so these exercise
// --layout end to end (file reading, TOML parsing, step iteration, output
// formatting) against the compiled binary without requiring a live Sway
// session.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A layout file fixture written to the OS temp directory, removed again
/// when it goes out of scope (even if an assertion panics mid-test).
struct TempToml(PathBuf);

impl TempToml {
    fn write(name: &str, contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sway-launch-test-{}.toml", name));
        fs::write(&path, contents).expect("failed to write temp layout file");
        Self(path)
    }
}

impl Deref for TempToml {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempToml {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn layout_plain_output_prints_one_container_id_per_step() {
    let path = TempToml::write(
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
    let path = TempToml::write(
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
    let path = TempToml::write("conflicts", "[[step]]\ncon_id = 42\n");

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
    let path = TempToml::write("malformed", "this is not toml [[[");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn layout_step_without_a_target_errors_naming_the_step() {
    let path = TempToml::write("no-target", "[[step]]\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("step 1"));
}

#[test]
fn layout_rejects_misspelled_field() {
    let path = TempToml::write(
        "misspelled-field",
        "[[step]]\ncon_id = 42\nflaoting = true\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn layout_rejects_command_and_con_id_together() {
    let path = TempToml::write(
        "command-and-con-id",
        "[[step]]\ncommand = \"kitty\"\ncon_id = 42\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("step 1"));
}

#[test]
fn layout_target_id_resolves_to_an_earlier_steps_container_id() {
    let path = TempToml::write(
        "target-id",
        "[[step]]\nid = \"first\"\ncon_id = 42\n\n[[step]]\ntarget_id = \"first\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(stdout, "42\n42\n");
}

#[test]
fn layout_rejects_unresolved_target_id() {
    let path = TempToml::write(
        "unresolved-target-id",
        "[[step]]\ntarget_id = \"missing\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("missing"));
}

#[test]
fn layout_rejects_duplicate_step_ids() {
    let path = TempToml::write(
        "duplicate-id",
        "[[step]]\nid = \"first\"\ncon_id = 42\n\n[[step]]\nid = \"first\"\ncon_id = 91\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("step 2"));
    assert!(stderr.contains("first"));
}
