use super::super::test_support::*;
use super::super::values::{sway_command_split_problem, Position, Size, Split, WorkspaceTarget};
use super::*;

// unfiltered_fallback_hint

#[test]
fn unfiltered_fallback_hint_is_empty_with_a_filter_and_names_the_flags_without() {
    assert_eq!(unfiltered_fallback_hint(filter("foot", "")), "");
    let hint = unfiltered_fallback_hint(None);
    assert!(hint.contains("pass -a/-c"), "{hint}");
}

// exec_timeout_message / event_timeout_message

fn unmatched(container_id: i64, app_id: Option<&str>, class: Option<&str>) -> UnmatchedWindow {
    UnmatchedWindow {
        identity: WindowIdentity {
            container_id,
            app_id: app_id.map(str::to_string),
            class: class.map(str::to_string),
            title: None,
        },
        launched_by_this_run: false,
    }
}

#[test]
fn exec_timeout_message_names_a_window_the_filter_rejected() {
    let message = exec_timeout_message(
        time::Duration::from_secs(2),
        filter("notfoot", ""),
        None,
        &[UnmatchedWindow {
            launched_by_this_run: true,
            ..unmatched(12, Some("foot"), None)
        }],
        Some(true),
        false,
    );
    assert_eq!(
        message,
        "no window with app_id \"notfoot\" appeared within 2 sec; 1 other new window did \
             (con_id 12, app_id \"foot\", opened by this command) — check --app-id/--class \
             (a window opened by this command was rechecked until the timeout, in case the \
             app sets its app_id/class after mapping)"
    );
}

#[test]
fn exec_timeout_message_lists_several_rejected_windows() {
    let message = exec_timeout_message(
        time::Duration::from_secs(5),
        filter("", "Firefox"),
        None,
        &[
            UnmatchedWindow {
                launched_by_this_run: true,
                ..unmatched(12, Some("foot"), None)
            },
            unmatched(13, None, Some("XTerm")),
            unmatched(14, None, None),
        ],
        None,
        false,
    );
    assert_eq!(
        message,
        "no window with class \"Firefox\" appeared within 5 sec; 3 other new windows did \
             (con_id 12, app_id \"foot\", opened by this command), (con_id 13, class \"XTerm\"), \
             (con_id 14, no app_id or class) — check --app-id/--class (a window opened by this \
             command was rechecked until the timeout, in case the app sets its app_id/class \
             after mapping)"
    );
}

