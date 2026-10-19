use super::super::confirmation::batch_command;
use super::super::test_support::leaf_node_value;
use super::*;

// SwayLaunch::build_actions

fn minimal_sway_launch() -> SwayLaunch<'static> {
    SwayLaunch {
        target: Target::ConId(42),
        launch_setup: None,
        actions: ActionRequest::default(),
        verbose: false,
        timeout: time::Duration::from_secs(5),
        wait_time: time::Duration::from_millis(20),
        require_launch_on_focused_workspace: false,
        warn_on_misplaced_launch: false,
        catch_late_window: false,
        excluded_containers: Vec::new(),
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
        ActionKind::WorkspaceToOutput { .. } => "workspace_to_output",
        ActionKind::Split(_) => "split",
        ActionKind::Floating => "floating",
        ActionKind::Sticky => "sticky",
        ActionKind::Fullscreen => "fullscreen",
        ActionKind::Focus => "focus",
        ActionKind::Height(_) => "height",
        ActionKind::Width(_) => "width",
        ActionKind::Position(_) => "position",
        ActionKind::Mark(_) => "mark",
        ActionKind::SwayCommand(_) => "sway_command",
        ActionKind::Scratchpad => "scratchpad",
    }
}

#[test]
fn build_actions_includes_every_flag_in_the_documented_fixed_order() {
    // Pins build_actions()'s full plan order against CLAUDE.md's
    // Architecture section: NewColumn -> NewRow -> Workspace -> Output ->
    // Split -> Floating -> Sticky -> Fullscreen -> Focus -> Height ->
    // Width -> Position -> Mark -> SwayCommand (each, in order) ->
    // Scratchpad. Planning never consults the
    // NewColumn/NewRow guard (execute_plan() does, per move), so both are
    // always planned.
    let mut sway_launch = minimal_sway_launch();
    sway_launch.actions.new_column = true;
    sway_launch.actions.new_row = true;
    sway_launch.actions.workspace = Some(WorkspaceTarget::Name("3"));
    sway_launch.actions.output = Some("HDMI-A-1");
    sway_launch.actions.split = Some(Split::H);
    sway_launch.actions.floating = true;
    sway_launch.actions.sticky = true;
    sway_launch.actions.fullscreen = true;
    sway_launch.actions.focus = true;
    sway_launch.actions.height = Some(Size::Pixels(300));
    sway_launch.actions.width = Some(Size::Pixels(400));
    sway_launch.actions.position = Some(Position::Center);
    sway_launch.actions.mark = Some("pinned");
    let sway_commands = ["border none".to_string(), "opacity 0.9".to_string()];
    sway_launch.actions.sway_commands = &sway_commands;
    sway_launch.actions.scratchpad = true;

    let actions = sway_launch.build_actions(42);

    let kinds: Vec<&str> = actions.iter().map(kind_label).collect();
    assert_eq!(
        kinds,
        vec![
            "new_column",
            "new_row",
            "workspace",
            "workspace_to_output",
            "split",
            "floating",
            "sticky",
            "fullscreen",
            "focus",
            "height",
            "width",
            "position",
            "mark",
            "sway_command",
            "sway_command",
            "scratchpad",
        ]
    );
    let verbs: Vec<String> = actions
        .iter()
        .filter(|action| matches!(action.kind, ActionKind::SwayCommand(_)))
        .map(SwayAction::sway_command_verb)
        .collect();
    assert_eq!(verbs, ["border none", "opacity 0.9"]);
    for action in &actions {
        assert_eq!(action.container_id, 42);
    }
}

