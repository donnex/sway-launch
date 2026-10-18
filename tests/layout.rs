// con_id-only steps never touch the Sway IPC socket, so these exercise
// --layout end to end (file reading, TOML parsing, step iteration, output
// formatting) against the compiled binary without requiring a live Sway
// session.

mod common;

use common::{assert_conflict_error, run_with_stdin, sway_launch, TempToml};
use std::fs;
use std::path::Path;

#[test]
fn layout_plain_output_prints_one_container_id_per_step() {
    let path = TempToml::write(
        "plain-output",
        "[[step]]\ncon_id = 42\n\n[[step]]\ncon_id = 91\n",
    );

    let output = sway_launch()
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

    let output = sway_launch()
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
fn layout_dry_run_sends_sway_commands_with_the_other_actions() {
    let path = TempToml::write(
        "dry-run-sway-commands",
        "[[step]]\ncon_id = 91\nmark = \"m\"\nscratchpad = true\n\
         sway_commands = [\"border none\", \"opacity 0.9\"]\n",
    );

    let output = sway_launch()
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
        "1. target existing container\n2. mark \"m\", border none, opacity 0.9\n3. move scratchpad\n"
    );
}

#[test]
fn a_sway_command_acting_on_the_session_is_refused_on_the_command_line() {
    let output = sway_launch()
        .args(["--con-id", "42", "--sway-command", "exec foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--sway-command"), "{stderr}");
    assert!(
        stderr.contains("isn't a command that acts on one window"),
        "{stderr}"
    );
}

#[test]
fn layout_dry_run_escapes_control_characters_in_a_command() {
    // A TOML escape can put a terminal escape sequence in a command; printed
    // raw, `\u001b[2K` plus a carriage-return-style cursor move would let a
    // layout file make the preview show something other than what it runs.
    let path = TempToml::write(
        "dry-run-escape",
        "[[step]]\ncommand = \"evil\\u001b[1G\\u001b[2Kfoot\"\nmark = \"tab\\tbed\"\n",
    );

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap(), "--dry-run"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        output.status.success(),
        "sway-launch --layout --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert!(
        !stdout.contains('\u{1b}'),
        "raw escape in output: {stdout:?}"
    );
    assert_eq!(
        stdout,
        "1. launch evil\\u{1b}[1G\\u{1b}[2Kfoot\n2. mark \"tab\\tbed\"\n"
    );
}

#[test]
fn layout_dry_run_describes_an_existing_step_matched_by_mark() {
    let path = TempToml::write(
        "dry-run-mark-match",
        "[[step]]\nexisting = true\nmark_match = \"dropdown-term\"\n",
    );

    let output = sway_launch()
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

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.contains(&format!("{}: step 1: ", path.display())),
        "a step error should name the layout file it came from: {stderr:?}"
    );
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

    let output = sway_launch()
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

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap(), "--dry-run", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        "{\"steps\":[{\"actions\":[{\"command\":\"focus\",\"kind\":\"focus\"}],\"target\":\"target existing container\"}]}"
    );
}

#[test]
fn layout_dry_run_reports_a_step_error_without_running_earlier_steps() {
    let path = TempToml::write(
        "dry-run-step-error",
        "[[step]]\ncon_id = 42\n\n[[step]]\nheight = \"notasize\"\ncon_id = 91\n",
    );

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap(), "--validate", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.starts_with('{') && stderr.contains("\"error\""));
    assert!(stderr.contains("step 1"));
}

#[test]
fn layout_json_output_is_a_single_object() {
    let path = TempToml::write(
        "json-output",
        "[[step]]\ncon_id = 42\n\n[[step]]\ncon_id = 91\n",
    );

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap(), "--floating"])
        .output()
        .expect("failed to run sway-launch binary");

    assert_conflict_error(&output, "--layout", "--floating");
}

