use super::super::test_support::{exec_action, targeted};
use super::*;

// SwayAction::new

#[test]
fn new_picks_the_timeout_for_an_event_confirmed_kind() {
    let action = SwayAction::new(
        42,
        ActionKind::Floating,
        true,
        time::Duration::from_secs(7),
        time::Duration::from_millis(42),
    );
    assert_eq!(action.timing, time::Duration::from_secs(7));
    assert_eq!(action.container_id, 42);
    assert!(action.verbose);
}

#[test]
fn new_picks_the_wait_time_for_a_polled_kind() {
    let action = SwayAction::new(
        42,
        ActionKind::Height(Size::Pixels(300)),
        false,
        time::Duration::from_secs(7),
        time::Duration::from_millis(42),
    );
    assert_eq!(action.timing, time::Duration::from_millis(42));
}

// ExecAction::sway_command

#[test]
fn exec_sway_command_prepends_the_marker_to_the_users_command() {
    // Asserts the text that actually reaches Sway. An earlier test asserted
    // a separately rendered "exec foot" that nothing ever sent.
    let action = exec_action("foot --app-id=probe", "", "");
    assert_eq!(
        action.sway_command("1234-5678"),
        "exec export SWAY_LAUNCH_PID_MARKER=1234-5678 && foot --app-id=probe"
    );
}

#[test]
fn exec_sway_command_keeps_a_command_led_by_a_shell_builtin_intact() {
    let action = exec_action("cd /tmp && exec foot", "", "");
    assert_eq!(
        action.sway_command("1234-5678"),
        "exec export SWAY_LAUNCH_PID_MARKER=1234-5678 && cd /tmp && exec foot"
    );
}

// SwayAction::sway_command

#[test]
fn sway_command_renders_every_kind() {
    let cases = [
        (ActionKind::Floating, "[con_id=42] floating enable"),
        (ActionKind::Sticky, "[con_id=42] sticky enable"),
        (ActionKind::Fullscreen, "[con_id=42] fullscreen enable"),
        (ActionKind::Focus, "[con_id=42] focus"),
        (ActionKind::Split(Split::V), "[con_id=42] splitv"),
        (ActionKind::Split(Split::H), "[con_id=42] splith"),
        (ActionKind::NewColumn, "[con_id=42] move right"),
        (ActionKind::NewRow, "[con_id=42] move down"),
        (
            ActionKind::Height(Size::Pixels(300)),
            "[con_id=42] resize set height 300px",
        ),
        (
            ActionKind::Width(Size::Percent(20)),
            "[con_id=42] resize set width 20ppt",
        ),
        (
            ActionKind::Position(Position::Center),
            "[con_id=42] move position center",
        ),
        (
            ActionKind::Position(Position::Coordinates { x: 100, y: 200 }),
            "[con_id=42] move position 100 200",
        ),
        (
            ActionKind::Position(Position::Coordinates { x: -1920, y: -200 }),
            "[con_id=42] move position -1920 -200",
        ),
        (ActionKind::Scratchpad, "[con_id=42] move scratchpad"),
    ];
    for (kind, expected) in cases {
        assert_eq!(targeted(kind).sway_command(), expected, "{kind:?}");
    }
}

#[test]
fn sway_command_mark_quotes_the_mark() {
    assert_eq!(
        targeted(ActionKind::Mark("foo, exec evil")).sway_command(),
        "[con_id=42] mark \"foo, exec evil\""
    );
}

#[test]
fn sway_command_workspace_quotes_the_workspace() {
    assert_eq!(
        targeted(ActionKind::Workspace("web, exec evil")).sway_command(),
        "[con_id=42] move workspace \"web, exec evil\""
    );
}

#[test]
fn sway_command_output_quotes_the_output() {
    assert_eq!(
        targeted(ActionKind::Output("HDMI-A-1, exec evil")).sway_command(),
        "[con_id=42] move container to output \"HDMI-A-1, exec evil\""
    );
}

// SwayAction::sway_command_verb

#[test]
fn sway_command_verb_omits_the_con_id_prefix() {
    // Regression test for the sway_command()/sway_command_verb() split
    // (--dry-run's foundation): the verb-only form is exactly
    // sway_command() with the "[con_id=N] " prefix stripped.
    let floating = targeted(ActionKind::Floating);
    assert_eq!(floating.sway_command(), "[con_id=42] floating enable");
    assert_eq!(floating.sway_command_verb(), "floating enable");

    let position = targeted(ActionKind::Position(Position::Coordinates {
        x: -1920,
        y: -200,
    }));
    assert_eq!(
        position.sway_command(),
        "[con_id=42] move position -1920 -200"
    );
    assert_eq!(position.sway_command_verb(), "move position -1920 -200");
}

// Display

#[test]
fn display_exec() {
    assert_eq!(
        exec_action("foot", "foot", "").to_string(),
        "Exec \"foot\" (app_id_match: \"foot\") (class_match: \"\")"
    );
}

#[test]
fn display_names_every_kind_with_the_same_container_id_wording() {
    let cases = [
        (
            ActionKind::Split(Split::H),
            "Split (container id: 42) (split: Horizontal)",
        ),
        (ActionKind::Floating, "Floating (container id: 42)"),
        (ActionKind::Sticky, "Sticky (container id: 42)"),
        (ActionKind::Fullscreen, "Fullscreen (container id: 42)"),
        (ActionKind::Focus, "Focus (container id: 42)"),
        (ActionKind::NewColumn, "New column (container id: 42)"),
        (ActionKind::NewRow, "New row (container id: 42)"),
        (
            ActionKind::Workspace("web"),
            "Workspace (container id: 42) (workspace: web)",
        ),
        (
            ActionKind::Output("HDMI-A-1"),
            "Output (container id: 42) (output: HDMI-A-1)",
        ),
        (
            ActionKind::Mark("foo"),
            "Mark (container id: 42) (mark: foo)",
        ),
        (
            ActionKind::Height(Size::Pixels(300)),
            "Height (container id: 42) (height: 300px)",
        ),
        (
            ActionKind::Width(Size::Percent(20)),
            "Width (container id: 42) (width: 20ppt)",
        ),
        (
            ActionKind::Position(Position::Center),
            "Position (container id: 42) (position: center)",
        ),
        (ActionKind::Scratchpad, "Scratchpad (container id: 42)"),
    ];
    for (kind, expected) in cases {
        assert_eq!(targeted(kind).to_string(), expected);
    }
}
