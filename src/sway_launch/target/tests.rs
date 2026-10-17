use super::super::test_support::*;
use super::*;

fn fields() -> TargetFields<'static> {
    TargetFields::default()
}

// Target::from_fields

#[test]
fn from_fields_builds_a_launch_with_its_filter() {
    let target = Target::from_fields(TargetFields {
        command: Some("foot"),
        app_id: Some("foot"),
        ..fields()
    });
    assert_eq!(
        target,
        Ok(Target::Exec {
            command: "foot",
            filter: Some(LaunchFilter::AppId("foot"))
        })
    );

    let unfiltered = Target::from_fields(TargetFields {
        command: Some("foot"),
        ..fields()
    });
    assert_eq!(
        unfiltered,
        Ok(Target::Exec {
            command: "foot",
            filter: None
        })
    );
}

#[test]
fn from_fields_treats_a_resolved_target_id_as_a_container_id() {
    let target = Target::from_fields(TargetFields {
        target_id: Some(7),
        ..fields()
    });
    assert_eq!(target, Ok(Target::ConId(7)));
}

#[test]
fn from_fields_builds_an_existing_target_from_each_matcher() {
    for (fields, expected) in [
        (
            TargetFields {
                app_id: Some("foot"),
                ..fields()
            },
            WindowMatch::AppId("foot"),
        ),
        (
            TargetFields {
                class: Some("Code"),
                ..fields()
            },
            WindowMatch::Class("Code"),
        ),
        (
            TargetFields {
                mark_match: Some("dropdown"),
                ..fields()
            },
            WindowMatch::Mark("dropdown"),
        ),
    ] {
        let fields = TargetFields {
            existing: true,
            ..fields
        };
        assert_eq!(
            Target::from_fields(fields),
            Ok(Target::Existing {
                window_match: expected,
                wait: false,
            })
        );
    }
}

#[test]
fn wait_carries_through_to_an_existing_target() {
    let fields = TargetFields {
        existing: true,
        wait: true,
        app_id: Some("firefox"),
        ..fields()
    };
    assert_eq!(
        Target::from_fields(fields),
        Ok(Target::Existing {
            window_match: WindowMatch::AppId("firefox"),
            wait: true,
        })
    );
}

#[test]
fn wait_without_existing_is_rejected_in_each_front_end_s_words() {
    for fields in [
        TargetFields {
            wait: true,
            command: Some("firefox"),
            ..fields()
        },
        TargetFields {
            wait: true,
            con_id: Some(5),
            ..fields()
        },
    ] {
        let error = Target::from_fields(fields);
        assert_eq!(error, Err(TargetError::WaitWithoutExisting));
    }
    let error = TargetError::WaitWithoutExisting;
    assert_eq!(error.message(FieldStyle::Cli), "--wait requires --existing");
    assert_eq!(
        error.message(FieldStyle::Binding),
        "wait = true requires existing = true"
    );
}

#[test]
fn from_fields_rejects_every_combination_the_rules_forbid() {
    for (fields, expected) in [
        (fields(), TargetError::NoTarget),
        (
            TargetFields {
                command: Some("foot"),
                con_id: Some(1),
                ..fields()
            },
            TargetError::SeveralTargets,
        ),
        (
            TargetFields {
                con_id: Some(1),
                target_id: Some(2),
                ..fields()
            },
            TargetError::SeveralTargets,
        ),
        (
            TargetFields {
                existing: true,
                app_id: Some("foot"),
                class: Some("Foot"),
                ..fields()
            },
            TargetError::SeveralMatchers,
        ),
        (
            TargetFields {
                con_id: Some(1),
                app_id: Some("foot"),
                ..fields()
            },
            TargetError::MatcherWithContainerId,
        ),
        (
            TargetFields {
                target_id: Some(1),
                mark_match: Some("m"),
                ..fields()
            },
            TargetError::MatcherWithContainerId,
        ),
        (
            TargetFields {
                command: Some("foot"),
                mark_match: Some("m"),
                ..fields()
            },
            TargetError::MarkMatchWithoutExisting,
        ),
        (
            TargetFields {
                existing: true,
                ..fields()
            },
            TargetError::ExistingWithoutMatcher,
        ),
        (
            TargetFields {
                command: Some("   "),
                ..fields()
            },
            TargetError::Blank("command"),
        ),
        (
            TargetFields {
                existing: true,
                class: Some(" "),
                ..fields()
            },
            TargetError::Blank("class"),
        ),
    ] {
        assert_eq!(
            Target::from_fields(fields),
            Err(expected.clone()),
            "{fields:?}"
        );
    }
}