#[test]
fn layout_conflicts_with_stray_apps_flag() {
    // Regression test: --apps combined with --layout used to parse cleanly
    // and silently discard --apps instead of erroring, since --layout wasn't
    // listed in --apps'/--bindings' conflicts (unlike --completions and
    // --list-templates, which already list both for the same reason).
    let path = TempToml::write("stray-apps", "[[step]]\ncon_id = 42\n");

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap(), "--apps", "foot"])
        .output()
        .expect("failed to run sway-launch binary");

    assert_conflict_error(&output, "--layout", "--apps");
}

#[test]
fn layout_conflicts_with_stray_bindings_flag() {
    let layout_path = TempToml::write("stray-bindings-layout", "[[step]]\ncon_id = 42\n");
    let bindings_path =
        TempToml::write("stray-bindings", "[[binding]]\nslot = \"a\"\ncon_id = 1\n");

    let output = sway_launch()
        .args([
            "--layout",
            layout_path.to_str().unwrap(),
            "--bindings",
            bindings_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert_conflict_error(&output, "--layout", "--bindings");
}

#[test]
fn layout_missing_file_errors() {
    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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

    let output = sway_launch()
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
fn layout_json_error_output_names_the_failing_step_s_container() {
    // A step can resolve its container and then fail on a later action; the
    // error names it as container_id. Step 2 resolves via con_id (no IPC) and
    // then fails on the unreachable socket. A retargeted container isn't
    // added to container_ids/containers -- it may never have existed -- and
    // with --rollback-on-error the answer is the same: a con_id window was
    // never this run's to close. (A launched failing step's window joining
    // the progress lists needs a real launch; see tests/live_sway.rs's
    // rollback_on_error_kills_the_failing_step_s_own_launched_window.)
    let path = TempToml::write(
        "json-error-failing-step",
        "[[step]]\ncon_id = 42\n\n[[step]]\nid = \"second\"\ncon_id = 91\nfloating = true\n",
    );

    for extra in [&[][..], &["--rollback-on-error"][..]] {
        let output = sway_launch()
            .args(["--layout", path.to_str().unwrap(), "--json"])
            .args(extra)
            .output()
            .expect("failed to run sway-launch binary");

        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
        let last_line = stderr.lines().next_back().unwrap_or_default();
        let error: serde_json::Value = serde_json::from_str(last_line)
            .unwrap_or_else(|_| panic!("the last stderr line should be JSON, got {stderr:?}"));
        assert_eq!(error["container_ids"], serde_json::json!([42]), "{extra:?}");
        assert_eq!(error["containers"], serde_json::json!({}), "{extra:?}");
        assert_eq!(error["container_id"], serde_json::json!(91), "{extra:?}");
        assert_eq!(error["rolled_back"], serde_json::json!([]), "{extra:?}");
    }
}

#[test]
fn layout_json_error_output_omits_progress_for_a_single_invocation() {
    // The partial-progress fields are a multi-step concept; a single
    // invocation either resolved its one container or didn't, so reporting
    // an empty array there would suggest a run that got nowhere rather than
    // one the concept never applied to.
    //
    // Needs the invocation to fail, which sway_launch()'s socket that cannot
    // exist guarantees — see its doc comment in tests/common/ for what
    // assuming none is reachable cost.
    let output = sway_launch()
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

    let output = sway_launch()
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

#[test]
fn layout_validate_rejects_a_command_sway_would_split() {
    // A NUL (TOML's \u0000) is rewritten to `;` by Sway's IPC handler just
    // like a newline: `foot -T nulcmd<NUL>mark NULINJ` ran `mark NULINJ` and
    // exited 0, and --validate used to call the file valid.
    for (name, command) in [
        ("split-nul", "foot -T nulcmd\\u0000mark NULINJ"),
        ("split-newline", "foot -T nl\\nmark NLINJ"),
        ("split-comma", "foot -T a,b"),
        ("split-semicolon", "foot; mark INJECTED"),
    ] {
        let path = TempToml::write(
            name,
            &format!("[[step]]\ncon_id = 42\n\n[[step]]\ncommand = \"{command}\"\n"),
        );

        let output = sway_launch()
            .args(["--layout", path.to_str().unwrap(), "--validate"])
            .output()
            .expect("failed to run sway-launch binary");

        assert!(!output.status.success(), "{command:?} should be rejected");
        let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
        assert!(stderr.contains("step 2"), "{command:?}: {stderr}");
        assert!(stderr.contains("command:"), "{command:?}: {stderr}");
    }
}

#[test]
fn layout_validate_rejects_a_mark_containing_a_nul() {
    // Stored by Sway as `ab;cd`, so --mark-match could never find it again.
    let path = TempToml::write(
        "mark-nul",
        "[[step]]\ncon_id = 42\nmark = \"ab\\u0000cd\"\n",
    );

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap(), "--validate"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("mark:"), "{stderr}");
    assert!(stderr.contains("NUL"), "{stderr}");
}

#[test]
fn layout_validate_rejects_a_workspace_containing_a_dollar_sign() {
    // Sway would substitute a `set $ws1` config variable here, so the window
    // would land elsewhere and the run would time out waiting for `$ws1`.
    let path = TempToml::write(
        "workspace-dollar",
        "[[step]]\ncon_id = 42\nworkspace = \"$ws1\"\n",
    );

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap(), "--validate"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("workspace:"), "{stderr}");
    assert!(stderr.contains("config variables"), "{stderr}");
}

#[test]
fn layout_validate_rejects_sway_keywords_as_workspace_and_output_names() {
    // Sway acts on these even quoted (an upper-case workspace keyword crashes
    // Sway 1.11), so they can never be names.
    for (field, value) in [("workspace", "Next"), ("output", "left")] {
        let path = TempToml::write(
            "keyword-name",
            &format!("[[step]]\ncon_id = 42\n{field} = \"{value}\"\n"),
        );
        let output = sway_launch()
            .args(["--layout", path.to_str().unwrap(), "--validate"])
            .output()
            .expect("failed to run sway-launch binary");

        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
        assert!(stderr.contains(&format!("{field}:")), "{stderr}");
        assert!(stderr.contains("is a Sway keyword"), "{stderr}");
    }
}

#[test]
fn layout_validate_rejects_a_zero_timeout() {
    let path = TempToml::write("zero-timeout", "[[step]]\ncon_id = 42\ntimeout = 0\n");

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap(), "--validate"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("timeout:"), "{stderr}");
}

#[test]
fn every_shipped_example_layout_passes_validate() {
    // The live suite drives these against a real compositor, but that job
    // isn't part of the release gate; --validate needs none, so a broken
    // shipped example fails the fast check job here too.
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/layouts");
    let mut checked = 0;
    for entry in fs::read_dir(&directory).expect("examples/layouts should be readable") {
        let path = entry.expect("directory entry should be readable").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("toml") {
            continue;
        }

        let output = sway_launch()
            .args(["--layout", path.to_str().unwrap(), "--validate"])
            .output()
            .expect("failed to run sway-launch binary");

        assert!(
            output.status.success(),
            "{} failed --validate: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        checked += 1;
    }
    assert!(
        checked >= 2,
        "expected at least 2 example layouts, checked {checked}"
    );
}

#[test]
fn layout_keeps_running_its_steps_after_stdout_is_closed() {
    // A closed stdout used to end the process with exit 0 at the first id it
    // couldn't print, so `--layout ... | head -n1` skipped every later step
    // and still reported success. The last step here fails, and that failure
    // is only reached -- and reported -- if the steps before it kept running
    // after their own output was lost. The read end is closed before the
    // process starts, so the very first write is the one that fails.
    let path = TempToml::write(
        "closed-stdout",
        "[[step]]\ncon_id = 42\n\n[[step]]\ncon_id = 91\n\n[[step]]\ntarget_id = \"missing\"\n",
    );
    let (stdout_reader, stdout_writer) = std::io::pipe().expect("failed to create a pipe");
    drop(stdout_reader);

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap()])
        .stdout(stdout_writer)
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        !output.status.success(),
        "the failing last step should still be reached and fail the run"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(stderr.contains("step 3:"), "unexpected stderr: {stderr}");
}

