use super::*;

// SwayLaunch::build_actions

fn minimal_sway_launch() -> SwayLaunch<'static> {
    SwayLaunch {
        target: Target::ConId(42),
        app_id_match: "",
        class_match: "",
        mark_match: "",
        split: None,
        floating: false,
        sticky: false,
        fullscreen: false,
        focus: false,
        mark: "",
        new_column: false,
        new_row: false,
        workspace: None,
        output: None,
        height: None,
        width: None,
        position: None,
        scratchpad: false,
        verbose: false,
        timeout: time::Duration::from_secs(5),
        wait_time: time::Duration::from_millis(20),
    }
}

#[test]
fn build_actions_is_empty_when_no_flags_are_set() {
    let sway_launch = minimal_sway_launch();
    let actions = sway_launch
        .build_actions(42, true)
        .expect("no flags set means no IPC call at all, so this can't fail");
    assert!(actions.is_empty());
}

#[test]
fn build_actions_includes_every_flag_in_the_documented_fixed_order() {
    // Pins build_actions()'s full plan order against CLAUDE.md's
    // Architecture section: NewColumn -> NewRow -> Workspace -> Output ->
    // Split -> Floating -> Sticky -> Fullscreen -> Focus -> Height ->
    // Width -> Position -> Mark -> Scratchpad. check_relocation: false is
    // what lets new_column/new_row join in headlessly; the relocation
    // guard itself is covered by tests/live_sway.rs.
    let mut sway_launch = minimal_sway_launch();
    sway_launch.new_column = true;
    sway_launch.new_row = true;
    sway_launch.workspace = Some("3");
    sway_launch.output = Some("HDMI-A-1");
    sway_launch.split = Some(Split::H);
    sway_launch.floating = true;
    sway_launch.sticky = true;
    sway_launch.fullscreen = true;
    sway_launch.focus = true;
    sway_launch.height = Some(Size::Pixels(300));
    sway_launch.width = Some(Size::Pixels(400));
    sway_launch.position = Some(Position::Center);
    sway_launch.mark = "pinned";
    sway_launch.scratchpad = true;

    let actions = sway_launch
        .build_actions(42, false)
        .expect("check_relocation: false makes no IPC call, so this can't fail");

    let kinds: Vec<&str> = actions
        .iter()
        .map(|planned| match planned {
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::NewColumn) => {
                "new_column"
            }
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::NewRow) => "new_row",
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Workspace(_)) => {
                "workspace"
            }
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Output(_)) => "output",
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Split(_)) => "split",
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Floating) => "floating",
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Sticky) => "sticky",
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Fullscreen) => {
                "fullscreen"
            }
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Focus) => "focus",
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Height(_)) => "height",
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Width(_)) => "width",
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Position(_)) => {
                "position"
            }
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Mark(_)) => "mark",
            PlannedAction::Run(action) if matches!(action.kind, ActionKind::Scratchpad) => {
                "scratchpad"
            }
            other => panic!("unexpected planned action: {:?}", other),
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "new_column",
            "new_row",
            "workspace",
            "output",
            "split",
            "floating",
            "sticky",
            "fullscreen",
            "focus",
            "height",
            "width",
            "position",
            "mark",
            "scratchpad",
        ]
    );
    for planned in &actions {
        match planned {
            PlannedAction::Run(action) => assert_eq!(action.container_id, 42),
            PlannedAction::Skip(skipped) => panic!("unexpected skip: {:?}", skipped),
        }
    }
}

#[test]
fn build_actions_gives_event_confirmed_actions_the_timeout_and_the_rest_the_wait_time() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.floating = true;
    sway_launch.mark = "pinned";
    sway_launch.split = Some(Split::V);
    sway_launch.height = Some(Size::Pixels(300));
    sway_launch.new_column = true;

    let actions = sway_launch
        .build_actions(42, false)
        .expect("check_relocation: false makes no IPC call, so this can't fail");

    for planned in &actions {
        let PlannedAction::Run(action) = planned else {
            panic!("unexpected skip: {:?}", planned);
        };
        let expected = if action.kind.is_event_confirmed() {
            sway_launch.timeout
        } else {
            sway_launch.wait_time
        };
        assert_eq!(action.timing, expected, "{:?}", action.kind);
        assert!(!action.verbose);
    }
    assert_eq!(actions.len(), 5);
}