#[test]
fn build_actions_moves_the_workspace_to_the_output_when_both_are_set() {
    // A container move to the output would land on whatever workspace that
    // output shows, undoing --workspace; moving the workspace keeps both.
    let mut sway_launch = minimal_sway_launch();
    sway_launch.actions.output = Some("HDMI-A-1");
    let alone = sway_launch.build_actions(42);
    assert_eq!(alone.len(), 1);
    assert_eq!(alone[0].kind, ActionKind::Output("HDMI-A-1"));

    sway_launch.actions.workspace = Some(WorkspaceTarget::Name("3"));
    let both = sway_launch.build_actions(42);
    assert_eq!(both.len(), 2);
    assert_eq!(
        both[0].kind,
        ActionKind::Workspace(WorkspaceTarget::Name("3"))
    );
    assert_eq!(
        both[1].kind,
        ActionKind::WorkspaceToOutput {
            workspace: WorkspaceTarget::Name("3"),
            output: "HDMI-A-1"
        }
    );
    assert_eq!(
        batch_command(42, &[&both[1]]),
        "[con_id=42] move workspace to output \"HDMI-A-1\""
    );
    assert_eq!(both[1].kind.flag_name(), "output");
}

#[test]
fn build_actions_gives_event_confirmed_actions_the_timeout_and_the_rest_the_wait_time() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.actions.floating = true;
    sway_launch.actions.mark = Some("pinned");
    sway_launch.actions.split = Some(Split::V);
    sway_launch.actions.height = Some(Size::Pixels(300));
    sway_launch.actions.new_column = true;

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
    sway_launch.actions.new_column = true;
    sway_launch.actions.new_row = true;
    sway_launch.actions.floating = true;
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
                    MoveDirection::Down => Some(SkipReason::TrailingWorkspaceEdge),
                })
            },
            |group| {
                log.borrow_mut().extend(group.iter().map(kind_label));
                Ok(vec![ActionOutcome::Changed; group.len()])
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
                    reason: SkipReason::TrailingWorkspaceEdge,
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
    sway_launch.actions.floating = true;
    sway_launch.actions.focus = true;
    sway_launch.actions.height = Some(Size::Percent(50));
    let records = sway_launch
        .execute_plan(
            sway_launch.build_actions(42),
            |_| panic!("no move in the plan, so the guard is never asked"),
            |group| {
                Ok(group
                    .iter()
                    .map(|action| match action.kind {
                        ActionKind::Floating => ActionOutcome::Changed,
                        ActionKind::Focus => ActionOutcome::AlreadySatisfied,
                        _ => ActionOutcome::Unconfirmed,
                    })
                    .collect())
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
    sway_launch.actions.new_column = true;
    sway_launch.actions.floating = true;
    let error = sway_launch
        .execute_plan(
            sway_launch.build_actions(42),
            |_| Err("cannot reach Sway IPC".to_string()),
            |group| panic!("nothing should run, but {:?} did", group[0].kind),
        )
        .expect_err("an unanswerable guard check stops the run");
    assert_eq!(error.message, "cannot reach Sway IPC");
}

#[test]
fn execute_plan_stops_at_the_first_failed_action() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.actions.workspace = Some(WorkspaceTarget::Name("2"));
    sway_launch.actions.floating = true;
    let ran = std::cell::RefCell::new(Vec::new());
    let error = sway_launch
        .execute_plan(
            sway_launch.build_actions(42),
            |_| Ok(None),
            |group| {
                ran.borrow_mut().extend(group.iter().map(kind_label));
                Err(RunError::from("workspace failed".to_string()))
            },
        )
        .expect_err("the first action fails");
    assert_eq!(error.message, "workspace failed");
    assert_eq!(ran.into_inner(), vec!["workspace"]);
}

#[test]
fn execute_plan_hands_consecutive_batching_actions_over_together() {
    // #32: everything between the moves and the scratchpad goes to Sway as
    // one command, so it reaches the runner as one group.
    let mut sway_launch = minimal_sway_launch();
    sway_launch.actions.new_column = true;
    sway_launch.actions.workspace = Some(WorkspaceTarget::Name("2"));
    sway_launch.actions.split = Some(Split::V);
    sway_launch.actions.floating = true;
    sway_launch.actions.sticky = true;
    sway_launch.actions.fullscreen = true;
    sway_launch.actions.focus = true;
    sway_launch.actions.height = Some(Size::Pixels(500));
    sway_launch.actions.width = Some(Size::Pixels(700));
    sway_launch.actions.position = Some(Position::Center);
    sway_launch.actions.mark = Some("m");
    sway_launch.actions.scratchpad = true;
    let groups = std::cell::RefCell::new(Vec::new());

    let records = sway_launch
        .execute_plan(
            sway_launch.build_actions(42),
            |_| Ok(None),
            |group| {
                groups
                    .borrow_mut()
                    .push(group.iter().map(kind_label).collect::<Vec<_>>());
                Ok(vec![ActionOutcome::Changed; group.len()])
            },
        )
        .expect("neither closure fails");

    assert_eq!(
        groups.into_inner(),
        vec![
            vec!["new_column"],
            vec!["workspace"],
            vec![
                "split",
                "floating",
                "sticky",
                "fullscreen",
                "focus",
                "height",
                "width",
                "position",
                "mark"
            ],
            vec!["scratchpad"],
        ]
    );
    assert_eq!(records.len(), 12, "one record per action, batched or not");
}

// SwayLaunch::build_actions_for_preview

#[test]
fn build_actions_for_preview_never_touches_ipc_even_with_new_column_set() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.actions.new_column = true;
    sway_launch.actions.floating = true;
    // build_actions() is pure, so a preview lists the move even though a
    // real run might skip it.
    let actions = sway_launch.build_actions_for_preview();
    assert_eq!(actions.len(), 2);
}