#[test]
fn layout_exits_0_after_finishing_every_step_when_stdout_is_closed() {
    let path = TempToml::write(
        "closed-stdout-success",
        "[[step]]\ncon_id = 42\n\n[[step]]\ncon_id = 91\n",
    );
    let (stdout_reader, stdout_writer) = std::io::pipe().expect("failed to create a pipe");
    drop(stdout_reader);

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap()])
        .stdout(stdout_writer)
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success(), "got {}", output.status);
}

#[test]
fn layout_reads_stdin_for_dash_and_names_it_stdin() {
    // A generated layout can be piped in instead of written to a temp file
    // (#50); errors and --validate call the source <stdin>.
    let output = run_with_stdin(&["--layout", "-", "--validate"], "[[step]]\ncon_id = 42\n");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "valid: <stdin> (1 step(s))"
    );

    let output = run_with_stdin(
        &["--layout", "-", "--validate"],
        "[[step]]\ncon_id = 42\nheight = \"bad\"\n",
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("<stdin>: step 1: height"),
        "{output:?}"
    );
}

#[test]
fn a_second_run_of_a_layout_is_refused_while_the_first_holds_its_lock() {
    // Layout runs take a lock keyed on the file and workspace (#55), so a
    // double-triggered layout can't interleave with itself. The lock here is
    // held by the test, standing in for a run still in progress; --no-lock
    // runs anyway. con_id-only steps with no actions need no compositor.
    let runtime_dir = common::TestTempDir::new();
    let path = TempToml::write("locked", "[[step]]\ncon_id = 42\n");
    let run = |extra: &[&str]| {
        sway_launch()
            .env("XDG_RUNTIME_DIR", &*runtime_dir)
            .args(["--layout", path.to_str().unwrap()])
            .args(extra)
            .output()
            .expect("failed to run sway-launch binary")
    };

    let first = run(&[]);
    assert!(first.status.success(), "{first:?}");
    let lock_path = std::fs::read_dir(runtime_dir.join("sway-launch"))
        .expect("the run should have created its lock directory")
        .map(|entry| entry.expect("directory entry should be readable").path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "lock")
        })
        .expect("the run should have created a lock file");
    let held = std::fs::File::open(&lock_path).expect("lock file should open");
    held.try_lock()
        .expect("the finished run should have released its lock");

    let second = run(&[]);
    assert_eq!(second.status.code(), Some(1), "{second:?}");
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("still in progress"),
        "{second:?}"
    );

    let forced = run(&["--no-lock"]);
    assert!(forced.status.success(), "{forced:?}");
}