#[test]
fn exec_timeout_message_escapes_control_characters_in_an_app_id_or_class() {
    let message = exec_timeout_message(
        time::Duration::from_secs(2),
        filter("notfoot", ""),
        None,
        &[
            unmatched(12, Some("foot\u{1b}]0;x\u{7}"), None),
            unmatched(13, None, Some("XTerm\r\u{1b}[2K")),
        ],
        None,
        false,
    );
    assert!(!message.chars().any(char::is_control), "{message:?}");
    assert!(
        message.contains(r#"(con_id 12, app_id "foot\u{1b}]0;x\u{7}")"#),
        "{message}"
    );
    assert!(
        message.contains(r#"(con_id 13, class "XTerm\r\u{1b}[2K")"#),
        "{message}"
    );
}

#[test]
fn exec_timeout_message_names_a_launched_window_that_never_set_its_class() {
    let message = exec_timeout_message(
        time::Duration::from_secs(5),
        filter("", "Spotify"),
        None,
        &[UnmatchedWindow {
            launched_by_this_run: true,
            ..unmatched(12, None, None)
        }],
        None,
        false,
    );
    assert_eq!(
        message,
        "no window with class \"Spotify\" appeared within 5 sec; 1 other new window did \
             (con_id 12, no app_id or class, opened by this command) — check --app-id/--class \
             (a window opened by this command was rechecked until the timeout, in case the \
             app sets its app_id/class after mapping)"
    );
}

#[test]
fn exec_timeout_message_points_at_path_when_the_command_never_started() {
    let message = exec_timeout_message(
        time::Duration::from_secs(2),
        filter("", ""),
        None,
        &[],
        Some(false),
        false,
    );
    assert_eq!(
        message,
        "no window appeared within 2 sec: the launched command exited without opening a \
             window — check that it is on PATH in Sway's environment"
    );
}

#[test]
fn exec_timeout_message_says_a_command_that_ran_and_exited_may_have_raised_an_open_window() {
    let message = exec_timeout_message(
        time::Duration::from_secs(5),
        filter("", "Code"),
        None,
        &[],
        Some(false),
        true,
    );
    assert_eq!(
        message,
        "no window with class \"Code\" appeared within 5 sec: the launched command ran and \
             exited without opening a new window — if the application was already running, it \
             may have raised its open window instead; act on that one with --existing"
    );
}

#[test]
fn exec_timeout_message_suggests_slowness_only_while_the_command_still_runs() {
    let message = exec_timeout_message(
        time::Duration::from_secs(2),
        filter("", ""),
        None,
        &[],
        Some(true),
        false,
    );
    assert!(message.contains("still running"), "{message}");
    assert!(message.contains("--timeout"), "{message}");
}

#[test]
fn exec_timeout_message_adds_nothing_when_the_command_s_state_is_unknown() {
    assert_eq!(
        exec_timeout_message(
            time::Duration::from_secs(3),
            filter("foot", ""),
            None,
            &[],
            None,
            false
        ),
        "no window with app_id \"foot\" appeared within 3 sec"
    );
}

#[test]
fn exec_timeout_message_prefers_this_command_s_rejected_window_over_its_state() {
    let message = exec_timeout_message(
        time::Duration::from_secs(2),
        filter("notfoot", ""),
        None,
        &[UnmatchedWindow {
            launched_by_this_run: true,
            ..unmatched(12, Some("foot"), None)
        }],
        Some(false),
        false,
    );
    assert!(message.contains("check --app-id/--class"), "{message}");
    assert!(!message.contains("exited"), "{message}");
}

#[test]
fn exec_timeout_message_leads_with_an_exited_command_over_a_foreign_window() {
    let message = exec_timeout_message(
        time::Duration::from_secs(2),
        filter("foot", ""),
        None,
        &[unmatched(7, Some("other"), None)],
        Some(false),
        false,
    );
    assert_eq!(
        message,
        "no window with app_id \"foot\" appeared within 2 sec: the launched command exited \
             without opening a window — check that it is on PATH in Sway's environment; \
             meanwhile 1 other new window opened, not traced to this command \
             (con_id 7, app_id \"other\") — if it is this app's, check --app-id/--class"
    );
}

#[test]
fn exec_timeout_message_leads_with_a_running_command_over_foreign_windows() {
    let message = exec_timeout_message(
        time::Duration::from_secs(2),
        filter("foot", ""),
        None,
        &[
            unmatched(7, Some("other"), None),
            unmatched(8, None, Some("XTerm")),
        ],
        Some(true),
        false,
    );
    assert_eq!(
        message,
        "no window with app_id \"foot\" appeared within 2 sec; the launched command is still \
             running — it may be slow to start (see --timeout) or not open a window at all; \
             meanwhile 2 other new windows opened, not traced to this command \
             (con_id 7, app_id \"other\"), (con_id 8, class \"XTerm\") — if one is this \
             app's, check --app-id/--class"
    );
    assert!(!message.contains("rechecked"), "{message}");
}

#[test]
fn event_timeout_message_names_the_wait_not_the_flag() {
    assert_eq!(
        event_timeout_message(time::Duration::from_secs(1)),
        "no matching window event within 1 sec"
    );
}

// ActionKind::matching_window_change_event

#[test]
fn exec_only_matches_new_window_change() {
    // Regression test: `Move` used to be accepted here too, which meant
    // any pre-existing window being reparented (not just the one we
    // just launched) could be mistaken for a match — even one that passes
    // the app_id filter.
    let action = exec_action("foot", "foot", "");
    let event = window_event("move", 99, Some("foot"), None);
    assert!(matches!(
        action.matches_window_event(&event),
        Err(WindowEventMatchError::EventChangeTypeMismatch)
    ));
}

#[test]
fn floating_matches_floating_window_change() {
    let action = targeted(ActionKind::Floating);
    assert_eq!(
        action.kind.matching_window_change_event(),
        Some(WindowChange::Floating)
    );
}

#[test]
fn fullscreen_matches_fullscreen_mode_window_change() {
    let action = targeted(ActionKind::Fullscreen);
    assert_eq!(
        action.kind.matching_window_change_event(),
        Some(WindowChange::FullscreenMode)
    );
}

#[test]
fn focus_matches_focus_window_change() {
    let action = targeted(ActionKind::Focus);
    assert_eq!(
        action.kind.matching_window_change_event(),
        Some(WindowChange::Focus)
    );
}

#[test]
fn workspace_matches_move_window_change() {
    let action = targeted(ActionKind::Workspace(WorkspaceTarget::Name("web")));
    assert_eq!(
        action.kind.matching_window_change_event(),
        Some(WindowChange::Move)
    );
}

#[test]
fn output_matches_move_window_change() {
    let action = targeted(ActionKind::Output("HDMI-A-1"));
    assert_eq!(
        action.kind.matching_window_change_event(),
        Some(WindowChange::Move)
    );
}

#[test]
fn scratchpad_matches_move_window_change() {
    let action = targeted(ActionKind::Scratchpad);
    assert_eq!(
        action.kind.matching_window_change_event(),
        Some(WindowChange::Move)
    );
}

#[test]
fn mark_matches_mark_window_change() {
    let action = targeted(ActionKind::Mark("foo"));
    assert_eq!(
        action.kind.matching_window_change_event(),
        Some(WindowChange::Mark)
    );
}

#[test]
fn sticky_has_no_matching_window_change() {
    // Regression test: confirmed live that toggling sticky fires no
    // WindowChange event at all, unlike Floating/Fullscreen/Focus,
    // which each have a dedicated variant — Sticky must stay a
    // wait-time action, not accidentally wired up to wait on an event
    // that will never arrive.
    let action = targeted(ActionKind::Sticky);
    assert_eq!(action.kind.matching_window_change_event(), None);
}

#[test]
fn split_height_width_have_no_matching_window_change() {
    let split = targeted(ActionKind::Split(Split::V));
    let new_column = targeted(ActionKind::NewColumn);
    let new_row = targeted(ActionKind::NewRow);
    let height = targeted(ActionKind::Height(Size::Pixels(300)));
    let width = targeted(ActionKind::Width(Size::Percent(20)));
    let position = targeted(ActionKind::Position(Position::Center));
    assert_eq!(split.kind.matching_window_change_event(), None);
    assert_eq!(new_column.kind.matching_window_change_event(), None);
    assert_eq!(new_row.kind.matching_window_change_event(), None);
    assert_eq!(height.kind.matching_window_change_event(), None);
    assert_eq!(width.kind.matching_window_change_event(), None);
    assert_eq!(position.kind.matching_window_change_event(), None);
}

// SwayAction::poll_grace

#[test]
fn move_actions_poll_for_a_shorter_grace_than_everything_else() {
    // NewColumn/NewRow confirm on "the rect changed at all", which can't
    // distinguish this action's own move from another client's change to
    // the same window — so the window in which that confusion is possible
    // is deliberately much shorter for them. See MOVE_POLL_GRACE.
    let new_column = targeted(ActionKind::NewColumn);
    let new_row = targeted(ActionKind::NewRow);
    let split = targeted(ActionKind::Split(Split::H));

    assert_eq!(new_column.poll_grace(), MOVE_POLL_GRACE);
    assert_eq!(new_row.poll_grace(), MOVE_POLL_GRACE);
    assert_eq!(split.poll_grace(), WAIT_TIME_POLL_GRACE);
    assert!(MOVE_POLL_GRACE < WAIT_TIME_POLL_GRACE);
}

#[test]
fn every_other_polling_variant_keeps_the_full_grace() {
    let height = targeted(ActionKind::Height(Size::Pixels(300)));
    let width = targeted(ActionKind::Width(Size::Pixels(300)));
    let position = targeted(ActionKind::Position(Position::Center));
    let sticky = targeted(ActionKind::Sticky);

    for action in [height, width, position, sticky] {
        assert_eq!(action.poll_grace(), WAIT_TIME_POLL_GRACE);
    }
}

// SwayAction::polls
//
// These assert which variants opt into polling at all — a pure decision
// with no IPC. The match outcome itself still needs a real compositor: see
// tests/live_sway.rs.

#[test]
fn split_polls() {
    for split in [Split::H, Split::V] {
        let action = targeted(ActionKind::Split(split));
        assert!(action.polls(None));
    }
}

#[test]
fn new_column_and_new_row_do_not_poll_without_a_baseline() {
    // Unlike Split/Height/Width/Position, NewColumn/NewRow have no fixed
    // target to check against — without a poll_baseline() snapshot to
    // compare the current rect to there is nothing to poll for, so these
    // opt out entirely.
    let new_column = targeted(ActionKind::NewColumn);
    let new_row = targeted(ActionKind::NewRow);
    assert!(!new_column.polls(None));
    assert!(!new_row.polls(None));
}

#[test]
fn new_column_and_new_row_poll_given_a_baseline() {
    let new_column = targeted(ActionKind::NewColumn);
    let new_row = targeted(ActionKind::NewRow);
    let baseline = Some(rect(0, 0, 640, 720));
    assert!(new_column.polls(baseline));
    assert!(new_row.polls(baseline));
}

#[test]
fn height_and_width_in_pixels_poll() {
    let height = targeted(ActionKind::Height(Size::Pixels(300)));
    let width = targeted(ActionKind::Width(Size::Pixels(400)));
    assert!(height.polls(None));
    assert!(width.polls(None));
}

#[test]
fn height_and_width_in_percent_do_not_poll() {
    // A `ppt` value has no pixel figure to poll for without also
    // resolving the reference dimension it's a percentage of (see Size's
    // doc comment), so these opt out rather than polling for something
    // that could never match.
    let height = targeted(ActionKind::Height(Size::Percent(20)));
    let width = targeted(ActionKind::Width(Size::Percent(20)));
    assert!(!height.polls(None));
    assert!(!width.polls(None));
}

#[test]
fn sticky_polls() {
    let action = targeted(ActionKind::Sticky);
    assert!(action.polls(None));
}

#[test]
fn position_polls() {
    for position in [Position::Center, Position::Coordinates { x: 100, y: 200 }] {
        let action = targeted(ActionKind::Position(position));
        assert!(action.polls(None));
    }
}

#[test]
fn event_confirmed_actions_do_not_poll() {
    // The `_ => false` arm: everything dispatched through an IPC event
    // confirms that way instead and never enters the poll loop.
    let floating = targeted(ActionKind::Floating);
    let mark = targeted(ActionKind::Mark("pinned"));
    assert!(!floating.polls(None));
    assert!(!mark.polls(None));
}

// SwayAction::matches_window_event

#[test]
fn exec_without_filter_matches_any_new_window() {
    let action = exec_action("foot", "", "");
    let event = window_event("new", 99, Some("foot"), None);
    assert!(matches!(
        action.matches_window_event(&event),
        Ok(WindowEventMatch::NewWindowMatchWithoutCheck)
    ));
}

#[test]
fn exec_without_filter_rejects_non_new_window_change() {
    // Regression test for the Move-over-matching bug: with no
    // app_id/class filter, only a `New` event may identify the window
    // just launched — not a `Move` (or any other) event belonging to
    // some other, pre-existing window.
    let action = exec_action("foot", "", "");
    let event = window_event("move", 99, Some("foot"), None);
    assert!(matches!(
        action.matches_window_event(&event),
        Err(WindowEventMatchError::EventChangeTypeMismatch)
    ));
}

#[test]
fn exec_with_app_id_match_accepts_matching_app_id() {
    let action = exec_action("foot", "foot", "");
    let event = window_event("new", 99, Some("foot"), None);
    assert!(matches!(
        action.matches_window_event(&event),
        Ok(WindowEventMatch::WindowAppId)
    ));
}

#[test]
fn exec_with_app_id_match_rejects_different_app_id() {
    let action = exec_action("foot", "foot", "");
    let event = window_event("new", 99, Some("alacritty"), None);
    assert!(matches!(
        action.matches_window_event(&event),
        Err(WindowEventMatchError::WindowAppIdMismatch)
    ));
}

#[test]
fn exec_with_app_id_match_rejects_missing_app_id() {
    let action = exec_action("firefox", "firefox", "");
    let event = window_event("new", 99, None, None);
    assert!(matches!(
        action.matches_window_event(&event),
        Err(WindowEventMatchError::WindowAppIdMismatch)
    ));
}

#[test]
fn exec_with_class_match_accepts_matching_class() {
    let action = exec_action("firefox", "", "Firefox");
    let event = window_event("new", 99, None, Some("Firefox"));
    assert!(matches!(
        action.matches_window_event(&event),
        Ok(WindowEventMatch::WindowClass)
    ));
}

#[test]
fn exec_with_class_match_rejects_different_class() {
    let action = exec_action("firefox", "", "Firefox");
    let event = window_event("new", 99, None, Some("Chromium"));
    assert!(matches!(
        action.matches_window_event(&event),
        Err(WindowEventMatchError::WindowClassMismatch)
    ));
}

#[test]
fn exec_with_class_match_rejects_missing_window_properties() {
    let action = exec_action("firefox", "", "Firefox");
    let event = window_event("new", 99, None, None);
    assert!(matches!(
        action.matches_window_event(&event),
        Err(WindowEventMatchError::WindowClassMismatch)
    ));
}

#[test]
fn exec_with_app_id_match_ignores_class_match_when_both_set() {
    // Documents current behavior: when both are set, app_id_match takes
    // priority and class_match is never consulted (see also
    // Args::app_id's conflicts_with in main.rs, which now prevents a
    // caller from setting both in the first place).
    let action = exec_action("foot", "foot", "SomethingElseEntirely");
    let event = window_event("new", 99, Some("foot"), Some("SomethingElseEntirely"));
    assert!(matches!(
        action.matches_window_event(&event),
        Ok(WindowEventMatch::WindowAppId)
    ));
}

#[test]
fn non_exec_action_matches_on_container_id() {
    let action = targeted(ActionKind::Floating);
    let event = window_event("floating", 42, None, None);
    assert!(matches!(
        action.matches_window_event(&event),
        Ok(WindowEventMatch::WindowContainerIdMatch)
    ));
}

#[test]
fn workspace_matches_on_container_id() {
    let action = targeted(ActionKind::Workspace(WorkspaceTarget::Name("web")));
    let event = window_event("move", 42, None, None);
    assert!(matches!(
        action.matches_window_event(&event),
        Ok(WindowEventMatch::WindowContainerIdMatch)
    ));
}

#[test]
fn fullscreen_matches_on_container_id() {
    let action = targeted(ActionKind::Fullscreen);
    let event = window_event("fullscreen_mode", 42, None, None);
    assert!(matches!(
        action.matches_window_event(&event),
        Ok(WindowEventMatch::WindowContainerIdMatch)
    ));
}

#[test]
fn scratchpad_matches_on_container_id() {
    let action = targeted(ActionKind::Scratchpad);
    let event = window_event("move", 42, None, None);
    assert!(matches!(
        action.matches_window_event(&event),
        Ok(WindowEventMatch::WindowContainerIdMatch)
    ));
}

#[test]
fn non_exec_action_rejects_different_container_id() {
    let action = targeted(ActionKind::Floating);
    let event = window_event("floating", 99, None, None);
    assert!(matches!(
        action.matches_window_event(&event),
        Err(WindowEventMatchError::NoMatchingEvent)
    ));
}

#[test]
fn non_exec_action_rejects_wrong_change_type() {
    let action = targeted(ActionKind::Floating);
    let event = window_event("mark", 42, None, None);
    assert!(matches!(
        action.matches_window_event(&event),
        Err(WindowEventMatchError::EventChangeTypeMismatch)
    ));
}

#[test]
fn a_polled_action_never_matches_a_window_event() {
    // Used to be an expect() — a polled kind has no confirming event, so
    // asking it to match one is answered rather than panicking.
    let action = targeted(ActionKind::Sticky);
    let event = window_event("floating", 42, None, None);
    assert!(matches!(
        action.matches_window_event(&event),
        Err(WindowEventMatchError::EventChangeTypeMismatch)
    ));
}

// ActionKind::is_event_confirmed

/// One instance of every `ActionKind`. The exhaustive match below has no
/// purpose but to stop compiling when a variant is added, pointing here so the
/// new kind is added to this list too.
fn one_of_every_kind() -> [ActionKind<'static>; 16] {
    fn every_variant_is_listed(kind: ActionKind) {
        match kind {
            ActionKind::Split(_)
            | ActionKind::Floating
            | ActionKind::Sticky
            | ActionKind::Fullscreen
            | ActionKind::Focus
            | ActionKind::NewColumn
            | ActionKind::NewRow
            | ActionKind::Workspace(_)
            | ActionKind::Output(_)
            | ActionKind::WorkspaceToOutput { .. }
            | ActionKind::Mark(_)
            | ActionKind::Height(_)
            | ActionKind::Width(_)
            | ActionKind::Position(_)
            | ActionKind::SwayCommand(_)
            | ActionKind::Scratchpad => {}
        }
    }
    let kinds = [
        ActionKind::Split(Split::H),
        ActionKind::Floating,
        ActionKind::Sticky,
        ActionKind::Fullscreen,
        ActionKind::Focus,
        ActionKind::NewColumn,
        ActionKind::NewRow,
        ActionKind::Workspace(WorkspaceTarget::Name("web")),
        ActionKind::Output("HDMI-A-1"),
        ActionKind::WorkspaceToOutput {
            workspace: WorkspaceTarget::Name("web"),
            output: "HDMI-A-1",
        },
        ActionKind::Mark("foo"),
        ActionKind::Height(Size::Pixels(300)),
        ActionKind::Width(Size::Pixels(300)),
        ActionKind::Position(Position::Center),
        ActionKind::SwayCommand("border none"),
        ActionKind::Scratchpad,
    ];
    kinds.into_iter().for_each(every_variant_is_listed);
    kinds
}

#[test]
fn is_event_confirmed_agrees_with_the_matching_events() {
    let event_confirmed: Vec<ActionKind> = one_of_every_kind()
        .into_iter()
        .filter(ActionKind::is_event_confirmed)
        .collect();
    assert_eq!(
        event_confirmed,
        vec![
            ActionKind::Floating,
            ActionKind::Fullscreen,
            ActionKind::Focus,
            ActionKind::Workspace(WorkspaceTarget::Name("web")),
            ActionKind::Output("HDMI-A-1"),
            ActionKind::Mark("foo"),
            ActionKind::Scratchpad,
        ]
    );
}

// SwayAction::state_satisfies

fn state_with(mutate: impl FnOnce(&mut ContainerState)) -> ContainerState {
    let mut state = ContainerState::default();
    mutate(&mut state);
    state
}

fn workspace_action(workspace: &str) -> SwayAction<'_> {
    targeted(ActionKind::Workspace(WorkspaceTarget::Name(workspace)))
}

fn output_action(output: &str) -> SwayAction<'_> {
    targeted(ActionKind::Output(output))
}

