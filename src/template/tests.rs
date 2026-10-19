use super::*;
use crate::sway_launch::DirectoryCheck;

fn minimal_step() -> TemplateStep {
    TemplateStep {
        slot: Some("editor".to_string()),
        target_id: None,
        split: None,
        floating: false,
        sticky: false,
        fullscreen: false,
        focus: false,
        mark: None,
        new_column: false,
        new_row: false,
        workspace: None,
        workspace_number: None,
        output: None,
        height: None,
        width: None,
        position: None,
        scratchpad: false,
        sway_commands: Vec::new(),
        timeout: None,
        wait_time: None,
        settle: None,
    }
}

fn minimal_metadata() -> TemplateMetadata {
    TemplateMetadata {
        description: "A test template.".to_string(),
        category: "Test".to_string(),
    }
}

fn minimal_template(step: Vec<TemplateStep>) -> Template {
    Template {
        template: minimal_metadata(),
        layout: TemplateLayoutContext::default(),
        params: Default::default(),
        step,
    }
}

fn minimal_binding() -> Binding {
    Binding {
        slot: "editor".to_string(),
        command: Some("foot".to_string()),
        cwd: None,
        env: Default::default(),
        con_id: None,
        existing: false,
        wait: false,
        app_id: None,
        class: None,
        mark_match: None,
        title: None,
        workspace: None,
        workspace_number: None,
        output: None,
        timeout: None,
        wait_time: None,
        settle: None,
    }
}

#[test]
fn resolve_rejects_step_without_slot_or_target_id() {
    let mut step = minimal_step();
    step.slot = None;
    let template = minimal_template(vec![step]);
    let bindings = Bindings {
        binding: vec![],
        ..Default::default()
    };
    assert!(resolve(&template, &bindings, &[], DirectoryCheck::Exists).is_err());
}