#[test]
fn layouts_piped_on_stdin_take_a_lock_of_their_own_contents() {
    // Every layout read from stdin is called `-`, so its lock is keyed on its
    // contents (#129): a different piped layout runs while one is held, the
    // same one is refused.
    use std::io::Write;
    let runtime_dir = common::TestTempDir::new();
    let run = |layout: &str| {
        let mut child = sway_launch()
            .env("XDG_RUNTIME_DIR", &*runtime_dir)
            .args(["--layout", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to run sway-launch binary");
        child
            .stdin
            .take()
            .expect("stdin should be piped")
            .write_all(layout.as_bytes())
            .expect("stdin should accept the layout");
        child
            .wait_with_output()
            .expect("failed to wait for sway-launch")
    };
    let lock_files = || -> Vec<std::path::PathBuf> {
        std::fs::read_dir(runtime_dir.join("sway-launch"))
            .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
            .unwrap_or_default()
    };

    let first = run("[[step]]\ncon_id = 42\n");
    assert!(first.status.success(), "{first:?}");
    let files = lock_files();
    let [lock_path] = files.as_slice() else {
        panic!("the run should have created one lock file: {files:?}");
    };
    let held = std::fs::File::open(lock_path).expect("lock file should open");
    held.try_lock()
        .expect("the finished run should have released its lock");

    let other = run("[[step]]\ncon_id = 43\n");
    assert!(other.status.success(), "{other:?}");
    let same = run("[[step]]\ncon_id = 42\n");
    assert_eq!(same.status.code(), Some(1), "{same:?}");
    assert!(
        String::from_utf8_lossy(&same.stderr).contains("still in progress"),
        "{same:?}"
    );
}

#[test]
fn a_step_command_given_as_words_is_quoted_for_sway_and_the_shell() {
    // A word list reaches the program exactly (#77): the preview shows the
    // quoted command, and a word Sway would rewrite is refused at parse time.
    let output = run_with_stdin(
        &["--layout", "-", "--dry-run"],
        "[[step]]\ncommand = [\"foot\", \"-T\", \"a, b; it's\"]\napp_id = \"foot\"\n",
    );
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("launch 'foot' '-T' 'a, b; it'\\''s'"),
        "{output:?}"
    );

    let output = run_with_stdin(
        &["--layout", "-", "--validate"],
        "[[step]]\ncommand = [\"echo\", \"$HOME\"]\n",
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("config variable"),
        "{output:?}"
    );
}

#[test]
fn a_step_s_cwd_and_env_appear_in_the_preview_and_need_a_command() {
    // #113: cwd/env become a cd/export prefix on the launched command, and
    // only make sense for a step that launches one.
    let output = run_with_stdin(
        &["--layout", "-", "--dry-run"],
        "[[step]]\ncommand = \"foot\"\ncwd = \"/\"\nenv = { GDK_BACKEND = \"x11\" }\n",
    );
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("launch cd '/' && export GDK_BACKEND='x11' && foot"),
        "{output:?}"
    );

    let output = run_with_stdin(
        &["--layout", "-", "--validate"],
        "[[step]]\ncon_id = 42\nenv = { A = \"1\" }\n",
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("only apply to a launched command"),
        "{output:?}"
    );
}