fn mark_action(mark: &str) -> SwayAction<'_> {
    targeted(ActionKind::Mark(mark))
}

fn floating_action() -> SwayAction<'static> {
    targeted(ActionKind::Floating)
}

#[test]
fn state_satisfies_workspace_only_for_the_requested_workspace() {
    let state = state_with(|state| state.workspace = Some("3".to_string()));

    assert!(workspace_action("3").state_satisfies(Some(&state)));
    assert!(!workspace_action("2").state_satisfies(Some(&state)));
}

#[test]
fn state_satisfies_workspace_number_on_any_workspace_with_that_number() {
    let state = state_with(|state| {
        state.workspace = Some("2: web".to_string());
        state.workspace_number = Some(2);
    });

    let by_number = |number| targeted(ActionKind::Workspace(WorkspaceTarget::Number(number)));
    assert!(by_number(2).state_satisfies(Some(&state)));
    assert!(!by_number(3).state_satisfies(Some(&state)));
    assert!(!workspace_action("2").state_satisfies(Some(&state)));
}

#[test]
fn state_satisfies_workspace_and_output_ignore_case_as_sway_does() {
    let state = state_with(|state| {
        state.workspace = Some("Web".to_string());
        state.output = Some("HEADLESS-1".to_string());
        state.workspace_visible = true;
    });

    assert!(workspace_action("web").state_satisfies(Some(&state)));
    assert!(output_action("headless-1").state_satisfies(Some(&state)));
    assert!(!workspace_action("Webs").state_satisfies(Some(&state)));
}