#[test]
fn from_fields_rejects_a_command_sway_would_split() {
    let error = Target::from_fields(TargetFields {
        command: Some("foot; firefox"),
        ..fields()
    })
    .unwrap_err();
    assert!(matches!(error, TargetError::Command(_)), "{error:?}");
    assert!(error
        .message(FieldStyle::LayoutStep)
        .starts_with("command: "));
    assert!(error.message(FieldStyle::Cli).starts_with("COMMAND: "));
}

// TargetError::message

#[test]
fn message_names_fields_the_way_each_front_end_spells_them() {
    let error = TargetError::ExistingWithoutMatcher;
    assert_eq!(
        error.message(FieldStyle::Cli),
        "--existing requires --app-id, --class, or --mark-match"
    );
    assert_eq!(
        error.message(FieldStyle::LayoutStep),
        "existing = true requires app_id, class, or mark_match"
    );

    assert_eq!(
        TargetError::NoTarget.message(FieldStyle::Cli),
        "Missing COMMAND, or one of --con-id/--existing"
    );
    assert_eq!(
        TargetError::NoTarget.message(FieldStyle::LayoutStep),
        "needs one of: command, con_id, existing, target_id"
    );
    assert_eq!(
        TargetError::NoTarget.message(FieldStyle::Binding),
        "needs one of: command, con_id, existing"
    );
    assert_eq!(
        TargetError::Blank("app_id").message(FieldStyle::Cli),
        "--app-id must not be empty or whitespace-only"
    );
    assert_eq!(
        TargetError::MatcherWithContainerId.message(FieldStyle::Binding),
        "must not combine con_id with app_id/class/mark_match"
    );
}

#[test]
fn every_error_has_a_message_in_every_style() {
    for error in [
        TargetError::NoTarget,
        TargetError::SeveralTargets,
        TargetError::SeveralMatchers,
        TargetError::MatcherWithContainerId,
        TargetError::MarkMatchWithoutExisting,
        TargetError::ExistingWithoutMatcher,
        TargetError::Blank("mark_match"),
        TargetError::Command("bad".to_string()),
    ] {
        for style in [FieldStyle::Cli, FieldStyle::LayoutStep, FieldStyle::Binding] {
            assert!(!error.message(style).is_empty(), "{error:?} {style:?}");
        }
    }
}

// WindowMatch / LaunchFilter

#[test]
fn window_match_reports_its_field_value_and_criterion() {
    let window_match = WindowMatch::Mark("dropdown");
    assert_eq!(window_match.field_name(), "mark_match");
    assert_eq!(window_match.value(), "dropdown");
    assert_eq!(window_match.to_string(), "mark \"dropdown\"");
    assert_eq!(WindowMatch::AppId("foot").field_name(), "app_id");
    assert_eq!(WindowMatch::Class("Code").to_string(), "class \"Code\"");
    assert_eq!(LaunchFilter::Class("Code").to_string(), "class \"Code\"");
    assert_eq!(LaunchFilter::AppId("foot").to_string(), "app_id \"foot\"");
}

#[test]
fn launch_filter_matches_app_id_or_class() {
    let event = window_event("new", 1, None, Some("Firefox"));
    assert!(LaunchFilter::Class("Firefox").matches(&event.container));
    assert!(!LaunchFilter::AppId("Firefox").matches(&event.container));
}
