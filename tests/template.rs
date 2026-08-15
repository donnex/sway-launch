// con_id-based bindings never touch the Sway IPC socket, so these exercise
// --template end to end (file reading, TOML parsing, resolution against
// --bindings/--apps, step iteration, output formatting) against the compiled
// binary without requiring a live Sway session — mirrors tests/layout.rs.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A TOML file fixture written to the OS temp directory, removed again when
/// it goes out of scope (even if an assertion panics mid-test). Used for
/// both template and bindings files.
struct TempToml(PathBuf);

impl TempToml {
    fn write(name: &str, contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sway-launch-test-{}.toml", name));
        fs::write(&path, contents).expect("failed to write temp file");
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
fn template_resolves_via_bindings_file_end_to_end() {
    let template = TempToml::write(
        "resolve-bindings-template",
        "[[step]]\nslot = \"editor\"\n\n[[step]]\nslot = \"terminal\"\n",
    );
    let bindings = TempToml::write(
        "resolve-bindings-bindings",
        "[[binding]]\nslot = \"editor\"\ncon_id = 42\n\n\
         [[binding]]\nslot = \"terminal\"\ncon_id = 91\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            bindings.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(stdout, "42\n91\n");
}

#[test]
fn template_target_id_references_an_earlier_slot() {
    let template = TempToml::write(
        "target-id-template",
        "[[step]]\nslot = \"editor\"\n\n[[step]]\ntarget_id = \"editor\"\n",
    );
    let bindings = TempToml::write(
        "target-id-bindings",
        "[[binding]]\nslot = \"editor\"\ncon_id = 42\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            bindings.to_str().unwrap(),
        ])
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
fn template_apps_count_mismatch_names_the_missing_slots() {
    let template = TempToml::write(
        "apps-mismatch-template",
        "[[step]]\nslot = \"editor\"\n\n[[step]]\nslot = \"terminal\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--template", template.to_str().unwrap(), "--apps", "kitty"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("editor"));
    assert!(stderr.contains("terminal"));
}

#[test]
fn template_rejects_two_steps_sharing_a_slot_name() {
    // Two steps sharing a slot both resolve to id = the slot name, so this
    // trips run_steps()'s existing duplicate-id check — proving templates
    // reuse that mechanism rather than needing their own.
    let template = TempToml::write(
        "duplicate-slot-template",
        "[[step]]\nslot = \"editor\"\n\n[[step]]\nslot = \"editor\"\n",
    );
    let bindings = TempToml::write(
        "duplicate-slot-bindings",
        "[[binding]]\nslot = \"editor\"\ncon_id = 42\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            bindings.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("step 2"));
    assert!(stderr.contains("editor"));
}

#[test]
fn template_rejects_missing_binding() {
    let template = TempToml::write("missing-binding-template", "[[step]]\nslot = \"editor\"\n");
    let bindings = TempToml::write("missing-binding-bindings", "");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            bindings.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("editor"));
}

#[test]
fn template_rejects_unused_binding() {
    let template = TempToml::write("unused-binding-template", "[[step]]\nslot = \"editor\"\n");
    let bindings = TempToml::write(
        "unused-binding-bindings",
        "[[binding]]\nslot = \"editor\"\ncon_id = 42\n\n\
         [[binding]]\nslot = \"terminal\"\ncon_id = 91\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            bindings.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("terminal"));
}

#[test]
fn template_rejects_misspelled_step_field() {
    let template = TempToml::write(
        "misspelled-field-template",
        "[[step]]\nslot = \"editor\"\nflaoting = true\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--template", template.to_str().unwrap(), "--apps", "kitty"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn template_conflicts_with_layout() {
    let template = TempToml::write("conflicts-template", "[[step]]\nslot = \"editor\"\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--apps",
            "kitty",
            "--layout",
            template.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}