#[test]
fn state_satisfies_output_only_on_the_output_s_visible_workspace() {
    // Sway's `move container to output` brings a window on a hidden
    // workspace of that output onto the visible one, so it isn't there yet.
    let state = state_with(|state| state.output = Some("HEADLESS-1".to_string()));
    assert!(!output_action("HEADLESS-1").state_satisfies(Some(&state)));
}

#[test]
fn state_satisfies_workspace_to_output_needs_both_names_but_not_visibility() {
    let action = targeted(ActionKind::WorkspaceToOutput {
        workspace: WorkspaceTarget::Name("3"),
        output: "HDMI-A-1",
    });
    let state = state_with(|state| {
        state.workspace = Some("3".to_string());
        state.output = Some("hdmi-a-1".to_string());
    });
    assert!(action.state_satisfies(Some(&state)));

    let elsewhere = state_with(|state| {
        state.workspace = Some("3".to_string());
        state.output = Some("DP-1".to_string());
    });
    assert!(!action.state_satisfies(Some(&elsewhere)));
    assert!(action.polls(None));
    assert!(!action.kind.is_event_confirmed());
}

#[test]
fn state_satisfies_workspace_false_when_the_workspace_is_unknown() {
    let state = ContainerState::default();
    assert!(!workspace_action("3").state_satisfies(Some(&state)));
}