#[test]
fn resolve_reports_a_template_step_error_with_its_1_based_step_number() {
    let mut second = minimal_step();
    second.slot = None;
    let template = minimal_template(vec![minimal_step(), second]);
    let bindings = Bindings {
        binding: vec![minimal_binding()],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .err()
        .expect("a step with neither slot nor target_id should be rejected");
    assert_eq!(
        error,
        ResolveError::Step {
            index: 1,
            message: "template step must set exactly one of: slot, target_id".to_string(),
        }
    );
    assert_eq!(
        error.to_string(),
        "step 2: template step must set exactly one of: slot, target_id"
    );
}

#[test]
fn resolve_reports_a_blank_target_id_as_a_step_error() {
    let mut step = minimal_step();
    step.slot = None;
    step.target_id = Some("  ".to_string());
    let template = minimal_template(vec![step]);
    let bindings = Bindings {
        binding: vec![],
        ..Default::default()
    };
    assert!(matches!(
        resolve(&template, &bindings, &[], DirectoryCheck::Exists),
        Err(ResolveError::Step { index: 0, .. })
    ));
}

#[test]
fn resolve_reports_binding_mistakes_as_bindings_errors_without_a_step_number() {
    let template = minimal_template(vec![minimal_step()]);
    let mut existing_without_matcher = minimal_binding();
    existing_without_matcher.command = None;
    existing_without_matcher.existing = true;
    let mut unused = minimal_binding();
    unused.slot = "terminal".to_string();
    for bindings in [
        vec![],
        vec![existing_without_matcher],
        vec![minimal_binding(), minimal_binding()],
        vec![minimal_binding(), unused],
    ] {
        let error = resolve(
            &template,
            &Bindings {
                binding: bindings,
                ..Default::default()
            },
            &[],
            DirectoryCheck::Exists,
        )
        .err()
        .expect("a binding mistake should be rejected");
        assert!(
            matches!(error, ResolveError::Bindings(_)),
            "a binding mistake belongs to the bindings, not a template step: {error:?}"
        );
        assert!(!error.to_string().starts_with("step "), "{error}");
    }
}

#[test]
fn resolve_reports_a_binding_s_cwd_and_env_mistakes_against_the_bindings() {
    let template = minimal_template(vec![minimal_step()]);
    let missing_directory = std::env::temp_dir().join("sway-launch-no-such-dir-example");
    let missing_directory = missing_directory.to_str().unwrap().to_string();
    let mut without_command = minimal_binding();
    without_command.command = None;
    without_command.existing = true;
    without_command.app_id = Some("foot".to_string());
    without_command.cwd = Some("/".to_string());
    let cases = [
        (
            Binding {
                cwd: Some(missing_directory.clone()),
                ..minimal_binding()
            },
            "cwd: ",
        ),
        (
            Binding {
                env: [("1BAD".to_string(), "x".to_string())].into(),
                ..minimal_binding()
            },
            "env: \"1BAD\"",
        ),
        (
            Binding {
                env: [("A".to_string(), "has $dollar".to_string())].into(),
                ..minimal_binding()
            },
            "env A: '$'",
        ),
        (
            without_command,
            "cwd and env only apply to a launched command",
        ),
    ];
    for (binding, expected) in cases {
        let error = resolve(
            &template,
            &Bindings {
                binding: vec![binding],
                ..Default::default()
            },
            &[],
            DirectoryCheck::Exists,
        )
        .err()
        .expect("a bad cwd or env should be rejected");
        let ResolveError::Bindings(message) = &error else {
            panic!("a binding's cwd/env mistake belongs to the bindings: {error:?}");
        };
        assert!(
            message.starts_with("binding for slot \"editor\": ") && message.contains(expected),
            "{message}"
        );
    }

    let format_only = resolve(
        &template,
        &Bindings {
            binding: vec![Binding {
                cwd: Some(missing_directory),
                ..minimal_binding()
            }],
            ..Default::default()
        },
        &[],
        DirectoryCheck::FormatOnly,
    );
    assert!(format_only.is_ok(), "{:?}", format_only.err());
}

#[test]
fn resolve_rejects_slot_and_target_id_together() {
    let mut step = minimal_step();
    step.target_id = Some("editor".to_string());
    let template = minimal_template(vec![step]);
    let bindings = Bindings {
        binding: vec![minimal_binding()],
        ..Default::default()
    };
    assert!(resolve(&template, &bindings, &[], DirectoryCheck::Exists).is_err());
}

#[test]
fn resolve_rejects_blank_slot() {
    let mut step = minimal_step();
    step.slot = Some("   ".to_string());
    let template = minimal_template(vec![step]);
    let bindings = Bindings {
        binding: vec![],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("blank slot should be rejected");
    assert!(
        error.contains("slot"),
        "error should name the field: {error:?}"
    );
}

#[test]
fn resolve_rejects_blank_target_id() {
    let mut step = minimal_step();
    step.slot = None;
    step.target_id = Some("  ".to_string());
    let template = minimal_template(vec![step]);
    let bindings = Bindings {
        binding: vec![],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("blank target_id should be rejected");
    assert!(
        error.contains("target_id"),
        "error should name the field: {error:?}"
    );
}

#[test]
fn resolve_rejects_blank_binding_slot() {
    let template = minimal_template(vec![minimal_step()]);
    let mut binding = minimal_binding();
    binding.slot = "   ".to_string();
    let bindings = Bindings {
        binding: vec![binding],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("blank binding slot should be rejected");
    assert!(
        error.contains("slot"),
        "error should name the field: {error:?}"
    );
}

#[test]
fn resolve_rejects_blank_binding_command_naming_the_slot() {
    let template = minimal_template(vec![minimal_step()]);
    let mut binding = minimal_binding();
    binding.command = Some("   ".to_string());
    let bindings = Bindings {
        binding: vec![binding],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("blank binding command should be rejected");
    assert!(
        error.contains("editor") && error.contains("command"),
        "error should name the offending slot and field: {:?}",
        error
    );
}

#[test]
fn parse_rejects_two_steps_sharing_a_slot_name() {
    let error = parse(
        "[template]\ndescription = \"A test template.\"\ncategory = \"Test\"\n\n\
         [[step]]\nslot = \"editor\"\n\n[[step]]\nslot = \"editor\"\n",
    )
    .err()
    .expect("two steps sharing a slot should be rejected when the template is parsed");
    assert!(
        error.contains("editor") && error.contains("more than once"),
        "error should name the offending slot and describe the actual mistake: {:?}",
        error
    );
}

#[test]
fn resolve_fills_in_the_binding_and_sets_id_to_the_slot_name() {
    let mut step = minimal_step();
    step.floating = true;
    step.mark = Some("pinned".to_string());
    step.sway_commands = vec!["border none".to_string()];
    let template = minimal_template(vec![step]);
    let bindings = Bindings {
        binding: vec![minimal_binding()],
        ..Default::default()
    };

    let resolved = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .expect("valid template should resolve");
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].id, Some("editor".to_string()));
    assert_eq!(resolved[0].command, Some("foot".to_string()));
    assert!(resolved[0].floating);
    assert_eq!(resolved[0].mark, Some("pinned".to_string()));
    assert_eq!(resolved[0].sway_commands, ["border none"]);
}

#[test]
fn parse_rejects_a_template_sway_command_acting_on_the_session() {
    let error = parse(
        "[template]\ndescription = \"A test template.\"\ncategory = \"Test\"\n\n\
         [[step]]\nslot = \"editor\"\nsway_commands = [\"bindsym Mod4+q exec foot\"]\n",
    )
    .err()
    .expect("bindsym must be refused");
    assert!(error.contains("acts on one window"), "{error}");
}

#[test]
fn parse_rejects_a_template_sway_command_only_the_command_line_may_send() {
    let error = parse(
        "[template]\ndescription = \"A test template.\"\ncategory = \"Test\"\n\n\
         [[step]]\nslot = \"editor\"\nsway_commands = [\"shortcuts_inhibitor enable\"]\n",
    )
    .err()
    .expect("shortcuts_inhibitor must be refused in a template");
    assert!(error.contains("--sway-command"), "{error}");
}

#[test]
fn resolve_leaves_the_template_layout_context_to_the_run_rather_than_each_step() {
    // The run focuses [layout]'s workspace/output once before the first step
    // (run.rs's BuildLocation); copying it into every step as a move built
    // the shape elsewhere and scrambled it on arrival (#45).
    let mut template = minimal_template(vec![minimal_step()]);
    template.layout.workspace = Some("3".to_string());
    template.layout.output = Some("HDMI-A-1".to_string());
    let bindings = Bindings {
        binding: vec![minimal_binding()],
        ..Default::default()
    };

    let resolved = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .expect("valid template should resolve");
    assert_eq!(resolved[0].workspace, None);
    assert_eq!(resolved[0].output, None);
}

#[test]
fn resolve_keeps_a_step_s_own_workspace_and_output_alongside_a_layout_context() {
    let mut step = minimal_step();
    step.workspace = Some("5".to_string());
    step.output = Some("DP-1".to_string());
    let mut template = minimal_template(vec![step]);
    template.layout.workspace = Some("3".to_string());
    template.layout.output = Some("HDMI-A-1".to_string());
    let bindings = Bindings {
        binding: vec![minimal_binding()],
        ..Default::default()
    };

    let resolved = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .expect("valid template should resolve");
    assert_eq!(resolved[0].workspace, Some("5".to_string()));
    assert_eq!(resolved[0].output, Some("DP-1".to_string()));
}

#[test]
fn resolve_leaves_workspace_and_output_unset_without_a_layout_context() {
    let template = minimal_template(vec![minimal_step()]);
    let bindings = Bindings {
        binding: vec![minimal_binding()],
        ..Default::default()
    };

    let resolved = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .expect("valid template should resolve");
    assert_eq!(resolved[0].workspace, None);
    assert_eq!(resolved[0].output, None);
}

#[test]
fn resolve_errors_on_missing_binding() {
    let template = minimal_template(vec![minimal_step()]);
    let bindings = Bindings {
        binding: vec![],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("missing binding should error");
    assert!(error.contains("editor"));
}

#[test]
fn resolve_errors_on_binding_with_no_target_field_naming_the_slot() {
    let template = minimal_template(vec![minimal_step()]);
    let bindings = Bindings {
        binding: vec![Binding {
            slot: "editor".to_string(),
            command: None,
            cwd: None,
            env: Default::default(),
            con_id: None,
            existing: false,
            wait: false,
            app_id: None,
            class: None,
            mark_match: None,
            title: None,
            workspace: None,
            workspace_number: None,
            output: None,
            timeout: None,
            wait_time: None,
            settle: None,
        }],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("binding with no target field should error");
    assert!(
        error.contains("editor"),
        "error should name the offending slot: {:?}",
        error
    );
    assert!(
        error.contains("binding"),
        "error should be worded in terms of the binding, not a layout step: {:?}",
        error
    );
}

#[test]
fn resolve_errors_on_binding_with_command_and_con_id_together() {
    let template = minimal_template(vec![minimal_step()]);
    let bindings = Bindings {
        binding: vec![Binding {
            slot: "editor".to_string(),
            command: Some("foot".to_string()),
            cwd: None,
            env: Default::default(),
            con_id: Some(42),
            existing: false,
            wait: false,
            app_id: None,
            class: None,
            mark_match: None,
            title: None,
            workspace: None,
            workspace_number: None,
            output: None,
            timeout: None,
            wait_time: None,
            settle: None,
        }],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("binding with command and con_id together should error");
    assert!(error.contains("editor"));
}

#[test]
fn resolve_errors_on_binding_with_app_id_and_class_together() {
    let template = minimal_template(vec![minimal_step()]);
    let bindings = Bindings {
        binding: vec![Binding {
            slot: "editor".to_string(),
            command: Some("foot".to_string()),
            cwd: None,
            env: Default::default(),
            con_id: None,
            existing: false,
            wait: false,
            app_id: Some("foot".to_string()),
            class: Some("Foot".to_string()),
            mark_match: None,
            title: None,
            workspace: None,
            workspace_number: None,
            output: None,
            timeout: None,
            wait_time: None,
            settle: None,
        }],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("binding with app_id and class together should error");
    assert!(error.contains("editor"));
}

#[test]
fn resolve_errors_on_binding_with_class_and_mark_match_together() {
    let template = minimal_template(vec![minimal_step()]);
    let mut binding = minimal_binding();
    binding.command = None;
    binding.existing = true;
    binding.class = Some("Foot".to_string());
    binding.mark_match = Some("dropdown-term".to_string());
    let bindings = Bindings {
        binding: vec![binding],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("binding with class and mark_match together should error");
    assert!(error.contains("editor"));
}

#[test]
fn resolve_rejects_a_blank_binding_app_id_naming_the_slot() {
    let template = minimal_template(vec![minimal_step()]);
    let mut binding = minimal_binding();
    binding.command = None;
    binding.existing = true;
    binding.app_id = Some("   ".to_string());
    let bindings = Bindings {
        binding: vec![binding],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("a blank binding app_id should be rejected");
    assert!(
        error.contains("editor") && error.contains("app_id"),
        "error should name the offending slot and field: {error:?}"
    );
}

#[test]
fn resolve_rejects_a_blank_binding_class_naming_the_slot() {
    let template = minimal_template(vec![minimal_step()]);
    let mut binding = minimal_binding();
    binding.command = None;
    binding.existing = true;
    binding.class = Some("".to_string());
    let bindings = Bindings {
        binding: vec![binding],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("a blank binding class should be rejected");
    assert!(
        error.contains("editor") && error.contains("class"),
        "error should name the offending slot and field: {error:?}"
    );
}

#[test]
fn resolve_rejects_binding_mark_match_without_existing() {
    let template = minimal_template(vec![minimal_step()]);
    let mut binding = minimal_binding();
    binding.mark_match = Some("dropdown-term".to_string());
    let bindings = Bindings {
        binding: vec![binding],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("binding mark_match without existing should be rejected");
    assert!(error.contains("editor"));
    assert!(error.contains("mark_match"));
}

#[test]
fn resolve_rejects_binding_existing_without_a_matcher_naming_the_slot() {
    let template = minimal_template(vec![minimal_step()]);
    let mut binding = minimal_binding();
    binding.command = None;
    binding.existing = true;
    let bindings = Bindings {
        binding: vec![binding],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("existing without a matcher should be rejected");
    assert!(
        error.contains("editor") && error.contains("binding"),
        "error should be worded in terms of the binding and name its slot, not the resulting \
             layout step: {error:?}"
    );
}

#[test]
fn resolve_fills_in_a_binding_s_mark_match() {
    let template = minimal_template(vec![minimal_step()]);
    let mut binding = minimal_binding();
    binding.command = None;
    binding.existing = true;
    binding.mark_match = Some("dropdown-term".to_string());
    let bindings = Bindings {
        binding: vec![binding],
        ..Default::default()
    };
    let resolved = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .expect("valid binding should resolve");
    assert_eq!(resolved[0].mark_match, Some("dropdown-term".to_string()));
}

#[test]
fn resolve_errors_on_unused_binding() {
    let template = minimal_template(vec![]);
    let bindings = Bindings {
        binding: vec![minimal_binding()],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("unused binding should error");
    assert!(error.contains("editor"));
}

#[test]
fn resolve_errors_on_duplicate_slot_within_bindings() {
    let template = minimal_template(vec![minimal_step()]);
    let bindings = Bindings {
        binding: vec![minimal_binding(), minimal_binding()],
        ..Default::default()
    };
    let error = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .map_err(|error| error.to_string())
        .err()
        .expect("duplicate binding slot should error");
    assert!(error.contains("editor"));
}

#[test]
fn resolve_target_id_step_has_no_id_and_no_target_fields() {
    let mut step = minimal_step();
    step.slot = None;
    step.target_id = Some("editor".to_string());
    let template = minimal_template(vec![step]);
    let bindings = Bindings {
        binding: vec![],
        ..Default::default()
    };

    let resolved = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
        .expect("valid template should resolve");
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].id, None);
    assert_eq!(resolved[0].target_id, Some("editor".to_string()));
    assert_eq!(resolved[0].command, None);
    assert_eq!(resolved[0].con_id, None);
    assert!(!resolved[0].existing);
}

#[test]
fn resolve_carries_every_binding_target_field_into_its_own_layout_step_field() {
    let step_for = |slot: &str| TemplateStep {
        slot: Some(slot.to_string()),
        ..minimal_step()
    };
    let template = minimal_template(vec![
        step_for("launched"),
        step_for("by-class"),
        step_for("by-mark"),
        step_for("by-id"),
    ]);
    let launched = Binding {
        slot: "launched".to_string(),
        command: Some("launch-command".to_string()),
        cwd: Some("launch-directory".to_string()),
        env: [("LAUNCH_NAME".to_string(), "launch-value".to_string())].into(),
        app_id: Some("launch-app-id".to_string()),
        title: Some(crate::sway_launch::parse_title_pattern("launch-title").unwrap()),
        ..minimal_binding()
    };
    let by_class = Binding {
        slot: "by-class".to_string(),
        command: None,
        existing: true,
        wait: true,
        class: Some("existing-class".to_string()),
        ..minimal_binding()
    };
    let by_mark = Binding {
        slot: "by-mark".to_string(),
        command: None,
        existing: true,
        mark_match: Some("existing-mark".to_string()),
        ..minimal_binding()
    };
    let by_id = Binding {
        slot: "by-id".to_string(),
        command: None,
        con_id: Some(4242),
        ..minimal_binding()
    };
    let bindings = Bindings {
        binding: vec![launched, by_class, by_mark, by_id],
        ..Default::default()
    };

    let resolved = resolve(&template, &bindings, &[], DirectoryCheck::FormatOnly)
        .expect("every binding is valid");
    let [launched, by_class, by_mark, by_id] = &resolved[..] else {
        panic!("expected four steps, got {}", resolved.len());
    };

    assert_eq!(launched.id.as_deref(), Some("launched"));
    assert_eq!(launched.target_id, None);
    assert_eq!(launched.command.as_deref(), Some("launch-command"));
    assert_eq!(launched.cwd.as_deref(), Some("launch-directory"));
    assert_eq!(
        launched.env.get("LAUNCH_NAME").map(String::as_str),
        Some("launch-value")
    );
    assert_eq!(launched.app_id.as_deref(), Some("launch-app-id"));
    assert_eq!(launched.class, None);
    assert_eq!(launched.mark_match, None);
    assert_eq!(
        launched.title.as_ref().map(|title| title.source()),
        Some("launch-title")
    );
    assert_eq!(launched.con_id, None);
    assert!(!launched.existing && !launched.wait);

    assert_eq!(by_class.id.as_deref(), Some("by-class"));
    assert_eq!(by_class.class.as_deref(), Some("existing-class"));
    assert_eq!(by_class.app_id, None);
    assert_eq!(by_class.command, None);
    assert_eq!(by_class.cwd, None);
    assert!(by_class.env.is_empty());
    assert!(by_class.title.is_none());
    assert!(by_class.existing && by_class.wait);

    assert_eq!(by_mark.mark_match.as_deref(), Some("existing-mark"));
    assert_eq!(by_mark.class, None);
    assert!(by_mark.existing && !by_mark.wait);

    assert_eq!(by_id.id.as_deref(), Some("by-id"));
    assert_eq!(by_id.con_id, Some(4242));
    assert!(!by_id.existing);
    assert_eq!(by_id.command, None);
}

#[test]
fn parse_reads_a_representative_template() {
    let template = parse(
        r#"
            [template]
            description = "A test template."
            category = "Test"

            [[step]]
            slot = "editor"
            floating = true
            "#,
    )
    .expect("valid template should parse");
    assert_eq!(template.step.len(), 1);
    assert_eq!(template.step[0].slot, Some("editor".to_string()));
}

#[test]
fn parse_reads_a_template_with_a_layout_context_table() {
    let template = parse(
        r#"
            [template]
            description = "A test template."
            category = "Test"

            [layout]
            workspace = "3"
            output = "HDMI-A-1"

            [[step]]
            slot = "editor"
            "#,
    )
    .expect("valid template should parse");
    assert_eq!(template.layout.workspace, Some("3".to_string()));
    assert_eq!(template.layout.output, Some("HDMI-A-1".to_string()));
}

#[test]
fn parse_defaults_the_layout_context_when_the_table_is_absent() {
    let template = parse(
        r#"
            [template]
            description = "A test template."
            category = "Test"

            [[step]]
            slot = "editor"
            "#,
    )
    .expect("valid template should parse");
    assert_eq!(template.layout.workspace, None);
    assert_eq!(template.layout.output, None);
}

#[test]
fn parse_rejects_misspelled_layout_context_field() {
    let result = parse(
        r#"
            [template]
            description = "A test template."
            category = "Test"

            [layout]
            workpsace = "3"

            [[step]]
            slot = "editor"
            "#,
    );
    assert!(result.is_err());
}

#[test]
fn parse_rejects_an_invalid_layout_context_workspace_naming_the_table() {
    let result = parse(
        r#"
            [template]
            description = "A test template."
            category = "Test"

            [layout]
            workspace = "a\"b"

            [[step]]
            slot = "editor"
            "#,
    );
    let error = result
        .err()
        .expect("quote in [layout] workspace should be rejected");
    assert!(
        error.starts_with("[layout] workspace: "),
        "error should name the [layout] table: {error:?}"
    );
}

#[test]
fn layout_context_workspace_number_is_a_numbered_build_target() {
    let template = parse(
        r#"
            [template]
            description = "A test template."
            category = "Test"

            [layout]
            workspace_number = 3

            [[step]]
            slot = "editor"
            "#,
    )
    .expect("a [layout] workspace_number should parse");
    assert_eq!(
        template.layout.workspace_target(),
        Some(sway_launch::WorkspaceTarget::Number(3))
    );
}

#[test]
fn parse_rejects_a_layout_context_with_both_workspace_forms_or_a_bad_number() {
    for layout in [
        "workspace = \"web\"\nworkspace_number = 3",
        "workspace_number = -2",
    ] {
        let result = parse(&format!(
            "[template]\ndescription = \"A test template.\"\ncategory = \"Test\"\n\n\
             [layout]\n{layout}\n\n[[step]]\nslot = \"editor\"\n"
        ));
        let error = result.err().expect("should be rejected");
        assert!(error.starts_with("[layout]"), "{error:?}");
    }
}

#[test]
fn parse_rejects_a_blank_layout_context_output_even_when_every_step_overrides_it() {
    let result = parse(
        r#"
            [template]
            description = "A test template."
            category = "Test"

            [layout]
            output = "  "

            [[step]]
            slot = "editor"
            output = "HDMI-A-1"
            "#,
    );
    let error = result
        .err()
        .expect("blank [layout] output should be rejected");
    assert!(
        error.starts_with("[layout] output: "),
        "error should name the [layout] table: {error:?}"
    );
}

#[test]
fn parse_rejects_misspelled_step_field() {
    // Regression test: mirrors layout::parse()'s same-named test —
    // deny_unknown_fields must catch typos here too.
    let result = parse(
        r#"
            [template]
            description = "A test template."
            category = "Test"

            [[step]]
            slot = "editor"
            flaoting = true
            "#,
    );
    assert!(result.is_err());
}

#[test]
fn parse_rejects_a_template_missing_the_template_table() {
    let result = parse(
        r#"
            [[step]]
            slot = "editor"
            "#,
    );
    assert!(result.is_err());
}

#[test]
fn parse_rejects_a_template_table_missing_category() {
    let result = parse(
        r#"
            [template]
            description = "A test template."

            [[step]]
            slot = "editor"
            "#,
    );
    assert!(result.is_err());
}

#[test]
fn parse_rejects_blank_description() {
    let result = parse(
        r#"
            [template]
            description = "   "
            category = "Test"

            [[step]]
            slot = "editor"
            "#,
    );
    let error = result.err().expect("blank description should be rejected");
    assert!(
        error.contains("description"),
        "error should name the field: {error:?}"
    );
}

#[test]
fn parse_rejects_blank_category() {
    let result = parse(
        r#"
            [template]
            description = "A test template."
            category = ""

            [[step]]
            slot = "editor"
            "#,
    );
    let error = result.err().expect("blank category should be rejected");
    assert!(
        error.contains("category"),
        "error should name the field: {error:?}"
    );
}

#[test]
fn parse_bindings_reads_a_representative_bindings_file() {
    let bindings = parse_bindings(
        r#"
            [[binding]]
            slot = "editor"
            command = "foot"
            "#,
    )
    .expect("valid bindings should parse");
    assert_eq!(bindings.binding.len(), 1);
    assert_eq!(bindings.binding[0].slot, "editor");
}

#[test]
fn parse_bindings_reads_a_top_level_concurrent_key_off_by_default() {
    let binding = "[[binding]]\nslot = \"editor\"\ncommand = \"foot\"\n";
    assert!(
        !parse_bindings(binding)
            .expect("bindings should parse")
            .concurrent
    );
    let concurrent =
        parse_bindings(&format!("concurrent = true\n{binding}")).expect("bindings should parse");
    assert!(concurrent.concurrent);
}

#[test]
fn parse_rejects_a_concurrent_key_in_a_template() {
    // Deliberate (#121): a template names no applications, so whether they
    // can launch at once is the bindings file's to say, not the template's.
    let result = parse(
        r#"
            concurrent = true

            [template]
            description = "A test template."
            category = "Test"

            [[step]]
            slot = "editor"
            "#,
    );
    assert!(result.is_err());
}

#[test]
fn parse_bindings_rejects_misspelled_field() {
    let result = parse_bindings(
        r#"
            [[binding]]
            slot = "editor"
            comand = "foot"
            "#,
    );
    assert!(result.is_err());
}

#[test]
fn builtin_finds_a_known_template_by_name() {
    let contents = builtin("quad-grid").expect("quad-grid should be a built-in template");
    assert!(contents.contains("slot = \"top-left\""));
}

#[test]
fn builtin_returns_none_for_an_unknown_name() {
    assert!(builtin("not-a-real-template").is_none());
}

#[test]
fn builtin_returns_none_for_a_name_containing_a_path_separator() {
    // get_file() searches recursively, but builtin_templates() lists only
    // immediate children -- so a nested template would otherwise be
    // runnable yet unlisted and untested. Rejecting separators keeps the
    // two in agreement. (Nothing is nested today; this pins the rule
    // before something is.)
    assert!(builtin("sub/quad-grid").is_none());
    assert!(builtin("../Cargo").is_none());
    assert!(builtin("sub\\quad-grid").is_none());
}

#[test]
fn builtin_returns_none_for_a_name_with_a_toml_extension() {
    // The lookup is keyed on the bare name only — main.rs's dispatch is
    // what decides a `.toml`-suffixed value should never reach here at
    // all, but builtin() itself shouldn't silently strip the extension
    // either.
    assert!(builtin("quad-grid.toml").is_none());
}

#[test]
fn builtin_templates_lists_every_shipped_template_sorted_by_name() {
    let templates = builtin_templates();
    assert!(
        templates.len() >= 18,
        "expected at least 18 built-in templates, found {}",
        templates.len()
    );
    let mut sorted_names: Vec<_> = templates.iter().map(|t| t.name).collect();
    let mut expected = sorted_names.clone();
    expected.sort_unstable();
    assert_eq!(
        sorted_names, expected,
        "builtin_templates() should be sorted by name"
    );
    sorted_names.sort_unstable();
    assert!(sorted_names.contains(&"quad-grid"));
}

#[test]
fn builtin_templates_every_description_is_a_complete_sentence() {
    // --list-templates prints each `[template]` table's description as
    // one line next to the template's name, so it has to stand on its
    // own as a complete sentence rather than a fragment.
    for template in builtin_templates() {
        assert!(
            template.description.trim_end().ends_with('.'),
            "{:?}'s description should be a complete sentence, got {:?}",
            template.name,
            template.description
        );
    }
}

#[test]
fn builtin_templates_every_category_is_non_empty() {
    for template in builtin_templates() {
        assert!(
            !template.category.trim().is_empty(),
            "{:?}'s category should be non-empty",
            template.name
        );
    }
}

#[test]
fn builtin_templates_reports_slots_and_slot_names_matching_the_apps_ordering() {
    // quad-grid.toml's slots, in file order -- pins the exact contract
    // --list-templates --json's "slots"/"slot_names" fields promise:
    // slots is the count, slot_names is that same order --apps zips
    // against (bindings_from_apps() uses the identical
    // slot_names() helper, so this also indirectly confirms
    // the two never disagree).
    let templates = builtin_templates();
    let quad_grid = templates
        .iter()
        .find(|template| template.name == "quad-grid")
        .expect("quad-grid should be a built-in template");
    assert_eq!(quad_grid.slots, 4);
    assert_eq!(
        quad_grid.slot_names,
        vec!["top-left", "top-right", "bottom-left", "bottom-right"]
    );
}

#[test]
fn every_builtin_template_resolves_and_converts_without_a_compositor() {
    // parse() only checks TOML shape and the [template] table; the
    // height/width/position formats, sway-string fields, target rules
    // and target_id references are checked by resolve(, &[], DirectoryCheck::Exists) and
    // to_sway_launch(). Running both here, the way --validate does,
    // means a broken built-in fails cargo test (and the release gate)
    // instead of only the separate live-Sway job.
    let mut checked = 0;
    for file in BUILTIN_TEMPLATES.files() {
        let name = file.path().display();
        let template = parse(file.contents_utf8().expect("template should be UTF-8"))
            .unwrap_or_else(|error| panic!("{name} failed to parse: {error}"));
        let bindings = Bindings {
            binding: slot_names(&template)
                .into_iter()
                .map(|slot| Binding {
                    slot: slot.to_string(),
                    ..minimal_binding()
                })
                .collect(),
            ..Default::default()
        };
        let steps = resolve(&template, &bindings, &[], DirectoryCheck::Exists)
            .unwrap_or_else(|error| panic!("{name} failed to resolve: {error}"));

        let mut resolved_ids = HashMap::new();
        for (index, step) in steps.iter().enumerate() {
            step.to_sway_launch(
                std::time::Duration::from_secs(1),
                std::time::Duration::from_millis(20),
                false,
                &resolved_ids,
                sway_launch::DirectoryCheck::FormatOnly,
            )
            .unwrap_or_else(|error| panic!("{name} step {}: {error}", index + 1));
            if let Some(id) = step.id.as_deref() {
                resolved_ids.insert(id.to_string(), index as i64);
            }
        }
        checked += 1;
    }
    assert!(
        checked >= 18,
        "expected at least 18 built-in templates, checked {checked}"
    );
}

#[test]
fn slot_names_lists_slot_steps_in_order_and_ignores_target_id_steps() {
    let mut editor_step = minimal_step();
    editor_step.slot = Some("editor".to_string());
    let mut target_id_step = minimal_step();
    target_id_step.slot = None;
    target_id_step.target_id = Some("editor".to_string());
    let mut terminal_step = minimal_step();
    terminal_step.slot = Some("terminal".to_string());

    let template = minimal_template(vec![editor_step, target_id_step, terminal_step]);

    assert_eq!(slot_names(&template), vec!["editor", "terminal"]);
}

fn repository_file(relative_path: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{relative_path}: {error}"))
}

struct ReadmeTemplateRow {
    preview: String,
    category: String,
    slots: String,
}

/// README.md's "Templates" table, keyed by template name: every table row
/// whose Template cell links into `templates/`.
fn readme_template_rows() -> std::collections::BTreeMap<String, ReadmeTemplateRow> {
    let readme = repository_file("README.md");
    let mut rows = std::collections::BTreeMap::new();
    for line in readme.lines().filter(|line| line.starts_with("| ")) {
        let cells: Vec<&str> = line.trim_matches('|').split(" | ").map(str::trim).collect();
        let [preview, category, template, _shape, slots] = cells.as_slice() else {
            continue;
        };
        let Some(name) = template
            .strip_prefix("[`")
            .and_then(|rest| rest.split_once("`](templates/"))
            .map(|(name, _)| name.to_string())
        else {
            continue;
        };
        let duplicate = rows.insert(
            name.clone(),
            ReadmeTemplateRow {
                preview: preview.to_string(),
                category: category.to_string(),
                slots: slots.to_string(),
            },
        );
        assert!(duplicate.is_none(), "README.md lists {name:?} twice");
    }
    rows
}

/// The screenshot script's own `EXCLUDED_TEMPLATES` set, read back from the
/// script so there's one list rather than a copy here to keep in sync.
fn screenshot_excluded_template_names() -> Vec<String> {
    let script = repository_file("scripts/generate-layout-screenshots");
    let line = script
        .lines()
        .find(|line| line.starts_with("EXCLUDED_TEMPLATES = {"))
        .expect("scripts/generate-layout-screenshots should define EXCLUDED_TEMPLATES");
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(|file_name| {
            file_name
                .strip_suffix(".toml")
                .unwrap_or_else(|| panic!("EXCLUDED_TEMPLATES entry {file_name:?} lacks .toml"))
                .to_string()
        })
        .collect()
}

#[test]
fn readme_templates_table_matches_every_builtin_template() {
    // README.md's "Templates" table repeats each template's category and
    // slot count by hand; the template file is the source of truth.
    let mut rows = readme_template_rows();
    for template in builtin_templates() {
        let row = rows.remove(template.name).unwrap_or_else(|| {
            panic!(
                "README.md's \"Templates\" table has no row for {:?} \
                 (category {:?}, {} slot(s))",
                template.name, template.category, template.slots
            )
        });
        assert_eq!(
            row.category, template.category,
            "README.md's category for {:?} disagrees with its [template] table",
            template.name
        );
        assert_eq!(
            row.slots,
            template.slots.to_string(),
            "README.md's slot count for {:?} disagrees with its steps",
            template.name
        );
    }
    assert!(
        rows.is_empty(),
        "README.md's \"Templates\" table lists templates that don't exist: {:?}",
        rows.keys().collect::<Vec<_>>()
    );
}

#[test]
fn every_builtin_template_has_a_screenshot_unless_the_screenshot_script_excludes_it() {
    let excluded = screenshot_excluded_template_names();
    let names: Vec<&str> = builtin_templates()
        .iter()
        .map(|template| template.name)
        .collect();
    for name in &excluded {
        assert!(
            names.contains(&name.as_str()),
            "EXCLUDED_TEMPLATES names {name:?}, which isn't a template"
        );
    }

    let rows = readme_template_rows();
    let screenshots_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("screenshots");
    for name in &names {
        let preview = &rows
            .get(*name)
            .unwrap_or_else(|| panic!("README.md has no row for {name:?}"))
            .preview;
        let screenshot = format!("screenshots/{name}.png");
        if excluded.iter().any(|excluded_name| excluded_name == name) {
            assert_eq!(
                preview, "—",
                "{name:?} is in EXCLUDED_TEMPLATES, so README.md's preview cell should be \"—\""
            );
        } else {
            assert!(
                screenshots_dir.join(format!("{name}.png")).is_file(),
                "{screenshot} is missing; run scripts/generate-layout-screenshots --only {name}"
            );
            assert!(
                preview.contains(&format!("src=\"{screenshot}\"")),
                "README.md's preview cell for {name:?} should embed {screenshot}"
            );
        }
    }

    for entry in std::fs::read_dir(&screenshots_dir).expect("screenshots/ should be readable") {
        let path = entry.expect("screenshots/ entry should be readable").path();
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        assert!(
            names.contains(&stem) && !excluded.iter().any(|name| name == stem),
            "{} doesn't belong to any screenshotted template",
            path.display()
        );
    }
}

#[test]
fn builtin_templates_header_comment_repeats_its_description() {
    // Each file's header comment restates its [template] description for a
    // reader of the raw file; this keeps the two from drifting apart.
    for template in builtin_templates() {
        let contents = builtin(template.name).expect("listed built-in should resolve");
        let header: Vec<&str> = contents
            .lines()
            .map_while(|line| line.strip_prefix('#'))
            .flat_map(str::split_whitespace)
            .collect();
        let description: Vec<&str> = template.description.split_whitespace().collect();
        assert!(
            header
                .windows(description.len())
                .any(|words| words == description),
            "{:?}'s header comment should repeat its description {:?}",
            template.name,
            template.description
        );
    }
}

#[test]
fn resolve_lets_a_binding_replace_the_step_placement() {
    let mut step = minimal_step();
    step.workspace = Some("1".to_string());
    step.output = Some("HDMI-A-1".to_string());
    let template = minimal_template(vec![step]);

    let resolved = resolve(
        &template,
        &Bindings {
            binding: vec![minimal_binding()],
            ..Default::default()
        },
        &[],
        DirectoryCheck::Exists,
    )
    .expect("a binding without placement should resolve");
    assert_eq!(resolved[0].workspace.as_deref(), Some("1"));
    assert_eq!(resolved[0].output.as_deref(), Some("HDMI-A-1"));

    let resolved = resolve(
        &template,
        &Bindings {
            binding: vec![Binding {
                workspace_number: Some(4),
                output: Some("DP-2".to_string()),
                ..minimal_binding()
            }],
            ..Default::default()
        },
        &[],
        DirectoryCheck::Exists,
    )
    .expect("a binding with placement should resolve");
    // The workspace pair is replaced as a whole, so the step's name doesn't
    // survive next to the binding's number.
    assert_eq!(resolved[0].workspace, None);
    assert_eq!(resolved[0].workspace_number, Some(4));
    assert_eq!(resolved[0].output.as_deref(), Some("DP-2"));
}

#[test]
fn resolve_reports_a_bad_binding_placement_against_the_bindings() {
    let mut new_column_step = minimal_step();
    new_column_step.new_column = true;
    let cases = [
        (
            minimal_step(),
            Binding {
                workspace: Some("3".to_string()),
                workspace_number: Some(3),
                ..minimal_binding()
            },
            "can't both be set",
        ),
        (
            minimal_step(),
            Binding {
                output: Some("current".to_string()),
                ..minimal_binding()
            },
            "output:",
        ),
        (
            minimal_step(),
            Binding {
                workspace_number: Some(-1),
                ..minimal_binding()
            },
            "workspace_number:",
        ),
        (
            new_column_step,
            Binding {
                workspace: Some("3".to_string()),
                ..minimal_binding()
            },
            "new_column/new_row",
        ),
    ];
    for (step, binding, expected) in cases {
        let template = minimal_template(vec![step]);
        let error = resolve(
            &template,
            &Bindings {
                binding: vec![binding],
                ..Default::default()
            },
            &[],
            DirectoryCheck::Exists,
        )
        .err()
        .expect("a bad placement should be rejected");
        match error {
            ResolveError::Bindings(message) => {
                assert!(message.contains("slot \"editor\""), "{message}");
                assert!(message.contains(expected), "{message}");
            }
            other => panic!("expected a bindings error, got {other:?}"),
        }
    }
}

fn params_template(params: &str, width: &str) -> String {
    format!(
        "[template]\ndescription = \"A test template.\"\ncategory = \"Test\"\n\n\
         [params]\n{params}\n\n[[step]]\nslot = \"main\"\n\n\
         [[step]]\nslot = \"stack\"\nwidth = \"{width}\"\n"
    )
}

#[test]
fn parse_checks_every_param_and_reference() {
    assert!(parse(&params_template("stack-width = \"35ppt\"", "{stack-width}")).is_ok());
    let cases = [
        (
            "stack-width = \"35ppt\"",
            "{other}",
            "{other} names no param",
        ),
        (
            "\"stack width\" = \"35ppt\"",
            "{stack width}",
            "isn't a valid param name",
        ),
        (
            "stack-width = \"wide\"",
            "{stack-width}",
            "param \"stack-width\" (used as width)",
        ),
        (
            "unused = \"35ppt\"",
            "35ppt",
            "\"unused\" isn't used by any step",
        ),
    ];
    for (params, width, expected) in cases {
        let error = parse(&params_template(params, width))
            .err()
            .unwrap_or_else(|| panic!("{params} / {width} should be rejected"));
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn resolve_takes_param_overrides_in_order_of_precedence() {
    let template = parse(&params_template("stack-width = \"35ppt\"", "{stack-width}"))
        .expect("template should parse");
    let bindings = |params: &[(&str, &str)]| Bindings {
        binding: ["main", "stack"]
            .into_iter()
            .map(|slot| Binding {
                slot: slot.to_string(),
                ..minimal_binding()
            })
            .collect(),
        params: params
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
        ..Default::default()
    };
    let width = |bindings: &Bindings, overrides: &[(String, String)]| {
        resolve(&template, bindings, overrides, DirectoryCheck::Exists).expect("should resolve")[1]
            .width
            .clone()
    };
    let override_with = |value: &str| vec![("stack-width".to_string(), value.to_string())];

    assert_eq!(width(&bindings(&[]), &[]).as_deref(), Some("35ppt"));
    let from_file = bindings(&[("stack-width", "40ppt")]);
    assert_eq!(width(&from_file, &[]).as_deref(), Some("40ppt"));
    assert_eq!(
        width(
            &from_file,
            &[override_with("20ppt"), override_with("45ppt")].concat()
        )
        .as_deref(),
        Some("45ppt")
    );
}

#[test]
fn resolve_reports_a_bad_param_override_against_its_source() {
    let template = parse(&params_template("stack-width = \"35ppt\"", "{stack-width}"))
        .expect("template should parse");
    let bindings = Bindings {
        binding: ["main", "stack"]
            .into_iter()
            .map(|slot| Binding {
                slot: slot.to_string(),
                ..minimal_binding()
            })
            .collect(),
        params: [("stack-width".to_string(), "huge".to_string())].into(),
        ..Default::default()
    };
    assert!(matches!(
        resolve(&template, &bindings, &[], DirectoryCheck::Exists),
        Err(ResolveError::Bindings(message)) if message.contains("[params]: param \"stack-width\"")
    ));

    let bindings = Bindings {
        params: Default::default(),
        ..bindings
    };
    let unknown = [("stack".to_string(), "40ppt".to_string())];
    assert!(matches!(
        resolve(&template, &bindings, &unknown, DirectoryCheck::Exists),
        Err(ResolveError::Param(message))
            if message.contains("unknown param \"stack\" (this template's params: stack-width)")
    ));
}

#[test]
fn parse_param_override_splits_at_the_first_equals_sign() {
    assert_eq!(
        parse_param_override("overlay-position=100 200"),
        Ok(("overlay-position".to_string(), "100 200".to_string()))
    );
    assert_eq!(
        parse_param_override("a=b=c"),
        Ok(("a".to_string(), "b=c".to_string()))
    );
    assert!(parse_param_override("stack-width").is_err());
    assert!(parse_param_override("=40ppt").is_err());
}

#[test]
fn every_builtin_param_is_described_in_its_header_comment() {
    for file in BUILTIN_TEMPLATES.files() {
        let contents = file.contents_utf8().expect("template should be UTF-8");
        let template = parse(contents).expect("built-in should parse");
        for (name, default) in &template.params {
            let line = format!("#   {}: ", name);
            assert!(
                contents.lines().any(|text| text.starts_with(&line)
                    && text.ends_with(&format!("(default {default})"))),
                "{} should describe param {name} as `{line}... (default {default})`",
                file.path().display()
            );
        }
    }
}

#[test]
fn resolve_lets_a_binding_set_its_slot_s_timeout_and_wait_time() {
    let mut step = minimal_step();
    step.timeout = Some(5);
    step.wait_time = Some(20);
    let template = minimal_template(vec![step]);
    let resolved = resolve(
        &template,
        &Bindings {
            binding: vec![Binding {
                timeout: Some(30),
                ..minimal_binding()
            }],
            ..Default::default()
        },
        &[],
        DirectoryCheck::Exists,
    )
    .expect("a binding timeout should resolve");
    assert_eq!(resolved[0].timeout, Some(30));
    assert_eq!(resolved[0].wait_time, Some(20));

    let error = resolve(
        &template,
        &Bindings {
            binding: vec![Binding {
                timeout: Some(0),
                ..minimal_binding()
            }],
            ..Default::default()
        },
        &[],
        DirectoryCheck::Exists,
    )
    .err()
    .expect("a zero timeout should be rejected");
    assert!(
        matches!(&error, ResolveError::Bindings(message) if message.contains("slot \"editor\": timeout:")),
        "{error:?}"
    );
}

#[test]
fn resolve_lets_a_binding_set_its_slot_s_settle() {
    let mut step = minimal_step();
    step.width = Some("700px".to_string());
    let template = minimal_template(vec![step]);
    let with_settle = |settle| Bindings {
        binding: vec![Binding {
            settle: Some(settle),
            ..minimal_binding()
        }],
        ..Default::default()
    };
    let resolved = resolve(&template, &with_settle(300), &[], DirectoryCheck::Exists)
        .expect("a binding settle resolves");
    assert_eq!(resolved[0].settle, Some(300));

    let error = resolve(&template, &with_settle(0), &[], DirectoryCheck::Exists)
        .err()
        .expect("a zero settle should be rejected");
    assert!(
        matches!(&error, ResolveError::Bindings(message) if message.contains("slot \"editor\": settle:")),
        "{error:?}"
    );

    let geometry_free = minimal_template(vec![minimal_step()]);
    let error = resolve(
        &geometry_free,
        &with_settle(300),
        &[],
        DirectoryCheck::Exists,
    )
    .err()
    .expect("a settle with nothing to hold should be rejected");
    assert!(
        matches!(&error, ResolveError::Bindings(message) if message.contains("sets no height, width or position")),
        "{error:?}"
    );
}