#[test]
fn build_actions_omits_mark_when_empty() {
    let sway_launch = minimal_sway_launch();
    let actions = sway_launch
        .build_actions(42, true)
        .expect("no flags set means no IPC call at all, so this can't fail");
    assert!(!actions
            .iter()
            .any(|planned| matches!(planned, PlannedAction::Run(action) if matches!(action.kind, ActionKind::Mark(_)))));
}

// build_actions()'s error propagation when relocates_to_another_output()
// can't read the tree is covered by tests/json_output.rs's
// new_column_and_new_row_report_the_relocation_checks_error, not here: it
// needs no reachable Sway socket, which a unit test can't arrange safely
// (std::env::set_var is process-global, and this harness is multi-threaded),
// while an integration test can set it per-invocation on the child process.
//
// The guard's actual behavior (skip vs. include) needs a live compositor
// either way: see tests/live_sway.rs's new_column_does_not_relocate_*.

#[test]
fn build_actions_with_check_relocation_false_never_touches_ipc() {
    // The whole point of check_relocation: false — confirms
    // new_column/new_row are always included (never skipped, and
    // never erroring) with no reachable Sway socket, whereas
    // check_relocation: true would read the tree (see the note above).
    let mut sway_launch = minimal_sway_launch();
    sway_launch.new_column = true;
    sway_launch.new_row = true;
    let actions = sway_launch
        .build_actions(42, false)
        .expect("check_relocation: false makes no IPC call, so this can't fail");
    assert!(actions
            .iter()
            .any(|planned| matches!(planned, PlannedAction::Run(action) if matches!(action.kind, ActionKind::NewColumn))));
    assert!(actions
            .iter()
            .any(|planned| matches!(planned, PlannedAction::Run(action) if matches!(action.kind, ActionKind::NewRow))));
    assert!(!actions
        .iter()
        .any(|planned| matches!(planned, PlannedAction::Skip(_))));
}

// SwayLaunch::build_actions_for_preview

#[test]
fn build_actions_for_preview_never_touches_ipc_even_with_new_column_set() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.new_column = true;
    sway_launch.floating = true;
    // Would panic (build_actions_for_preview()'s own .expect()) if this
    // somehow tried an IPC call in this headless test environment —
    // passing at all is the assertion.
    let actions = sway_launch.build_actions_for_preview();
    assert_eq!(actions.len(), 2);
}

#[test]
fn build_actions_for_preview_verbs_are_container_id_free() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.floating = true;
    sway_launch.mark = "pinned";
    let actions = sway_launch.build_actions_for_preview();
    let verbs: Vec<String> = actions.iter().map(|a| a.sway_command_verb()).collect();
    assert_eq!(verbs, vec!["floating enable", "mark \"pinned\""]);
}

// DEBUG_EVENT_SUBSCRIPTIONS

#[test]
fn debug_event_subscriptions_cover_every_sway_ipc_event_type_once() {
    // Sway's own IPC event list (sway-ipc(7), "EVENTS"), by wire name. The
    // array's length is fixed by its type, so a duplicate entry would push a
    // real event type out; comparing as sets catches that as well as a gap.
    let expected: std::collections::BTreeSet<&str> = [
        "workspace",
        "output",
        "mode",
        "window",
        "barconfig_update",
        "binding",
        "shutdown",
        "tick",
        "bar_state_update",
        "input",
    ]
    .into_iter()
    .collect();

    let subscribed: Vec<String> = DEBUG_EVENT_SUBSCRIPTIONS
        .iter()
        .map(|event_type| {
            serde_json::to_value(event_type)
                .expect("EventType should serialize")
                .as_str()
                .expect("EventType should serialize to a string")
                .to_string()
        })
        .collect();
    let subscribed_set: std::collections::BTreeSet<&str> =
        subscribed.iter().map(String::as_str).collect();

    assert_eq!(subscribed.len(), subscribed_set.len(), "duplicate entry");
    assert_eq!(subscribed_set, expected);
}
