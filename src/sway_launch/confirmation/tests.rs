use super::super::test_support::*;
use super::super::values::Position;
use super::*;

// exec_timeout_message / event_timeout_message

fn unmatched(container_id: i64, app_id: Option<&str>, class: Option<&str>) -> UnmatchedWindow {
    UnmatchedWindow {
        container_id,
        app_id: app_id.map(str::to_string),
        class: class.map(str::to_string),
        launched_by_this_run: false,
    }
}

#[test]
fn exec_timeout_message_names_a_window_the_filter_rejected() {
    let message = exec_timeout_message(
        time::Duration::from_secs(2),
        "notfoot",
        "",
        &[UnmatchedWindow {
            launched_by_this_run: true,
            ..unmatched(12, Some("foot"), None)
        }],
        Some(true),
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
        "",
        "Firefox",
        &[
            unmatched(12, Some("foot"), None),
            unmatched(13, None, Some("XTerm")),
            unmatched(14, None, None),
        ],
        None,
    );
    assert_eq!(
        message,
        "no window with class \"Firefox\" appeared within 5 sec; 3 other new windows did \
             (con_id 12, app_id \"foot\"), (con_id 13, class \"XTerm\"), \
             (con_id 14, no app_id or class) — check --app-id/--class"
    );
    assert!(!message.contains("rechecked"), "{message}");
}

#[test]
fn exec_timeout_message_names_a_launched_window_that_never_set_its_class() {
    let message = exec_timeout_message(
        time::Duration::from_secs(5),
        "",
        "Spotify",
        &[UnmatchedWindow {
            launched_by_this_run: true,
            ..unmatched(12, None, None)
        }],
        None,
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
fn exec_timeout_message_says_when_the_command_exited_without_a_window() {
    let message = exec_timeout_message(time::Duration::from_secs(2), "", "", &[], Some(false));
    assert_eq!(
        message,
        "no window appeared within 2 sec: the launched command exited without opening a \
             window — check that it is on PATH in Sway's environment"
    );
}

#[test]
fn exec_timeout_message_suggests_slowness_only_while_the_command_still_runs() {
    let message = exec_timeout_message(time::Duration::from_secs(2), "", "", &[], Some(true));
    assert!(message.contains("still running"), "{message}");
    assert!(message.contains("--timeout"), "{message}");
}

#[test]
fn exec_timeout_message_adds_nothing_when_the_command_s_state_is_unknown() {
    assert_eq!(
        exec_timeout_message(time::Duration::from_secs(3), "foot", "", &[], None),
        "no window with app_id \"foot\" appeared within 3 sec"
    );
}

#[test]
fn exec_timeout_message_prefers_rejected_windows_over_the_command_s_state() {
    let message = exec_timeout_message(
        time::Duration::from_secs(2),
        "notfoot",
        "",
        &[unmatched(12, Some("foot"), None)],
        Some(false),
    );
    assert!(message.contains("check --app-id/--class"), "{message}");
    assert!(!message.contains("exited"), "{message}");
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
    let action = targeted(ActionKind::Workspace("web"));
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
    let action = targeted(ActionKind::Workspace("web"));
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
fn one_of_every_kind() -> [ActionKind<'static>; 14] {
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
            | ActionKind::Mark(_)
            | ActionKind::Height(_)
            | ActionKind::Width(_)
            | ActionKind::Position(_)
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
        ActionKind::Workspace("web"),
        ActionKind::Output("HDMI-A-1"),
        ActionKind::Mark("foo"),
        ActionKind::Height(Size::Pixels(300)),
        ActionKind::Width(Size::Pixels(300)),
        ActionKind::Position(Position::Center),
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
            ActionKind::Workspace("web"),
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
    targeted(ActionKind::Workspace(workspace))
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
fn state_satisfies_workspace_false_when_the_workspace_is_unknown() {
    let state = ContainerState::default();
    assert!(!workspace_action("3").state_satisfies(Some(&state)));
}

#[test]
fn state_satisfies_output_only_for_the_requested_output() {
    let state = state_with(|state| state.output = Some("HEADLESS-1".to_string()));

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
    // run_wait_matching_events() accepts a matching event only once
    // state_satisfies() agrees, so an event-confirmed kind it can never
    // satisfy would time out after every successful command.
    let state = ContainerState {
        workspace: Some("web".to_string()),
        output: Some("HDMI-A-1".to_string()),
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
