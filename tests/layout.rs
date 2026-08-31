// con_id-only steps never touch the Sway IPC socket, so these exercise
// --layout end to end (file reading, TOML parsing, step iteration, output
// formatting) against the compiled binary without requiring a live Sway
// session.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

/// A layout file fixture written to the OS temp directory, removed again
/// when it goes out of scope (even if an assertion panics mid-test).
struct TempToml(PathBuf);

impl TempToml {
    fn write(name: &str, contents: &str) -> Self {
        // Scoped to a per-process directory rather than written straight
        // into the shared temp dir under a fixed name. Two of these test
        // files independently used the fixture name "malformed", so both
        // wrote the same path -- harmless while cargo runs test binaries one
        // at a time, but a real collision under a parallel runner (cargo
        // nextest), under two concurrent `cargo test --test ...` runs, or
        // with two users on one machine. A predictable path in a shared
        // /tmp is also writable through a symlink planted by another user.
        let directory = std::env::temp_dir().join(format!("sway-launch-test-{}", process::id()));
        fs::create_dir_all(&directory).expect("failed to create temp fixture directory");
        let path = directory.join(format!("{}.toml", name));
        fs::write(&path, contents).expect("failed to write temp fixture file");
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
fn layout_dry_run_prints_a_continuously_numbered_plan() {
    let path = TempToml::write(
        "dry-run",
        "[[step]]\ncommand = \"code\"\nsplit = \"h\"\n\n[[step]]\ncon_id = 91\nfloating = true\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--dry-run"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "sway-launch --layout --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout,
        "1. launch code\n2. splith\n3. target existing container\n4. floating enable\n"
    );
}

#[test]
fn layout_dry_run_describes_an_existing_step_matched_by_mark() {
    let path = TempToml::write(
        "dry-run-mark-match",
        "[[step]]\nexisting = true\nmark_match = \"dropdown-term\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--dry-run"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "sway-launch --layout --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout,
        "1. target existing window (mark_match=\"dropdown-term\")\n"
    );
}

#[test]
fn layout_rejects_app_id_and_mark_match_together() {
    let path = TempToml::write(
        "app-id-and-mark-match",
        "[[step]]\nexisting = true\napp_id = \"foot\"\nmark_match = \"dropdown-term\"\n",
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
fn layout_dry_run_resolves_target_id_without_launching_anything() {
    // The interesting case: a target_id step references an earlier step's
    // id, which normally resolves to a real container id run_steps()
    // learned by actually launching that step — --dry-run never launches
    // anything, so to_sway_launch()'s target_id lookup has to resolve
    // against a synthetic placeholder instead. This confirms that
    // resolution still succeeds (no error) rather than failing with
    // "target_id not found".
    let path = TempToml::write(
        "dry-run-target-id",
        "[[step]]\ncommand = \"code\"\nid = \"editor\"\n\n[[step]]\ntarget_id = \"editor\"\nwidth = \"70ppt\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--dry-run"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "sway-launch --layout --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout,
        "1. launch code\n2. target existing container\n3. resize set width 70ppt\n"
    );
}

#[test]
fn layout_dry_run_json_output_is_a_structured_steps_array() {
    let path = TempToml::write("dry-run-json", "[[step]]\ncon_id = 42\nfocus = true\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--dry-run", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        "{\"steps\":[{\"actions\":[\"focus\"],\"target\":\"target existing container\"}]}"
    );
}

#[test]
fn layout_dry_run_reports_a_step_error_without_running_earlier_steps() {
    let path = TempToml::write(
        "dry-run-step-error",
        "[[step]]\ncon_id = 42\n\n[[step]]\nheight = \"notasize\"\ncon_id = 91\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--dry-run"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("step 2"));
    assert!(stderr.contains("height"));
}

#[test]
fn layout_validate_reports_success_without_launching_anything() {
    let path = TempToml::write(
        "validate-ok",
        "[[step]]\ncommand = \"code\"\nid = \"editor\"\n\n[[step]]\ntarget_id = \"editor\"\nwidth = \"70ppt\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--validate"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "sway-launch --layout --validate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        format!("valid: {} (2 step(s))", path.display())
    );
}

#[test]
fn layout_validate_json_output_is_a_structured_object() {
    let path = TempToml::write("validate-ok-json", "[[step]]\ncon_id = 42\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--validate", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        format!(
            "{{\"source\":\"{}\",\"steps\":1,\"valid\":true}}",
            path.display()
        )
    );
}

#[test]
fn layout_validate_reports_a_step_error() {
    let path = TempToml::write(
        "validate-bad",
        "[[step]]\ncon_id = 42\n\n[[step]]\nheight = \"notasize\"\ncon_id = 91\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--validate"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("step 2"));
    assert!(stderr.contains("height"));
}

#[test]
fn layout_validate_json_error_output_is_a_structured_object() {
    let path = TempToml::write(
        "validate-bad-json",
        "[[step]]\nheight = \"notasize\"\ncon_id = 42\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--validate", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.starts_with('{') && stderr.contains("\"error\""));
    assert!(stderr.contains("step 1"));
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
    assert_eq!(
        stdout.trim(),
        "{\"actions\":[],\"container_ids\":[42,91],\"containers\":{}}"
    );
}

#[test]
fn layout_json_output_maps_named_steps_in_containers() {
    let path = TempToml::write(
        "json-output-containers",
        "[[step]]\ncon_id = 42\nid = \"editor\"\n\n[[step]]\ncon_id = 91\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        "{\"actions\":[],\"container_ids\":[42,91],\"containers\":{\"editor\":42}}"
    );
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
fn layout_conflicts_with_stray_apps_flag() {
    // Regression test: --apps combined with --layout used to parse cleanly
    // and silently discard --apps instead of erroring, since --layout wasn't
    // listed in --apps'/--bindings' conflicts (unlike --completions and
    // --list-templates, which already list both for the same reason).
    let path = TempToml::write("stray-apps", "[[step]]\ncon_id = 42\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--apps", "foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn layout_conflicts_with_stray_bindings_flag() {
    let layout_path = TempToml::write("stray-bindings-layout", "[[step]]\ncon_id = 42\n");
    let bindings_path =
        TempToml::write("stray-bindings", "[[binding]]\nslot = \"a\"\ncon_id = 1\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--layout",
            layout_path.to_str().unwrap(),
            "--bindings",
            bindings_path.to_str().unwrap(),
        ])
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
fn layout_with_no_steps_errors() {
    // Every layout field is optional, so a file with no [[step]] blocks
    // parses fine -- and used to "succeed" silently: exit 0, no output,
    // nothing launched, indistinguishable from a run that worked.
    let path = TempToml::write("no-steps", "");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.contains("no steps found") && stderr.contains("no-steps"),
        "the error should say what was wrong and name the file: {stderr:?}"
    );
}

#[test]
fn layout_with_no_steps_errors_under_validate_too() {
    // --validate exists to catch a broken layout without a Sway session, so
    // it must agree with a real run rather than reporting "valid: 0 step(s)".
    let path = TempToml::write("no-steps-validate", "");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--validate"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("no steps found"));
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
        "[[step]]\ncommand = \"foot\"\ncon_id = 42\n",
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
fn layout_rejects_app_id_and_class_together() {
    let path = TempToml::write(
        "app-id-and-class",
        "[[step]]\ncon_id = 42\napp_id = \"foot\"\nclass = \"Foot\"\n",
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
fn layout_rejects_target_id_and_app_id_together() {
    let path = TempToml::write(
        "target-id-and-app-id",
        "[[step]]\nid = \"first\"\ncon_id = 42\n\n\
         [[step]]\ntarget_id = \"first\"\napp_id = \"foot\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("step 2"));
    assert!(stderr.contains("target_id"));
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

#[test]
fn layout_json_error_output_is_a_structured_object() {
    let path = TempToml::write(
        "json-error",
        "[[step]]\nid = \"first\"\ncon_id = 42\n\n[[step]]\nid = \"first\"\ncon_id = 91\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.trim_start().starts_with('{') && stderr.contains("\"error\""),
        "expected a JSON error object, got {stderr:?}"
    );
    assert!(stderr.contains("step 2"));
}

#[test]
fn layout_json_error_output_reports_the_steps_that_already_completed() {
    // Plain output prints each step's container id as that step finishes, so
    // a mid-layout failure still leaves the caller holding every id. --json
    // collects them for one object at the end instead, so before this the
    // error object named nothing that had already run -- leaving a caller
    // with real windows open and no way to identify them. Steps 1 and 2 here
    // are con_id-only (headless-safe, never touching the socket); step 3
    // fails on a duplicate id.
    let path = TempToml::write(
        "json-error-progress",
        "[[step]]\nid = \"first\"\ncon_id = 42\n\n[[step]]\ncon_id = 91\n\n\
         [[step]]\nid = \"first\"\ncon_id = 7\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", path.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    let error: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|_| panic!("stderr should be a JSON object, got {stderr:?}"));
    assert_eq!(error["container_ids"], serde_json::json!([42, 91]));
    assert_eq!(error["containers"], serde_json::json!({ "first": 42 }));
    assert!(
        error["error"]
            .as_str()
            .expect("error should be a string")
            .contains("step 3"),
        "the reported error should still be the step failure: {error:?}"
    );
}

#[test]
fn layout_json_error_output_omits_progress_for_a_single_invocation() {
    // The partial-progress fields are a multi-step concept; a single
    // invocation either resolved its one container or didn't, so reporting
    // an empty array there would suggest a run that got nowhere rather than
    // one the concept never applied to.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--con-id", "42", "--floating", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    let last_line = stderr.lines().next_back().unwrap_or_default();
    let error: serde_json::Value = serde_json::from_str(last_line)
        .unwrap_or_else(|_| panic!("the last stderr line should be JSON, got {stderr:?}"));
    assert!(error.get("container_ids").is_none());
    assert!(error.get("containers").is_none());
}

#[test]
fn layout_rollback_on_error_reports_empty_rollback_when_nothing_was_launched() {
    // con_id-only steps are never rolled back (they retarget a pre-existing
    // window, not one this invocation launched) — proves --rollback-on-error
    // doesn't error/panic and reports an empty rollback list in that case,
    // headlessly. The actual "kills a real launched window" behavior needs
    // a live Sway session — see tests/live_sway.rs's
    // rollback_on_error_kills_earlier_launched_windows_when_a_later_step_fails.
    let path = TempToml::write(
        "rollback-empty",
        "[[step]]\nid = \"first\"\ncon_id = 42\n\n[[step]]\nid = \"first\"\ncon_id = 91\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.contains("\"rolled_back\":[]"),
        "expected an empty rollback list since neither step launched a window: {stderr:?}"
    );
}