#[test]
fn build_actions_for_preview_verbs_are_container_id_free() {
    let mut sway_launch = minimal_sway_launch();
    sway_launch.actions.floating = true;
    sway_launch.actions.mark = Some("pinned");
    let actions = sway_launch.build_actions_for_preview();
    let verbs: Vec<String> = actions.iter().map(|a| a.sway_command_verb()).collect();
    assert_eq!(verbs, vec!["floating enable", "mark \"pinned\""]);
}

// DEBUG_EVENT_SUBSCRIPTIONS

#[test]
fn debug_event_json_names_the_event_type_beside_sway_s_payload() {
    let tick: swayipc::Event =
        serde_json::from_value(serde_json::json!({"Tick": {"first": true, "payload": "x"}}))
            .expect("a Tick event should deserialize");
    assert_eq!(
        debug_event_json(&tick),
        serde_json::json!({"event": "tick", "first": true, "payload": "x"})
    );
}

/// One event of every `swayipc::Event` variant, as `(variant, payload)`,
/// the smallest payload each deserializes from.
fn one_event_of_every_kind() -> Vec<(&'static str, serde_json::Value)> {
    let node = leaf_node_value(7, Some("foot"), None);
    let color = serde_json::json!("#000000");
    let colors: serde_json::Map<String, serde_json::Value> = [
        "background",
        "statusline",
        "separator",
        "focused_background",
        "focused_statusline",
        "focused_separator",
        "focused_workspace_text",
        "focused_workspace_bg",
        "focused_workspace_border",
        "active_workspace_text",
        "active_workspace_bg",
        "active_workspace_border",
        "inactive_workspace_text",
        "inactive_workspace_bg",
        "inactive_workspace_border",
        "urgent_workspace_text",
        "urgent_workspace_bg",
        "urgent_workspace_border",
        "binding_mode_text",
        "binding_mode_bg",
        "binding_mode_border",
    ]
    .into_iter()
    .map(|field| (field.to_string(), color.clone()))
    .collect();
    vec![
        (
            "Workspace",
            serde_json::json!({"change": "focus", "current": node, "old": null}),
        ),
        ("Output", serde_json::json!({"change": "unspecified"})),
        (
            "Mode",
            serde_json::json!({"change": "default", "pango_markup": false}),
        ),
        (
            "Window",
            serde_json::json!({"change": "new", "container": node}),
        ),
        (
            "BarConfigUpdate",
            serde_json::json!({
                "id": "bar-0",
                "mode": "dock",
                "position": "top",
                "status_command": null,
                "font": "monospace 10",
                "workspace_buttons": true,
                "binding_mode_indicator": true,
                "verbose": false,
                "colors": colors,
                "gaps": {"top": 0, "bottom": 0, "right": 0, "left": 0},
                "bar_height": 0,
                "status_padding": 1,
                "status_edge_padding": 3,
            }),
        ),
        (
            "Binding",
            serde_json::json!({
                "change": "run",
                "binding": {
                    "command": "nop",
                    "input_code": 0,
                    "symbol": "a",
                    "input_type": "keyboard",
                },
            }),
        ),
        ("Shutdown", serde_json::json!({"change": "exit"})),
        ("Tick", serde_json::json!({"first": false, "payload": ""})),
        (
            "BarStateUpdate",
            serde_json::json!({"id": "bar-0", "visible_by_modifier": true}),
        ),
        (
            "Input",
            serde_json::json!({
                "change": "added",
                "input": {"identifier": "0:0:example", "name": "example", "type": "keyboard"},
            }),
        ),
    ]
}

