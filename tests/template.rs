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
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n\n[[step]]\nslot = \"terminal\"\n",
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
fn template_dry_run_prints_a_continuously_numbered_plan_via_apps() {
    let template = TempToml::write(
        "dry-run-apps-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\nsplit = \"h\"\n\n[[step]]\nslot = \"terminal\"\nnew_column = true\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--apps",
            "code,foot",
            "--dry-run",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "sway-launch --template --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout,
        "1. launch code\n2. splith\n3. launch foot\n4. move right\n"
    );
}

#[test]
fn template_resolves_a_binding_matched_by_mark_via_dry_run() {
    let template = TempToml::write(
        "mark-match-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"terminal\"\n",
    );
    let bindings = TempToml::write(
        "mark-match-bindings",
        "[[binding]]\nslot = \"terminal\"\nexisting = true\nmark_match = \"dropdown-term\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            bindings.to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout,
        "1. target existing window (mark_match=\"dropdown-term\")\n"
    );
}

#[test]
fn template_dry_run_resolves_target_id_without_launching_anything() {
    let template = TempToml::write(
        "dry-run-target-id-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n\n[[step]]\ntarget_id = \"editor\"\nwidth = \"70ppt\"\n",
    );
    let bindings = TempToml::write(
        "dry-run-target-id-bindings",
        "[[binding]]\nslot = \"editor\"\ncommand = \"code\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            bindings.to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "sway-launch --template --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout,
        "1. launch code\n2. target existing container\n3. resize set width 70ppt\n"
    );
}

#[test]
fn template_layout_context_applies_workspace_and_output_to_a_step_without_its_own() {
    let template = TempToml::write(
        "layout-context-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [layout]\nworkspace = \"3\"\noutput = \"HDMI-A-1\"\n\n\
         [[step]]\nslot = \"editor\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--apps",
            "code",
            "--dry-run",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "sway-launch --template --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout,
        "1. launch code\n2. move workspace \"3\"\n3. move container to output \"HDMI-A-1\"\n"
    );
}

#[test]
fn template_layout_context_does_not_override_a_step_s_own_workspace_and_output() {
    let template = TempToml::write(
        "layout-context-override-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [layout]\nworkspace = \"3\"\noutput = \"HDMI-A-1\"\n\n\
         [[step]]\nslot = \"editor\"\nworkspace = \"5\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--apps",
            "code",
            "--dry-run",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout,
        "1. launch code\n2. move workspace \"5\"\n3. move container to output \"HDMI-A-1\"\n"
    );
}

#[test]
fn template_dry_run_json_output_is_a_structured_steps_array() {
    let template = TempToml::write(
        "dry-run-json-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\nfocus = true\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--apps",
            "code",
            "--dry-run",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        "{\"steps\":[{\"actions\":[\"focus\"],\"target\":\"launch code\"}]}"
    );
}

#[test]
fn template_validate_reports_success_without_launching_anything() {
    let template = TempToml::write(
        "validate-ok-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n\n[[step]]\nslot = \"terminal\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--apps",
            "code,foot",
            "--validate",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "sway-launch --template --validate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        format!("valid: {} (2 step(s))", template.display())
    );
}

#[test]
fn template_validate_reports_a_step_error() {
    let template = TempToml::write(
        "validate-bad-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\nheight = \"notasize\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--apps",
            "code",
            "--validate",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("step 1"));
    assert!(stderr.contains("height"));
}