#[test]
fn validate_and_dry_run_accept_a_cwd_missing_on_this_machine() {
    // #146: a file checked in CI or a dotfiles repository names a directory
    // that exists only where it is run for real.
    let layout = "[[step]]\ncommand = \"foot\"\ncwd = \"/nonexistent/sway-launch-146\"\n";
    let output = run_with_stdin(&["--layout", "-", "--validate"], layout);
    assert!(output.status.success(), "{output:?}");

    let output = run_with_stdin(&["--layout", "-", "--dry-run"], layout);
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("launch cd '/nonexistent/sway-launch-146' && foot"),
        "{output:?}"
    );

    let output = sway_launch()
        .args(["--cwd", "/nonexistent/sway-launch-146", "--dry-run", "foot"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(output.status.success(), "{output:?}");

    let output = run_with_stdin(&["--layout", "-", "--no-lock"], layout);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("cwd: /nonexistent/sway-launch-146 is not a directory"),
        "{output:?}"
    );
}

#[test]
fn a_quote_split_across_sway_commands_entries_is_refused_by_validate() {
    // #123: each entry but the first is balanced on its own, and joined
    // they would run an `exec` behind [con_id=N].
    let path = TempToml::write(
        "split-quote-sway-commands",
        "[[step]]\ncon_id = 91\n\
         sway_commands = ['title_format \"a', 'border \", exec touch /tmp/x, border \"']\n",
    );

    let output = sway_launch()
        .args(["--layout", path.to_str().unwrap(), "--validate"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unclosed '\"' quote"), "{stderr}");
}