#[test]
fn debug_event_json_names_every_event_type_as_sway_does() {
    // The name a script filters on must be the one Sway's own subscribe
    // request takes, which for a bar config change isn't the variant's
    // snake case (`bar_config_update`).
    let names: std::collections::BTreeSet<String> = one_event_of_every_kind()
        .into_iter()
        .map(|(variant, payload)| {
            let event: swayipc::Event =
                serde_json::from_value(serde_json::json!({ variant: payload }))
                    .unwrap_or_else(|error| panic!("{} should deserialize: {}", variant, error));
            debug_event_json(&event)["event"]
                .as_str()
                .expect("every event should carry an `event` name")
                .to_string()
        })
        .collect();
    let subscribed: std::collections::BTreeSet<String> = DEBUG_EVENT_SUBSCRIPTIONS
        .iter()
        .map(|event_type| {
            serde_json::to_value(event_type)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();

    assert!(names.contains("barconfig_update"));
    assert_eq!(names, subscribed);
}

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

// focus_moved_during_launch

fn workspace(id: i64, name: &str) -> WorkspaceIdentity {
    WorkspaceIdentity {
        id,
        name: name.to_string(),
    }
}

#[test]
fn focus_moved_during_launch_is_silent_while_focus_stays_put() {
    assert_eq!(
        focus_moved_during_launch(&workspace(5, "1"), Some(&workspace(5, "1"))),
        None
    );
}

#[test]
fn focus_moved_during_launch_ignores_a_rename_of_the_same_workspace() {
    assert_eq!(
        focus_moved_during_launch(&workspace(5, "1"), Some(&workspace(5, "1: foot"))),
        None
    );
}

#[test]
fn focus_moved_during_launch_names_both_workspaces() {
    let message = focus_moved_during_launch(&workspace(5, "1"), Some(&workspace(6, "21")))
        .expect("focus moved");
    assert!(
        message.contains("focus moved from workspace \"1\" to \"21\""),
        "{message}"
    );
    assert!(focus_moved_during_launch(&workspace(5, "1"), None)
        .expect("focus left")
        .contains("left workspace \"1\""));
}

#[test]
fn focus_moved_during_launch_tells_a_new_workspace_under_the_same_name_apart() {
    assert!(focus_moved_during_launch(&workspace(5, "1"), Some(&workspace(9, "1"))).is_some());
}

#[test]
fn build_location_focuses_the_output_then_the_workspace_by_name_or_number() {
    let by_name = BuildLocation {
        workspace: Some(WorkspaceTarget::Name("2: web")),
        output: Some("HEADLESS-1"),
    };
    assert_eq!(
        by_name.commands(),
        [
            "focus output \"HEADLESS-1\"",
            "workspace --no-auto-back-and-forth \"2: web\""
        ]
    );
    let by_number = BuildLocation {
        workspace: Some(WorkspaceTarget::Number(2)),
        output: None,
    };
    assert_eq!(
        by_number.commands(),
        ["workspace --no-auto-back-and-forth number 2"]
    );
}
