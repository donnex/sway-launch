use super::*;

// preferred_socket_path

#[test]
fn preferred_socket_path_prefers_swaysock_over_i3sock() {
    assert_eq!(
        preferred_socket_path(Some("/sway.sock".into()), Some("/i3.sock".into())),
        Some(SocketPath {
            path: std::path::PathBuf::from("/sway.sock"),
            variable: "SWAYSOCK",
        })
    );
}

#[test]
fn preferred_socket_path_falls_back_to_i3sock() {
    assert_eq!(
        preferred_socket_path(None, Some("/i3.sock".into())),
        Some(SocketPath {
            path: std::path::PathBuf::from("/i3.sock"),
            variable: "I3SOCK",
        })
    );
}

#[test]
fn preferred_socket_path_is_none_when_neither_is_set() {
    assert_eq!(preferred_socket_path(None, None), None);
}

// connect_error_at / undiscovered_socket_error

#[test]
fn connect_error_at_names_sway_ipc_the_path_its_source_and_the_cause() {
    let socket = SocketPath {
        path: std::path::PathBuf::from("/tmp/example-sway.sock"),
        variable: "I3SOCK",
    };
    let error = std::io::Error::from(std::io::ErrorKind::NotFound);
    let message = connect_error_at(&socket, &error);
    assert!(message.starts_with("cannot reach Sway IPC at /tmp/example-sway.sock (from I3SOCK): "));
    assert!(message.contains(&error.to_string()));
    assert!(message.ends_with("is this running inside a Sway session?"));
}

#[test]
fn undiscovered_socket_error_names_both_variables_and_how_to_fix_it() {
    let message = undiscovered_socket_error(swayipc::Error::SocketNotFound);
    assert!(message.starts_with("cannot reach Sway IPC: neither SWAYSOCK nor I3SOCK is set"));
    assert!(message.contains("export SWAYSOCK"));
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
    // Regression test: confirmed live that a literal newline embedded in a --mark value
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

// version_reply_length / parse_version_reply / version_report

fn version_header(length: u32, message_type: u32) -> [u8; 14] {
    let mut header = [0_u8; 14];
    header[..6].copy_from_slice(&swayipc::MAGIC);
    header[6..10].copy_from_slice(&length.to_ne_bytes());
    header[10..14].copy_from_slice(&message_type.to_ne_bytes());
    header
}

fn sway_reply(major: i64, minor: i64, patch: i64) -> CompositorVersion {
    CompositorVersion::Reply(VersionReply {
        major,
        minor,
        patch,
        human_readable: format!("{}.{}", major, minor),
        variant: Some("sway".to_string()),
    })
}

#[test]
fn version_reply_length_reads_a_get_version_header() {
    assert_eq!(version_reply_length(&version_header(120, 7)), Ok(120));
}

#[test]
fn version_reply_length_rejects_a_header_without_the_magic() {
    let mut header = version_header(120, 7);
    header[0] = b'x';
    assert!(version_reply_length(&header)
        .unwrap_err()
        .contains("no i3-ipc header"));
}

#[test]
fn version_reply_length_rejects_another_message_type() {
    assert!(version_reply_length(&version_header(120, 4))
        .unwrap_err()
        .contains("message type 4, not GET_VERSION (7)"));
}

#[test]
fn version_reply_length_rejects_an_implausible_length() {
    assert!(version_reply_length(&version_header(u32::MAX, 7))
        .unwrap_err()
        .contains("announces"));
}

#[test]
fn parse_version_reply_reads_sways_reply() {
    let payload = br#"{"human_readable": "1.9", "variant": "sway", "major": 1, "minor": 9,
        "patch": 0, "loaded_config_file_name": "/dev/null"}"#;
    assert_eq!(
        parse_version_reply(payload),
        CompositorVersion::Reply(VersionReply {
            major: 1,
            minor: 9,
            patch: 0,
            human_readable: "1.9".to_string(),
            variant: Some("sway".to_string()),
        })
    );
}

#[test]
fn parse_version_reply_reads_a_reply_without_a_variant() {
    let payload = br#"{"human_readable": "4.23", "major": 4, "minor": 23, "patch": 0}"#;
    assert!(matches!(
        parse_version_reply(payload),
        CompositorVersion::Reply(VersionReply { variant: None, .. })
    ));
}

#[test]
fn parse_version_reply_is_unrecognized_without_the_version_fields() {
    assert!(matches!(
        parse_version_reply(br#"{"success": false}"#),
        CompositorVersion::Unrecognized(reason) if reason.starts_with("not a version reply")
    ));
}

#[test]
fn version_report_is_silent_from_the_oldest_supported_sway_on() {
    for (major, minor) in [(1, 9), (1, 11), (1, 12), (2, 0)] {
        let (line, warning) = version_report(&sway_reply(major, minor, 0));
        assert_eq!(warning, None, "{major}.{minor}");
        assert!(
            line.starts_with(&format!("Sway version: {major}.{minor}.0")),
            "{line}"
        );
    }
}

#[test]
fn version_report_warns_about_an_older_sway() {
    let (_, warning) = version_report(&sway_reply(1, 7, 0));
    assert_eq!(
        warning.as_deref(),
        Some(
            "Sway 1.7.0 is older than 1.9, the oldest version sway-launch supports; results may \
             be wrong"
        )
    );
    assert!(version_report(&sway_reply(0, 15, 0)).1.is_some());
}

#[test]
fn version_report_warns_about_a_compositor_that_is_not_sway() {
    let reply = CompositorVersion::Reply(VersionReply {
        major: 4,
        minor: 23,
        patch: 0,
        human_readable: "4.23 (2023-10-29)".to_string(),
        variant: None,
    });
    let (line, warning) = version_report(&reply);
    assert!(line.contains("not Sway"), "{line}");
    let warning = warning.unwrap();
    assert!(warning.contains("is not Sway"), "{warning}");
    assert!(
        warning.contains("\"4.23 (2023-10-29)\", no variant"),
        "{warning}"
    );
    assert!(warning.contains("Sway 1.9 and newer"), "{warning}");
}

#[test]
fn version_report_escapes_a_reported_version_s_control_characters() {
    let reply = CompositorVersion::Reply(VersionReply {
        major: 4,
        minor: 0,
        patch: 0,
        human_readable: "4.0\u{1b}[2J".to_string(),
        variant: Some("other\r".to_string()),
    });
    let (line, warning) = version_report(&reply);
    for text in [line, warning.unwrap()] {
        assert!(!text.chars().any(char::is_control), "{text:?}");
    }
}

#[test]
fn version_report_warns_about_an_unrecognized_reply() {
    let (line, warning) = version_report(&CompositorVersion::Unrecognized(
        "the reply has no i3-ipc header".to_string(),
    ));
    assert!(line.contains("unrecognized reply"), "{line}");
    assert!(warning
        .unwrap()
        .contains("didn't answer a version request as Sway does (the reply has no i3-ipc header)"));
}