#[test]
fn template_target_id_references_an_earlier_slot() {
    let template = TempToml::write(
        "target-id-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n\n[[step]]\ntarget_id = \"editor\"\n",
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
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n\n[[step]]\nslot = \"terminal\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--template", template.to_str().unwrap(), "--apps", "foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("editor"));
    assert!(stderr.contains("terminal"));
}

#[test]
fn template_rejects_two_steps_sharing_a_slot_name() {
    // Two steps sharing a slot are rejected by resolve() itself, with a
    // template-shaped error naming the slot -- this used to be left for
    // run_steps()'s generic "id already used by an earlier step" check to
    // catch instead (an implementation detail, not the actual authoring
    // mistake), so the error text this asserts on changed accordingly.
    let template = TempToml::write(
        "duplicate-slot-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n\n[[step]]\nslot = \"editor\"\n",
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
    assert!(stderr.contains("slot"));
    assert!(stderr.contains("editor"));
    assert!(stderr.contains("more than once"));
}

#[test]
fn template_rejects_missing_binding() {
    let template = TempToml::write("missing-binding-template", "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n");
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
    let template = TempToml::write("unused-binding-template", "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n");
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
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\nflaoting = true\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--template", template.to_str().unwrap(), "--apps", "foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn template_malformed_toml_errors() {
    let path = TempToml::write("malformed", "this is not toml [[[");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--template", path.to_str().unwrap(), "--apps", "foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn template_missing_file_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            "/nonexistent-sway-launch-test-template.toml",
            "--apps",
            "foot",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("nonexistent-sway-launch-test-template.toml"));
}

#[test]
fn template_bindings_malformed_toml_errors() {
    let template = TempToml::write(
        "malformed-bindings-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n",
    );
    let bindings = TempToml::write("malformed-bindings-bindings", "this is not toml [[[");

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
}

#[test]
fn template_bindings_missing_file_errors() {
    let template = TempToml::write("missing-bindings-template", "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            "/nonexistent-sway-launch-test-bindings.toml",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("nonexistent-sway-launch-test-bindings.toml"));
}

#[test]
fn template_requires_bindings_or_apps() {
    let template = TempToml::write("requires-binding-source", "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--template", template.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("--bindings"));
    assert!(stderr.contains("--apps"));
}

#[test]
fn template_apps_without_template_flag_errors() {
    // Regression test: --apps combined with --con-id (no --template) used to
    // parse cleanly and silently fall through to the ordinary --con-id
    // dispatch, discarding --apps entirely instead of erroring.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--apps", "foot", "--con-id", "1"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("--template"));
}

#[test]
fn template_bindings_without_template_flag_errors() {
    let bindings = TempToml::write(
        "bindings-without-template",
        "[[binding]]\nslot = \"editor\"\ncon_id = 1\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--bindings", bindings.to_str().unwrap(), "--con-id", "1"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("--template"));
}

#[test]
fn template_apps_rejects_an_empty_entry() {
    let template = TempToml::write(
        "apps-empty-entry-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n\n[[step]]\nslot = \"terminal\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--template", template.to_str().unwrap(), "--apps", "foot,"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("terminal"));
}

#[test]
fn template_rejects_binding_with_app_id_and_class_together() {
    let template = TempToml::write(
        "binding-app-id-and-class-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n",
    );
    let bindings = TempToml::write(
        "binding-app-id-and-class-bindings",
        "[[binding]]\nslot = \"editor\"\ncommand = \"foot\"\napp_id = \"foot\"\nclass = \"Foot\"\n",
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
    assert!(stderr.contains("app_id"));
    assert!(stderr.contains("class"));
}

#[test]
fn template_builtin_name_is_found_and_parsed_without_a_toml_extension() {
    // Deliberately mismatches --apps' count against quad-grid's real 4
    // slots rather than trying to fully run() it (its steps set split
    // actions, which would need a real Sway socket) — the resulting error
    // naming all 4 real slot names only happens if the embedded
    // quad-grid.toml content was actually found and parsed via the bare
    // name, proving the lookup itself works, while staying headless-safe:
    // bindings_from_apps() fails before run_steps() ever touches IPC.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--template", "quad-grid", "--apps", "foot,foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    for slot in ["top-left", "top-right", "bottom-left", "bottom-right"] {
        assert!(
            stderr.contains(slot),
            "stderr should name slot {slot:?}: {stderr}"
        );
    }
}

#[test]
fn template_unknown_builtin_name_errors_clearly() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--template", "not-a-real-template", "--apps", "foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("not-a-real-template"));
    assert!(stderr.contains("--list-templates"));
}

#[test]
fn template_toml_suffixed_name_is_never_treated_as_a_builtin() {
    // A bare name with no extension that happens to also not exist as a
    // built-in (checked above) is one failure mode; this checks the
    // opposite direction — a nonexistent .toml path must fail as a file
    // read, not silently fall back to a built-in lookup, even though
    // "quad-grid" itself is a real built-in name.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--template", "quad-grid.toml", "--apps", "foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("quad-grid.toml"));
    assert!(!stderr.contains("--list-templates"));
}

#[test]
fn template_file_named_dot_toml_is_read_as_a_file_not_a_builtin_lookup() {
    // Regression test: Path::extension() returns None for a filename that's
    // only an extension with nothing before it (e.g. a file literally named
    // ".toml"), which used to misclassify this as a built-in-name lookup
    // instead of a file read, erroring with "no built-in template named
    // \".toml\"" even when the file existed on disk. resolve_template_contents()
    // now checks the string suffix directly instead.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--template", ".toml", "--apps", "foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains(".toml"));
    assert!(!stderr.contains("no built-in template named"));
}

#[test]
fn list_templates_prints_known_names_and_descriptions() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--list-templates"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert!(stdout.contains("quad-grid"));
    assert!(stdout
        .contains("An equally sized 2x2 grid, two rows of two, with no application baked in."));
    assert!(stdout.contains("Grid"));
    assert!(stdout.contains("sidebar-left-dual-stack"));
}