#[test]
fn state_satisfies_output_only_for_the_requested_output() {
    let state = state_with(|state| {
        state.output = Some("HEADLESS-1".to_string());
        state.workspace_visible = true;
    });

    assert!(output_action("HEADLESS-1").state_satisfies(Some(&state)));
    assert!(!output_action("HEADLESS-2").state_satisfies(Some(&state)));
}

#[test]
fn state_satisfies_floating_fullscreen_focus_and_scratchpad_read_their_own_flag() {
    let floating = state_with(|state| state.floating = true);
    let fullscreen = state_with(|state| state.fullscreen = true);
    let focused = state_with(|state| state.focused = true);
    let scratchpadded = state_with(|state| state.in_scratchpad = true);

    let fullscreen_action = targeted(ActionKind::Fullscreen);
    let focus_action = targeted(ActionKind::Focus);
    let scratchpad_action = targeted(ActionKind::Scratchpad);

    assert!(floating_action().state_satisfies(Some(&floating)));
    assert!(!floating_action().state_satisfies(Some(&fullscreen)));
    assert!(fullscreen_action.state_satisfies(Some(&fullscreen)));
    assert!(!fullscreen_action.state_satisfies(Some(&floating)));
    assert!(focus_action.state_satisfies(Some(&focused)));
    assert!(!focus_action.state_satisfies(Some(&floating)));
    assert!(scratchpad_action.state_satisfies(Some(&scratchpadded)));
    assert!(!scratchpad_action.state_satisfies(Some(&floating)));
}

