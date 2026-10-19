// A failed run whose stderr isn't a terminal also shows its error with
// swaynag (#148). Every test here runs the binary with stderr piped and no
// compositor behind `SWAYSOCK`, so a run that acts on a window fails, and
// `common::sway_launch()`'s stand-in `swaynag` records whether it was shown.

mod common;

use common::{sway_launch, SwayLaunchCommand, TempToml};
use std::time::Duration;

/// How long a check that swaynag *isn't* shown waits for a call to land:
/// well past the few milliseconds the stand-in takes when it is.
const NO_CALL_WAIT: Duration = Duration::from_millis(500);

/// How long a check that swaynag *is* shown waits for its call.
const CALL_WAIT: Duration = Duration::from_secs(5);

fn run(command: &mut SwayLaunchCommand, args: &[&str]) -> std::process::Output {
    command
        .args(args)
        .output()
        .expect("failed to run sway-launch binary")
}

const FLOATING_STEP_LAYOUT: &str = "[[step]]\ncon_id = 42\nfloating = true\n";

const FLOATING_TEMPLATE: &str = "[template]\ndescription = \"Test template.\"\n\
                                 category = \"Test\"\n\n[[step]]\nslot = \"main\"\n\
                                 floating = true\n";

const BINDINGS: &str = "[[binding]]\nslot = \"main\"\ncon_id = 42\n";

#[test]
fn a_failure_with_stderr_piped_is_shown_with_swaynag() {
    let mut command = sway_launch();
    let output = run(&mut command, &["--con-id", "42", "--floating"]);
    assert_eq!(output.status.code(), Some(1));

    let calls = command.swaynag_calls(CALL_WAIT);
    assert_eq!(calls.len(), 1, "expected one swaynag call: {calls:?}");
    assert!(
        calls[0].starts_with("-t error -m sway-launch: cannot reach Sway IPC"),
        "expected swaynag to show the error itself: {calls:?}"
    );
    assert!(
        !calls[0].contains('\n'),
        "swaynag's message should be one line: {calls:?}"
    );
}

#[test]
fn a_layout_failure_is_shown_with_swaynag() {
    let layout = TempToml::write("swaynag-layout", FLOATING_STEP_LAYOUT);
    let mut command = sway_launch();
    let output = run(&mut command, &["--layout", layout.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));

    let calls = command.swaynag_calls(CALL_WAIT);
    assert_eq!(calls.len(), 1, "expected one swaynag call: {calls:?}");
    assert!(
        calls[0].contains("step 1"),
        "expected the step's error: {calls:?}"
    );
}

#[test]
fn an_unreadable_layout_file_is_shown_with_swaynag() {
    let mut command = sway_launch();
    let output = run(
        &mut command,
        &["--layout", "/nonexistent/sway-launch-test.toml"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(command.swaynag_calls(CALL_WAIT).len(), 1);
}

#[test]
fn no_swaynag_keeps_a_failure_off_swaynag() {
    let mut command = sway_launch();
    let output = run(
        &mut command,
        &["--con-id", "42", "--floating", "--no-swaynag"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(command.swaynag_calls(NO_CALL_WAIT), Vec::<String>::new());
}

#[test]
fn json_keeps_a_failure_off_swaynag() {
    let mut command = sway_launch();
    let output = run(&mut command, &["--con-id", "42", "--floating", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(command.swaynag_calls(NO_CALL_WAIT), Vec::<String>::new());
}

#[test]
fn a_layout_file_can_turn_swaynag_off() {
    let layout = TempToml::write(
        "swaynag-off-layout",
        &format!("no_swaynag = true\n{FLOATING_STEP_LAYOUT}"),
    );
    let mut command = sway_launch();
    let output = run(&mut command, &["--layout", layout.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(command.swaynag_calls(NO_CALL_WAIT), Vec::<String>::new());
}

#[test]
fn a_bindings_file_can_turn_swaynag_off() {
    let template = TempToml::write("swaynag-template", FLOATING_TEMPLATE);
    let bindings = TempToml::write(
        "swaynag-off-bindings",
        &format!("no_swaynag = true\n{BINDINGS}"),
    );
    let mut command = sway_launch();
    let output = run(
        &mut command,
        &[
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            bindings.to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(command.swaynag_calls(NO_CALL_WAIT), Vec::<String>::new());
}

#[test]
fn a_template_failure_without_the_bindings_field_is_shown_with_swaynag() {
    let template = TempToml::write("swaynag-template", FLOATING_TEMPLATE);
    let bindings = TempToml::write("swaynag-bindings", BINDINGS);
    let mut command = sway_launch();
    let output = run(
        &mut command,
        &[
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            bindings.to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(command.swaynag_calls(CALL_WAIT).len(), 1);
}

#[test]
fn validate_and_dry_run_failures_are_not_shown_with_swaynag() {
    let broken = TempToml::write("swaynag-broken", "[[step]]\nnot_a_field = 1\n");
    for mode in ["--validate", "--dry-run"] {
        let mut command = sway_launch();
        let output = run(&mut command, &["--layout", broken.to_str().unwrap(), mode]);
        assert_eq!(output.status.code(), Some(1), "{mode}");
        assert_eq!(
            command.swaynag_calls(NO_CALL_WAIT),
            Vec::<String>::new(),
            "{mode}"
        );
    }
}

#[test]
fn a_usage_error_is_not_shown_with_swaynag() {
    let mut command = sway_launch();
    let output = run(&mut command, &["--timeout", "0", "foot"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(command.swaynag_calls(NO_CALL_WAIT), Vec::<String>::new());
}