#[test]
fn list_templates_json_output_is_a_structured_array() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--list-templates", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("output should be valid JSON");
    let templates = parsed["templates"]
        .as_array()
        .expect("templates should be an array");
    assert!(templates.len() >= 18);
    assert!(templates.iter().any(|entry| entry["name"] == "quad-grid"
        && entry["description"].is_string()
        && entry["category"] == "Grid"));
}

#[test]
fn list_templates_json_output_reports_slots_and_slot_names() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--list-templates", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("output should be valid JSON");
    let templates = parsed["templates"]
        .as_array()
        .expect("templates should be an array");
    let quad_grid = templates
        .iter()
        .find(|entry| entry["name"] == "quad-grid")
        .expect("quad-grid should be a built-in template");
    assert_eq!(quad_grid["slots"], 4);
    assert_eq!(
        quad_grid["slot_names"],
        serde_json::json!(["top-left", "top-right", "bottom-left", "bottom-right"])
    );
}

#[test]
fn list_templates_plain_output_includes_slot_count_and_names() {
    // Per user decision 2026-08-22: plain output also reports slot info,
    // matching --json rather than staying description-only. Checks
    // quad-grid's own line specifically (rather than scanning the whole
    // listing for the substring "slot", which would false-positive on
    // retarget-by-slot's name/description).
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--list-templates"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let quad_grid_line = stdout
        .lines()
        .find(|line| line.starts_with("quad-grid "))
        .expect("quad-grid should be listed");
    assert!(quad_grid_line.contains("(4 slots: top-left, top-right, bottom-left, bottom-right)"));
}

#[test]
fn list_templates_plain_output_uses_singular_slot_for_a_single_slot_template() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--list-templates"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let floating_centered_line = stdout
        .lines()
        .find(|line| line.starts_with("floating-centered "))
        .expect("floating-centered should be listed");
    assert!(floating_centered_line.contains("(1 slot: window)"));
    assert!(!floating_centered_line.contains("1 slots"));
}

#[test]
fn show_template_prints_a_builtin_s_raw_toml() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--show-template", "quad-grid"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert!(stdout.contains("quad-terminals.toml's shape."));
    assert!(stdout.contains("slot = \"top-left\""));
}

#[test]
fn show_template_prints_a_file_s_raw_toml() {
    let template = TempToml::write("show-template-file", "[[step]]\nslot = \"editor\"\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--show-template", template.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(stdout, "[[step]]\nslot = \"editor\"\n");
}

#[test]
fn show_template_json_output_is_a_structured_object() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--show-template", "quad-grid", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("output should be valid JSON");
    assert_eq!(parsed["name"], "quad-grid");
    assert!(parsed["contents"]
        .as_str()
        .expect("contents should be a string")
        .contains("slot = \"top-left\""));
}

#[test]
fn show_template_unknown_builtin_name_errors_clearly() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--show-template", "not-a-real-template"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("not-a-real-template"));
    assert!(stderr.contains("--list-templates"));
}

#[test]
fn show_template_conflicts_with_a_command() {
    // --show-template is a standalone mode, same as --list-templates: it
    // must never launch/act on any window, so it conflicts with a positional
    // command outright rather than silently ignoring one.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--show-template", "quad-grid", "foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn show_template_conflicts_with_template() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--show-template", "quad-grid", "--template", "quad-grid"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}

#[test]
fn template_conflicts_with_layout() {
    let template = TempToml::write("conflicts-template", "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--apps",
            "foot",
            "--layout",
            template.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
}