#[test]
fn state_satisfies_mark_matches_any_held_mark() {
    let state =
        state_with(|state| state.marks = vec!["other".to_string(), "dropdown-term".to_string()]);

    assert!(mark_action("dropdown-term").state_satisfies(Some(&state)));
    assert!(!mark_action("missing").state_satisfies(Some(&state)));
}

#[test]
fn state_satisfies_is_false_for_an_action_with_no_state_to_check() {
    let state = state_with(|state| state.floating = true);
    let action = targeted(ActionKind::Split(Split::H));

    assert!(!action.state_satisfies(Some(&state)));
}

#[test]
fn every_event_confirmed_kind_can_recognise_its_own_end_state() {
    // run_batch() accepts a matching event only once
    // state_satisfies() agrees, so an event-confirmed kind it can never
    // satisfy would time out after every successful command.
    let state = ContainerState {
        workspace: Some("web".to_string()),
        workspace_number: None,
        output: Some("HDMI-A-1".to_string()),
        workspace_visible: true,
        floating: true,
        fullscreen: true,
        focused: true,
        in_scratchpad: true,
        marks: vec!["foo".to_string()],
    };

    for kind in one_of_every_kind()
        .into_iter()
        .filter(ActionKind::is_event_confirmed)
    {
        assert!(
            targeted(kind).state_satisfies(Some(&state)),
            "{kind:?} is event-confirmed but state_satisfies() never accepts it"
        );
    }
}

#[test]
fn state_satisfies_is_false_when_the_container_is_gone() {
    // A window that has closed isn't floating, focused, or on the target
    // workspace — short-circuiting an action into success over a container
    // that no longer exists would be worse than letting the command fail.
    assert!(!floating_action().state_satisfies(None));
    assert!(!workspace_action("3").state_satisfies(None));
    assert!(!mark_action("dropdown-term").state_satisfies(None));
}

// Display impls for the private matching-result enums

#[test]
fn window_event_match_display() {
    assert_eq!(
        WindowEventMatch::WindowAppId.to_string(),
        "Window app_id match"
    );
    assert_eq!(
        WindowEventMatch::WindowClass.to_string(),
        "Window class match"
    );
    assert_eq!(
        WindowEventMatch::NewWindowMatchWithoutCheck.to_string(),
        "New window without app_id or class check"
    );
    assert_eq!(
        WindowEventMatch::WindowContainerIdMatch.to_string(),
        "Window container id match"
    );
}

#[test]
fn window_event_match_error_display() {
    assert_eq!(
        WindowEventMatchError::EventChangeTypeMismatch.to_string(),
        "Event does not match action event matches"
    );
    assert_eq!(
        WindowEventMatchError::WindowAppIdMismatch.to_string(),
        "app_id mismatch"
    );
    assert_eq!(
        WindowEventMatchError::WindowClassMismatch.to_string(),
        "class mismatch"
    );
    assert_eq!(
        WindowEventMatchError::NoMatchingEvent.to_string(),
        "No matching event"
    );
}

// deadline_after

#[test]
fn deadline_after_returns_a_future_instant() {
    let before = time::Instant::now();
    let deadline = deadline_after(time::Duration::from_secs(5)).expect("5s is representable");
    assert!(deadline >= before + time::Duration::from_secs(5));
}

#[test]
fn deadline_after_errors_instead_of_panicking_on_an_unrepresentable_timeout() {
    let error = deadline_after(time::Duration::from_secs(u64::MAX))
        .expect_err("u64::MAX seconds overflows Instant");
    assert!(error.contains("too large"), "{error}");
}

#[test]
fn only_actions_confirmed_by_a_window_event_need_a_window_target() {
    for kind in one_of_every_kind() {
        let expected = matches!(
            kind,
            ActionKind::Workspace(_) | ActionKind::Output(_) | ActionKind::Focus
        );
        assert_eq!(kind.needs_a_window(), expected, "{kind:?}");
    }
}

#[test]
fn split_container_error_names_the_container_and_the_fix() {
    let message = split_container_error(12);
    assert!(
        message.contains("container 12 is a split container"),
        "{message}"
    );
    assert!(message.contains("target a window inside it"), "{message}");
}

#[test]
fn exec_with_a_title_rejects_a_window_whose_title_does_not_match_yet() {
    let title = super::super::values::parse_title_pattern("project-a").unwrap();
    let action = ExecAction {
        title: Some(&title),
        ..exec_action("code", "", "Code")
    };
    let mut splash = window_event("new", 99, None, Some("Code"));
    splash.container.name = Some("Loading…".to_string());
    assert!(matches!(
        action.matches_window_event(&splash),
        Err(WindowEventMatchError::WindowTitleMismatch)
    ));
    let mut editor = window_event("new", 100, None, Some("Code"));
    editor.container.name = Some("main.rs - project-a".to_string());
    assert!(matches!(
        action.matches_window_event(&editor),
        Ok(WindowEventMatch::WindowClass)
    ));
    // The app_id/class filter is checked first, so a wrong class says so.
    let mut other = window_event("new", 101, None, Some("Firefox"));
    other.container.name = Some("project-a".to_string());
    assert!(matches!(
        action.matches_window_event(&other),
        Err(WindowEventMatchError::WindowClassMismatch)
    ));
}

