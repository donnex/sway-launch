use super::*;

// preferred_socket_path

#[test]
fn preferred_socket_path_prefers_swaysock_over_i3sock() {
    assert_eq!(
        preferred_socket_path(Some("/sway.sock".into()), Some("/i3.sock".into())),
        Some(std::path::PathBuf::from("/sway.sock"))
    );
}

#[test]
fn preferred_socket_path_falls_back_to_i3sock() {
    assert_eq!(
        preferred_socket_path(None, Some("/i3.sock".into())),
        Some(std::path::PathBuf::from("/i3.sock"))
    );
}

#[test]
fn preferred_socket_path_is_none_when_neither_is_set() {
    assert_eq!(preferred_socket_path(None, None), None);
}

// quote_sway_string

#[test]
fn quote_sway_string_wraps_plain_value() {
    assert_eq!(quote_sway_string("foo"), "\"foo\"");
}

#[test]
fn quote_sway_string_escapes_embedded_quotes() {
    assert_eq!(quote_sway_string("foo\"bar"), "\"foo\\\"bar\"");
}

#[test]
fn quote_sway_string_escapes_backslashes() {
    assert_eq!(quote_sway_string("foo\\bar"), "\"foo\\\\bar\"");
}

#[test]
fn quote_sway_string_wraps_a_value_containing_a_newline() {
    // Regression test: confirmed live (this project's security review,
    // 2026-08-21) that a literal newline embedded in a --mark value
    // can't break out of the quoting either -- Sway's own parser treats
    // it as part of the quoted literal, not a command separator, the
    // same as the comma/semicolon case below. quote_sway_string()
    // itself needs no special handling for `\n` (only `\`/`"` are
    // escaped) since it's neither of those; this test just pins that
    // the newline survives untouched inside the quotes rather than
    // being stripped or otherwise mishandled. See
    // mark_with_special_characters_is_stored_literally_not_executed in
    // tests/live_sway.rs for the live-Sway proof this is actually safe.
    let injected = "foo\nexec malicious-command";
    let quoted = quote_sway_string(injected);
    assert_eq!(quoted, "\"foo\nexec malicious-command\"");
}

#[test]
fn quote_sway_string_neutralizes_command_separators() {
    // Regression test: an unquoted mark containing a command separator
    // used to let extra Sway commands be injected into the same call.
    let injected = "foo, exec malicious-command";
    let quoted = quote_sway_string(injected);
    assert_eq!(quoted, "\"foo, exec malicious-command\"");
    assert!(!quoted.trim_matches('"').contains('"'));
}

// ipc_error

#[test]
fn ipc_error_explains_a_socket_timeout() {
    // A read that hits IPC_ROUND_TRIP_TIMEOUT arrives as a bare
    // "Resource temporarily unavailable (os error 11)", which says nothing
    // about what happened. Both kinds are mapped: Linux reports a timed-out
    // socket read as WouldBlock, other platforms as TimedOut.
    for kind in [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut] {
        let error = ipc_error(swayipc::Error::Io(std::io::Error::from(kind)));
        assert!(
            error.contains("did not respond") && error.contains("10 sec"),
            "should name the condition and the bound, got {error:?}"
        );
        assert!(
            error.contains("--timeout"),
            "should say which knob this is *not*, got {error:?}"
        );
    }
}

#[test]
fn ipc_error_leaves_other_errors_alone() {
    // swayipc's own wording is already specific for these; rewriting them
    // would lose the compositor's actual complaint.
    let error = ipc_error(swayipc::Error::CommandFailed(
        "No matching node.".to_string(),
    ));
    assert!(
        error.contains("No matching node."),
        "should keep swayipc's own message, got {error:?}"
    );
    assert!(!error.contains("did not respond"));
}

#[test]
fn ipc_error_leaves_a_non_timeout_io_error_alone() {
    let error = ipc_error(swayipc::Error::Io(std::io::Error::from(
        std::io::ErrorKind::ConnectionRefused,
    )));
    assert!(!error.contains("did not respond"), "got {error:?}");
}

// first_outcome_error

#[test]
fn first_outcome_error_ok_when_all_succeed() {
    let outcomes: Vec<Result<(), String>> = vec![Ok(()), Ok(())];
    assert_eq!(first_outcome_error(outcomes, "cmd"), Ok(()));
}

#[test]
fn first_outcome_error_fails_when_empty() {
    let outcomes: Vec<Result<(), String>> = vec![];
    assert_eq!(
        first_outcome_error(outcomes, "cmd"),
        Err("cmd command failed".to_string())
    );
}

#[test]
fn first_outcome_error_surfaces_a_leading_failure() {
    let outcomes: Vec<Result<(), String>> = vec![Err("boom".to_string()), Ok(())];
    assert_eq!(
        first_outcome_error(outcomes, "cmd"),
        Err("boom".to_string())
    );
}

#[test]
fn first_outcome_error_surfaces_a_trailing_failure() {
    // Regression test: a prior version only inspected the first outcome
    // and returned Ok(()) here, silently dropping this failure.
    let outcomes: Vec<Result<(), String>> = vec![Ok(()), Err("boom".to_string())];
    assert_eq!(
        first_outcome_error(outcomes, "cmd"),
        Err("boom".to_string())
    );
}

// single_outcome_error

#[test]
fn single_outcome_error_ok_for_one_success() {
    let outcomes: Vec<Result<(), String>> = vec![Ok(())];
    assert_eq!(single_outcome_error(outcomes, "cmd"), Ok(()));
}

#[test]
fn single_outcome_error_surfaces_a_single_failure() {
    let outcomes: Vec<Result<(), String>> = vec![Err("boom".to_string())];
    assert_eq!(
        single_outcome_error(outcomes, "cmd"),
        Err("boom".to_string())
    );
}

#[test]
fn single_outcome_error_fails_when_sway_split_the_command_even_if_all_succeeded() {
    // The `foot -T x; mark INJECTED` case: every piece succeeded, so
    // first_outcome_error() alone reports success for a split launch.
    let outcomes: Vec<Result<(), String>> = vec![Ok(()), Ok(())];
    let error = single_outcome_error(outcomes, "cmd").expect_err("a split must fail");
    assert!(error.contains("2 separate commands"), "{error:?}");
}
