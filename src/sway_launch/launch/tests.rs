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
        mark: None,
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
    assert!(sway_launch.build_actions(42).is_empty());
}

fn kind_label(action: &SwayAction) -> &'static str {
    match action.kind {
        ActionKind::NewColumn => "new_column",
        ActionKind::NewRow => "new_row",
        ActionKind::Workspace(_) => "workspace",
        ActionKind::Output(_) => "output",
        ActionKind::Split(_) => "split",
        ActionKind::Floating => "floating",
        ActionKind::Sticky => "sticky",
        ActionKind::Fullscreen => "fullscreen",
        ActionKind::Focus => "focus",
        ActionKind::Height(_) => "height",
        ActionKind::Width(_) => "width",
        ActionKind::Position(_) => "position",
        ActionKind::Mark(_) => "mark",
        ActionKind::Scratchpad => "scratchpad",
    }
}

#[test]
fn build_actions_includes_every_flag_in_the_documented_fixed_order() {
    // Pins build_actions()'s full plan order against CLAUDE.md's
    // Architecture section: NewColumn -> NewRow -> Workspace -> Output ->
    // Split -> Floating -> Sticky -> Fullscreen -> Focus -> Height ->
    // Width -> Position -> Mark -> Scratchpad. Planning never consults the
    // NewColumn/NewRow guard (execute_plan() does, per move), so both are
    // always planned.
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
    sway_launch.mark = Some("pinned");
    sway_launch.scratchpad = true;

    let actions = sway_launch.build_actions(42);

    let kinds: Vec<&str> = actions.iter().map(kind_label).collect();
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
    for action in &actions {
        assert_eq!(action.container_id, 42);
    }
}

#[test]
fn build_actions_gives_event_confirmed_actions_the_timeout_and_the_rest_the_wait_time() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.floating = true;
    sway_launch.mark = Some("pinned");
    sway_launch.split = Some(Split::V);
    sway_launch.height = Some(Size::Pixels(300));
    sway_launch.new_column = true;

    let actions = sway_launch.build_actions(42);

    for action in &actions {
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
fn build_actions_omits_mark_when_unset() {
    let sway_launch = minimal_sway_launch();
    assert!(!sway_launch
        .build_actions(42)
        .iter()
        .any(|action| matches!(action.kind, ActionKind::Mark(_))));
}

// SwayLaunch::execute_plan

#[test]
fn execute_plan_judges_new_row_only_after_new_column_has_run() {
    // The guard used to run while planning, so --new-row was judged against
    // a tree --new-column was about to change. Recording every guard call
    // and every run in one log pins that each move is checked right before
    // it's sent, after everything ahead of it in the plan has run.
    let mut sway_launch = minimal_sway_launch();
    sway_launch.new_column = true;
    sway_launch.new_row = true;
    sway_launch.floating = true;
    let log = std::cell::RefCell::new(Vec::new());

    let records = sway_launch
        .execute_plan(
            sway_launch.build_actions(42),
            |direction| {
                log.borrow_mut().push(match direction {
                    MoveDirection::Right => "check right",
                    MoveDirection::Down => "check down",
                });
                Ok(match direction {
                    MoveDirection::Right => None,
                    MoveDirection::Down => Some("trailing_workspace_edge"),
                })
            },
            |action| {
                log.borrow_mut().push(kind_label(action));
                Ok(ActionOutcome::Changed)
            },
        )
        .expect("neither closure fails");

    assert_eq!(
        log.into_inner(),
        vec!["check right", "new_column", "check down", "floating"]
    );
    assert_eq!(
        records,
        vec![
            ActionRecord {
                kind: "new_column",
                command: Some("move right".to_string()),
                status: ActionStatus::Changed,
            },
            ActionRecord {
                kind: "new_row",
                command: None,
                status: ActionStatus::Skipped {
                    reason: "trailing_workspace_edge",
                },
            },
            ActionRecord {
                kind: "floating",
                command: Some("floating enable".to_string()),
                status: ActionStatus::Changed,
            },
        ]
    );
}

#[test]
fn execute_plan_maps_each_action_outcome_to_its_status() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.floating = true;
    sway_launch.focus = true;
    sway_launch.height = Some(Size::Percent(50));
    let records = sway_launch
        .execute_plan(
            sway_launch.build_actions(42),
            |_| panic!("no move in the plan, so the guard is never asked"),
            |action| {
                Ok(match action.kind {
                    ActionKind::Floating => ActionOutcome::Changed,
                    ActionKind::Focus => ActionOutcome::AlreadySatisfied,
                    _ => ActionOutcome::Unconfirmed,
                })
            },
        )
        .expect("neither closure fails");
    let statuses: Vec<ActionStatus> = records.iter().map(|record| record.status).collect();
    assert_eq!(
        statuses,
        vec![
            ActionStatus::Changed,
            ActionStatus::AlreadySatisfied,
            ActionStatus::Unconfirmed,
        ]
    );
}

#[test]
fn execute_plan_stops_at_a_failed_guard_check_without_sending_the_move() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.new_column = true;
    sway_launch.floating = true;
    let error = sway_launch
        .execute_plan(
            sway_launch.build_actions(42),
            |_| Err("cannot reach Sway IPC".to_string()),
            |action| panic!("nothing should run, but {:?} did", action.kind),
        )
        .expect_err("an unanswerable guard check stops the run");
    assert_eq!(error.message, "cannot reach Sway IPC");
}

#[test]
fn execute_plan_stops_at_the_first_failed_action() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.floating = true;
    sway_launch.focus = true;
    let ran = std::cell::RefCell::new(Vec::new());
    let error = sway_launch
        .execute_plan(
            sway_launch.build_actions(42),
            |_| Ok(None),
            |action| {
                ran.borrow_mut().push(kind_label(action));
                Err(RunError::from("floating failed".to_string()))
            },
        )
        .expect_err("the first action fails");
    assert_eq!(error.message, "floating failed");
    assert_eq!(ran.into_inner(), vec!["floating"]);
}

// SwayLaunch::build_actions_for_preview

#[test]
fn build_actions_for_preview_never_touches_ipc_even_with_new_column_set() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.new_column = true;
    sway_launch.floating = true;
    // build_actions() is pure, so a preview lists the move even though a
    // real run might skip it.
    let actions = sway_launch.build_actions_for_preview();
    assert_eq!(actions.len(), 2);
}

#[test]
fn build_actions_for_preview_verbs_are_container_id_free() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.floating = true;
    sway_launch.mark = Some("pinned");
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