#[test]
fn exec_timeout_message_names_the_title_it_waited_for() {
    let title = super::super::values::parse_title_pattern("project-a").unwrap();
    assert_eq!(
        exec_timeout_message(
            time::Duration::from_secs(3),
            None,
            Some(&title),
            &[unmatched(12, Some("foot"), None)],
            None,
            false
        ),
        "no window with title matching \"project-a\" appeared within 3 sec; meanwhile 1 other \
         new window opened, not traced to this command (con_id 12, app_id \"foot\", no title) \
         — if it is this app's, check --app-id/--class/--title"
    );
}

// joins_batch / batch_command / batch_failure (#32)

#[test]
fn joins_batch_keeps_moves_and_the_scratchpad_out() {
    let joining: Vec<&str> = [
        ActionKind::Split(Split::V),
        ActionKind::Floating,
        ActionKind::Sticky,
        ActionKind::Fullscreen,
        ActionKind::Focus,
        ActionKind::Mark("m"),
        ActionKind::Height(Size::Pixels(1)),
        ActionKind::Width(Size::Percent(1)),
        ActionKind::Position(Position::Center),
        ActionKind::NewColumn,
        ActionKind::NewRow,
        ActionKind::Workspace(WorkspaceTarget::Name("2")),
        ActionKind::Output("HEADLESS-1"),
        ActionKind::WorkspaceToOutput {
            workspace: WorkspaceTarget::Name("2"),
            output: "HEADLESS-1",
        },
        ActionKind::Scratchpad,
    ]
    .iter()
    .filter(|kind| kind.joins_batch())
    .map(|kind| kind.flag_name())
    .collect();
    assert_eq!(
        joining,
        vec![
            "split",
            "floating",
            "sticky",
            "fullscreen",
            "focus",
            "mark",
            "height",
            "width",
            "position"
        ]
    );
}

#[test]
fn batch_command_puts_every_verb_behind_one_criteria() {
    let floating = targeted(ActionKind::Floating);
    let mark = targeted(ActionKind::Mark("a, b"));
    assert_eq!(
        batch_command(42, &[&floating, &mark]),
        "[con_id=42] floating enable, mark \"a, b\""
    );
}

#[test]
fn batch_failure_names_the_action_whose_part_failed() {
    let floating = targeted(ActionKind::Floating);
    let position = targeted(ActionKind::Position(Position::Coordinates { x: 1, y: 2 }));
    let mark = targeted(ActionKind::Mark("m"));
    let sent = [&floating, &position, &mark];
    assert_eq!(batch_failure(&sent, &[Ok(()), Ok(()), Ok(())]), None);
    assert_eq!(
        batch_failure(&sent, &[Ok(()), Err("refused".to_string()), Ok(())]),
        Some("move position 1 2: refused".to_string())
    );
}

#[test]
fn batch_failure_reports_a_reply_cut_short_without_an_error() {
    let floating = targeted(ActionKind::Floating);
    let mark = targeted(ActionKind::Mark("m"));
    assert_eq!(
        batch_failure(&[&floating, &mark], &[Ok(())]),
        Some("Sway answered 1 of the 2 commands sent together; the rest never ran".to_string())
    );
}

#[test]
fn batch_progress_reports_anything_left_waiting_as_unconfirmed() {
    let outcomes: Vec<ActionOutcome> = [
        BatchProgress::AlreadySatisfied,
        BatchProgress::Changed,
        BatchProgress::Unconfirmed,
        BatchProgress::Waiting,
        BatchProgress::EventSeen,
    ]
    .iter()
    .map(|progress| progress.outcome())
    .collect();
    assert_eq!(
        outcomes,
        vec![
            ActionOutcome::AlreadySatisfied,
            ActionOutcome::Changed,
            ActionOutcome::Unconfirmed,
            ActionOutcome::Unconfirmed,
            ActionOutcome::Unconfirmed,
        ]
    );
}

// is_held_geometry (#17)

#[test]
fn is_held_geometry_is_pixel_sizes_and_positions_only() {
    assert!(is_held_geometry(ActionKind::Height(Size::Pixels(1))));
    assert!(is_held_geometry(ActionKind::Width(Size::Pixels(1))));
    assert!(is_held_geometry(ActionKind::Position(Position::Center)));
    assert!(!is_held_geometry(ActionKind::Height(Size::Percent(1))));
    assert!(!is_held_geometry(ActionKind::Width(Size::Percent(1))));
    assert!(!is_held_geometry(ActionKind::Floating));
}

#[test]
fn a_batch_whose_entries_split_a_quote_between_them_is_refused_before_sending() {
    // #123: neither entry passes validate_sway_command() any more, but the
    // pre-send check refuses their join as well.
    let first = targeted(ActionKind::SwayCommand("title_format \"a"));
    let second = targeted(ActionKind::SwayCommand(
        "border \", exec touch /tmp/x, border \"",
    ));
    let command = batch_command(42, &[&first, &second]);
    assert!(sway_command_split_problem(&command, 2).is_some());
}

#[test]
fn batch_verbs_joins_every_verb_and_is_a_lone_action_s_own_verb() {
    let split = targeted(ActionKind::Split(Split::V));
    let sticky = targeted(ActionKind::Sticky);
    assert_eq!(batch_verbs(&[&split, &sticky]), "splitv, sticky enable");
    assert_eq!(batch_verbs(&[&sticky]), "sticky enable");
}

// geometry_shows (#159)

