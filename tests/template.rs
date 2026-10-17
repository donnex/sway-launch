// con_id-based bindings never touch the Sway IPC socket, so these exercise
// --template end to end (file reading, TOML parsing, resolution against
// --bindings/--apps, step iteration, output formatting) against the compiled
// binary without requiring a live Sway session — mirrors tests/layout.rs.

mod common;

use common::{assert_conflict_error, run_with_stdin, TempToml};
use std::process::Command;

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
fn template_layout_context_switches_to_its_output_and_workspace_before_the_first_step() {
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
        "1. focus output \"HDMI-A-1\"\n2. workspace \"3\"\n3. launch code\n"
    );
}

#[test]
fn template_layout_context_leaves_a_step_s_own_workspace_move_in_place() {
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
        "1. focus output \"HDMI-A-1\"\n2. workspace \"3\"\n3. launch code\n4. move workspace \"5\"\n"
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
        "{\"steps\":[{\"actions\":[{\"command\":\"focus\",\"kind\":\"focus\"}],\"target\":\"launch code\"}]}"
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
    assert!(
        stderr.contains(&format!("{}: step 1: height", template.display())),
        "a step error should name the template file and the step: {stderr:?}"
    );
}

#[test]
fn template_validate_rejects_a_control_character_in_a_workspace() {
    let template = TempToml::write(
        "validate-control-character-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\nworkspace = \"\\u001b[31mred\"\n",
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
    assert!(stderr.contains("step 1"), "{stderr:?}");
    assert!(stderr.contains("control character"), "{stderr:?}");
    assert!(
        !stderr.contains('\u{1b}'),
        "raw escape in output: {stderr:?}"
    );
}

#[test]
fn template_validate_blames_the_layout_table_not_a_step_for_its_invalid_workspace() {
    let template = TempToml::write(
        "validate-layout-context-blame-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [layout]\nworkspace = \"a\\\"b\"\n\n\
         [[step]]\nslot = \"editor\"\n\n[[step]]\nslot = \"terminal\"\n",
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

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("[layout] workspace: "), "{stderr:?}");
    assert!(!stderr.contains("step 1"), "{stderr:?}");
}

#[test]
fn template_validate_rejects_a_blank_layout_output_that_every_step_overrides() {
    let template = TempToml::write(
        "validate-layout-context-overridden-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [layout]\noutput = \"  \"\n\n\
         [[step]]\nslot = \"editor\"\noutput = \"HDMI-A-1\"\n\n\
         [[step]]\nslot = \"terminal\"\noutput = \"HDMI-A-1\"\n",
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

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("[layout] output: "), "{stderr:?}");
}

#[test]
fn template_parse_error_does_not_echo_a_raw_terminal_escape() {
    // TOML forbids a raw control character in a comment, so the parse fails
    // and the error quotes the offending line — which must not reach the
    // terminal as a live escape sequence (here: set title, clear screen).
    let template = TempToml::write(
        "parse-error-escape-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n# hi \u{1b}]0;PWNED\u{7} \u{1b}[2J\n\n[[step]]\nslot = \"editor\"\n",
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
    assert!(
        !stderr.chars().any(|c| c.is_control() && c != '\n'),
        "raw control character in output: {stderr:?}"
    );
    assert!(stderr.contains("\\u{1b}"), "{stderr:?}");
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
    assert!(stderr.contains("--apps: "), "{stderr:?}");
}

#[test]
fn template_rejects_two_steps_sharing_a_slot_name() {
    // Two steps sharing a slot are rejected as soon as the template is
    // parsed, with a template-shaped error naming the slot, rather than
    // left for run_steps()'s generic "id already used by an earlier step"
    // check (an implementation detail, not the actual authoring mistake).
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
fn template_rejects_two_steps_sharing_a_slot_name_before_counting_apps() {
    // With --apps the slot count is checked before resolve() runs, so a
    // repeated slot has to be caught earlier still -- otherwise the count
    // check reports a misleading "needs 1 application(s)" for a template
    // that's actually invalid.
    let template = TempToml::write(
        "duplicate-slot-apps-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n\n[[step]]\nslot = \"editor\"\n",
    );

    for apps in ["code", "code,foot"] {
        let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
            .args([
                "--template",
                template.to_str().unwrap(),
                "--apps",
                apps,
                "--validate",
            ])
            .output()
            .expect("failed to run sway-launch binary");

        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
        assert!(
            stderr.contains("editor") && stderr.contains("more than once"),
            "--apps {apps}: {stderr}"
        );
        assert!(
            !stderr.contains("application(s)"),
            "--apps {apps}: {stderr}"
        );
    }
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
    assert!(
        stderr.contains(&format!(
            "{}: binding(s) for unused slot(s)",
            bindings.display()
        )),
        "a bindings mistake should name the bindings file: {stderr:?}"
    );
    assert!(
        !stderr.contains(&template.display().to_string()),
        "a bindings mistake should not be blamed on the template: {stderr:?}"
    );
}

#[test]
fn template_bindings_mistake_for_a_builtin_names_the_bindings_file_not_the_builtin() {
    let bindings = TempToml::write("builtin-missing-binding-bindings", "");

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            "quad-grid",
            "--bindings",
            bindings.to_str().unwrap(),
            "--validate",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.contains(&format!(
            "{}: no binding for slot \"top-left\"",
            bindings.display()
        )),
        "{stderr:?}"
    );
    assert!(!stderr.contains("quad-grid:"), "{stderr:?}");
}

#[test]
fn template_step_without_slot_or_target_id_names_the_template_and_step() {
    let template = TempToml::write(
        "step-number-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [[step]]\nslot = \"editor\"\n\n[[step]]\nfloating = true\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--apps",
            "foot",
            "--validate",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.contains(&format!(
            "{}: step 2: template step must set exactly one of: slot, target_id",
            template.display()
        )),
        "{stderr:?}"
    );
}

#[test]
fn template_with_no_steps_errors() {
    // A template's steps are optional too, so one with only a [template]
    // table resolves to zero steps and used to succeed silently. Driven via
    // an empty --bindings file rather than --apps, since --apps has no way
    // to express "zero applications" (an empty string splits to one entry).
    let template = TempToml::write(
        "no-steps-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n",
    );
    let bindings = TempToml::write("no-steps-bindings", "");

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
    assert!(
        stderr.contains("no steps found"),
        "the error should say what was wrong: {stderr:?}"
    );
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
fn template_missing_builtin_name_with_toml_suffix_hints_at_the_bare_name() {
    // Still a file read (see the test above), but the error says the name
    // without its extension is a real built-in rather than leaving only a
    // bare "No such file or directory".
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            "quad-grid.toml",
            "--apps",
            "foot,foot,foot,foot",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("quad-grid.toml: "));
    assert!(stderr.contains("there is a built-in template \"quad-grid\", use --template quad-grid"));
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
    // Plain output also reports slot info,
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
fn show_template_missing_builtin_name_with_toml_suffix_hints_at_the_bare_name() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--show-template", "quad-grid.toml"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("quad-grid.toml: "));
    assert!(stderr
        .contains("there is a built-in template \"quad-grid\", use --show-template quad-grid"));
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

    assert_conflict_error(&output, "--show-template", "[COMMAND]");
}

#[test]
fn show_template_conflicts_with_template() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--show-template", "quad-grid", "--template", "quad-grid"])
        .output()
        .expect("failed to run sway-launch binary");

    assert_conflict_error(&output, "--show-template", "--template");
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

    assert_conflict_error(&output, "--template", "--layout");
}

#[test]
fn template_validate_rejects_a_binding_command_sway_would_split() {
    let template = TempToml::write(
        "split-binding-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"editor\"\n",
    );
    let bindings = TempToml::write(
        "split-binding-bindings",
        "[[binding]]\nslot = \"editor\"\ncommand = \"code; kill\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            bindings.to_str().unwrap(),
            "--validate",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    // Reported against the bindings file that holds the mistake, not as a
    // step of the template.
    assert!(
        stderr.contains("split-binding-bindings.toml: binding for slot \"editor\": command:"),
        "{stderr}"
    );
    assert!(!stderr.contains("step 1"), "{stderr}");
}

#[test]
fn on_workspace_overrides_the_template_layout_workspace_in_the_preview() {
    let template = TempToml::write(
        "on-workspace-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [layout]\nworkspace = \"3\"\n\n[[step]]\nslot = \"editor\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--apps",
            "code",
            "--on-workspace",
            "9",
            "--dry-run",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be a JSON object");
    assert_eq!(json["setup"], serde_json::json!(["workspace \"9\""]));
}

#[test]
fn workspace_number_reaches_the_preview_from_the_layout_table_and_the_flag() {
    let template = TempToml::write(
        "workspace-number-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [layout]\nworkspace_number = 3\n\n[[step]]\nslot = \"editor\"\n",
    );
    let setup = |extra: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
            .args(["--template", template.to_str().unwrap(), "--apps", "code"])
            .args(extra)
            .args(["--dry-run", "--json"])
            .output()
            .expect("failed to run sway-launch binary");
        assert!(output.status.success(), "{output:?}");
        let json: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("stdout should be a JSON object");
        json["setup"].clone()
    };

    assert_eq!(setup(&[]), serde_json::json!(["workspace number 3"]));
    assert_eq!(
        setup(&["--on-workspace-number", "9"]),
        serde_json::json!(["workspace number 9"])
    );
    assert_eq!(
        setup(&["--on-workspace", "9"]),
        serde_json::json!(["workspace \"9\""])
    );
}

#[test]
fn template_validate_rejects_a_step_combining_workspace_with_new_column() {
    let template = TempToml::write(
        "workspace-new-column-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [[step]]\nslot = \"editor\"\nworkspace = \"3\"\nnew_column = true\n",
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
    assert!(
        stderr.contains("step 1: workspace can't be combined with new_column/new_row"),
        "{stderr}"
    );
}

#[test]
fn template_and_bindings_each_read_stdin_for_dash() {
    // Either input can come from a pipe (#50), but not both at once.
    let bindings = "[[binding]]\nslot = \"main\"\ncon_id = 42\n";
    let template_file = TempToml::write(
        "stdin-template",
        "[template]\ndescription = \"One slot.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"main\"\n",
    );
    let output = run_with_stdin(
        &[
            "--template",
            template_file.to_str().unwrap(),
            "--bindings",
            "-",
            "--validate",
        ],
        bindings,
    );
    assert!(output.status.success(), "{output:?}");

    let output = run_with_stdin(
        &["--template", "-", "--apps", "foot", "--dry-run"],
        "[template]\ndescription = \"One slot.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"main\"\n",
    );
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("launch foot"));

    let output = run_with_stdin(&["--template", "-", "--bindings", "-"], bindings);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("can't both read stdin"));
}

#[test]
fn a_binding_command_given_as_words_is_quoted_too() {
    let template_file = TempToml::write(
        "words-template",
        "[template]\ndescription = \"One slot.\"\ncategory = \"Test\"\n\n[[step]]\nslot = \"main\"\n",
    );
    let output = run_with_stdin(
        &[
            "--template",
            template_file.to_str().unwrap(),
            "--bindings",
            "-",
            "--dry-run",
        ],
        "[[binding]]\nslot = \"main\"\ncommand = [\"foot\", \"-T\", \"x,y\"]\n",
    );
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("launch 'foot' '-T' 'x,y'"),
        "{output:?}"
    );
}

#[test]
fn a_binding_places_a_built_in_template_slot_on_its_own_output() {
    let bindings = TempToml::write(
        "dual-output-bindings",
        "[[binding]]\nslot = \"first\"\ncommand = \"foot\"\noutput = \"eDP-1\"\n\n\
         [[binding]]\nslot = \"second\"\ncommand = \"foot\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            "dual-output",
            "--bindings",
            bindings.to_str().unwrap(),
            "--on-output",
            "DP-3",
            "--dry-run",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success(), "{output:?}");
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be a JSON object");
    assert_eq!(json["setup"], serde_json::json!(["focus output \"DP-3\""]));
    let commands: Vec<&str> = json["steps"]
        .as_array()
        .expect("steps should be an array")
        .iter()
        .map(|step| step["actions"][0]["command"].as_str().unwrap())
        .collect();
    // The first slot's binding replaces the step's output; the second keeps
    // the template's own.
    assert_eq!(
        commands,
        [
            "move container to output \"eDP-1\"",
            "move container to output \"DP-1\""
        ]
    );
}