fn held_window_tree() -> Node {
    let mut leaf = leaf_node_value(42, Some("foot"), None);
    leaf["rect"] = serde_json::json!({"x": 10, "y": 45, "width": 700, "height": 375});
    leaf["deco_rect"] = serde_json::json!({"x": 10, "y": 20, "width": 700, "height": 25});
    state_tree("1", leaf)
}

#[test]
fn geometry_shows_judges_held_sizes_and_positions_from_the_given_tree() {
    let tree = held_window_tree();
    assert!(targeted(ActionKind::Width(Size::Pixels(700))).geometry_shows(&tree));
    assert!(!targeted(ActionKind::Width(Size::Pixels(800))).geometry_shows(&tree));
    assert!(targeted(ActionKind::Height(Size::Pixels(400))).geometry_shows(&tree));
    assert!(!targeted(ActionKind::Height(Size::Pixels(375))).geometry_shows(&tree));
    assert!(
        targeted(ActionKind::Position(Position::Coordinates { x: 10, y: 20 }))
            .geometry_shows(&tree)
    );
    assert!(
        !targeted(ActionKind::Position(Position::Coordinates { x: 0, y: 0 })).geometry_shows(&tree)
    );
}

#[test]
fn geometry_shows_nothing_for_a_container_not_in_the_tree_or_a_kind_it_does_not_hold() {
    let tree = state_tree("1", leaf_node_value(7, Some("foot"), None));
    assert!(!targeted(ActionKind::Width(Size::Pixels(0))).geometry_shows(&tree));
    assert!(!targeted(ActionKind::Width(Size::Percent(50))).geometry_shows(&held_window_tree()));
    assert!(!targeted(ActionKind::Floating).geometry_shows(&held_window_tree()));
}

// poll_shows (#187)

/// `state_tree()`, with the workspace's `layout` set to `layout`, which is
/// what a split of its only window changes.
fn tree_with_workspace_layout(layout: &str, leaf: serde_json::Value) -> Node {
    let mut workspace = named_node_value(2, "workspace", "3", vec![leaf]);
    workspace["layout"] = serde_json::json!(layout);
    let mut output = named_node_value(1, "output", "HEADLESS-1", vec![workspace]);
    output["focus"] = serde_json::json!([2]);
    serde_json::from_value(named_node_value(0, "root", "root", vec![output]))
        .expect("valid Node test fixture")
}

#[test]
fn poll_shows_judges_a_split_by_its_parent_s_layout() {
    let tree = tree_with_workspace_layout("splitv", leaf_node_value(42, Some("foot"), None));
    assert!(targeted(ActionKind::Split(Split::V)).poll_shows(&tree, None));
    assert!(!targeted(ActionKind::Split(Split::H)).poll_shows(&tree, None));
}

#[test]
fn poll_shows_judges_sticky_from_the_window_s_own_field() {
    let mut leaf = leaf_node_value(42, Some("foot"), None);
    assert!(!targeted(ActionKind::Sticky).poll_shows(&state_tree("3", leaf.clone()), None));
    leaf["sticky"] = serde_json::json!(true);
    assert!(targeted(ActionKind::Sticky).poll_shows(&state_tree("3", leaf), None));
}

#[test]
fn poll_shows_judges_held_geometry_as_geometry_shows_does() {
    let tree = held_window_tree();
    for kind in [
        ActionKind::Width(Size::Pixels(700)),
        ActionKind::Width(Size::Pixels(800)),
        ActionKind::Height(Size::Pixels(400)),
        ActionKind::Position(Position::Coordinates { x: 10, y: 20 }),
        ActionKind::Position(Position::Coordinates { x: 0, y: 0 }),
    ] {
        let action = targeted(kind);
        assert_eq!(action.poll_shows(&tree, None), action.geometry_shows(&tree));
    }
    assert!(!targeted(ActionKind::Width(Size::Percent(50))).poll_shows(&tree, None));
}

#[test]
fn poll_shows_a_move_only_once_the_rect_left_its_baseline() {
    let tree = held_window_tree();
    for kind in [ActionKind::NewColumn, ActionKind::NewRow] {
        let action = targeted(kind);
        assert!(!action.poll_shows(&tree, None));
        assert!(!action.poll_shows(&tree, Some(rect(10, 45, 700, 375))));
        assert!(action.poll_shows(&tree, Some(rect(0, 0, 700, 375))));
    }
}

#[test]
fn poll_shows_a_workspace_to_output_move_once_both_names_match() {
    let tree = state_tree("3", leaf_node_value(42, Some("foot"), None));
    let action = |output| {
        targeted(ActionKind::WorkspaceToOutput {
            workspace: WorkspaceTarget::Name("3"),
            output,
        })
    };
    assert!(action("headless-1").poll_shows(&tree, None));
    assert!(!action("DP-1").poll_shows(&tree, None));
}

#[test]
fn poll_shows_nothing_for_a_missing_container_or_a_kind_that_does_not_poll() {
    let elsewhere = state_tree("3", leaf_node_value(7, Some("foot"), None));
    assert!(!targeted(ActionKind::Sticky).poll_shows(&elsewhere, None));
    assert!(!targeted(ActionKind::Split(Split::V)).poll_shows(&elsewhere, None));
    assert!(!targeted(ActionKind::NewColumn).poll_shows(&elsewhere, Some(rect(0, 0, 1, 1))));
    let tree = held_window_tree();
    assert!(!targeted(ActionKind::Floating).poll_shows(&tree, None));
    assert!(!targeted(ActionKind::SwayCommand("border none")).poll_shows(&tree, None));
}
