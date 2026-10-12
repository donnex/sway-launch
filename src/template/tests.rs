use super::*;

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
        output: None,
        height: None,
        width: None,
        position: None,
        scratchpad: false,
        timeout: None,
        wait_time: None,
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
        step,
    }
}

fn minimal_binding() -> Binding {
    Binding {
        slot: "editor".to_string(),
        command: Some("foot".to_string()),
        con_id: None,
        existing: false,
        app_id: None,
        class: None,
        mark_match: None,
    }
}

#[test]
fn resolve_rejects_step_without_slot_or_target_id() {
    let mut step = minimal_step();
    step.slot = None;
    let template = minimal_template(vec![step]);
    let bindings = Bindings { binding: vec![] };
    assert!(resolve(&template, &bindings).is_err());
}

#[test]
fn resolve_rejects_slot_and_target_id_together() {
    let mut step = minimal_step();
    step.target_id = Some("editor".to_string());
    let template = minimal_template(vec![step]);
    let bindings = Bindings {
        binding: vec![minimal_binding()],
    };
    assert!(resolve(&template, &bindings).is_err());
}

#[test]
fn resolve_rejects_blank_slot() {
    let mut step = minimal_step();
    step.slot = Some("   ".to_string());
    let template = minimal_template(vec![step]);
    let bindings = Bindings { binding: vec![] };
    let error = resolve(&template, &bindings)
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
    let bindings = Bindings { binding: vec![] };
    let error = resolve(&template, &bindings)
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
    };
    let error = resolve(&template, &bindings)
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
    };
    let error = resolve(&template, &bindings)
        .err()
        .expect("blank binding command should be rejected");
    assert!(
        error.contains("editor") && error.contains("command"),
        "error should name the offending slot and field: {:?}",
        error
    );
}

#[test]
fn resolve_rejects_two_steps_sharing_a_slot_name() {
    let template = minimal_template(vec![minimal_step(), minimal_step()]);
    let bindings = Bindings {
        binding: vec![minimal_binding()],
    };
    let error = resolve(&template, &bindings)
        .err()
        .expect("two steps sharing a slot should error");
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
    let template = minimal_template(vec![step]);
    let bindings = Bindings {
        binding: vec![minimal_binding()],
    };

    let resolved = resolve(&template, &bindings).expect("valid template should resolve");
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].id, Some("editor".to_string()));
    assert_eq!(resolved[0].command, Some("foot".to_string()));
    assert!(resolved[0].floating);
    assert_eq!(resolved[0].mark, Some("pinned".to_string()));
}

#[test]
fn resolve_applies_the_template_layout_context_to_a_step_without_its_own() {
    let mut template = minimal_template(vec![minimal_step()]);
    template.layout.workspace = Some("3".to_string());
    template.layout.output = Some("HDMI-A-1".to_string());
    let bindings = Bindings {
        binding: vec![minimal_binding()],
    };

    let resolved = resolve(&template, &bindings).expect("valid template should resolve");
    assert_eq!(resolved[0].workspace, Some("3".to_string()));
    assert_eq!(resolved[0].output, Some("HDMI-A-1".to_string()));
}

#[test]
fn resolve_lets_a_step_s_own_workspace_and_output_win_over_the_template_layout_context() {
    let mut step = minimal_step();
    step.workspace = Some("5".to_string());
    step.output = Some("DP-1".to_string());
    let mut template = minimal_template(vec![step]);
    template.layout.workspace = Some("3".to_string());
    template.layout.output = Some("HDMI-A-1".to_string());
    let bindings = Bindings {
        binding: vec![minimal_binding()],
    };

    let resolved = resolve(&template, &bindings).expect("valid template should resolve");
    assert_eq!(resolved[0].workspace, Some("5".to_string()));
    assert_eq!(resolved[0].output, Some("DP-1".to_string()));
}

#[test]
fn resolve_mixes_a_step_s_own_field_with_the_template_layout_context_s_other_field() {
    let mut step = minimal_step();
    step.workspace = Some("5".to_string());
    let mut template = minimal_template(vec![step]);
    template.layout.workspace = Some("3".to_string());
    template.layout.output = Some("HDMI-A-1".to_string());
    let bindings = Bindings {
        binding: vec![minimal_binding()],
    };

    let resolved = resolve(&template, &bindings).expect("valid template should resolve");
    assert_eq!(resolved[0].workspace, Some("5".to_string()));
    assert_eq!(resolved[0].output, Some("HDMI-A-1".to_string()));
}

#[test]
fn resolve_leaves_workspace_and_output_unset_without_a_layout_context() {
    let template = minimal_template(vec![minimal_step()]);
    let bindings = Bindings {
        binding: vec![minimal_binding()],
    };

    let resolved = resolve(&template, &bindings).expect("valid template should resolve");
    assert_eq!(resolved[0].workspace, None);
    assert_eq!(resolved[0].output, None);
}

#[test]
fn resolve_errors_on_missing_binding() {
    let template = minimal_template(vec![minimal_step()]);
    let bindings = Bindings { binding: vec![] };
    let error = resolve(&template, &bindings)
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
            con_id: None,
            existing: false,
            app_id: None,
            class: None,
            mark_match: None,
        }],
    };
    let error = resolve(&template, &bindings)
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
            con_id: Some(42),
            existing: false,
            app_id: None,
            class: None,
            mark_match: None,
        }],
    };
    let error = resolve(&template, &bindings)
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
            con_id: None,
            existing: false,
            app_id: Some("foot".to_string()),
            class: Some("Foot".to_string()),
            mark_match: None,
        }],
    };
    let error = resolve(&template, &bindings)
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
    };
    let error = resolve(&template, &bindings)
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
    };
    let error = resolve(&template, &bindings)
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
    };
    let error = resolve(&template, &bindings)
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
    };
    let error = resolve(&template, &bindings)
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
    };
    let error = resolve(&template, &bindings)
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
    };
    let resolved = resolve(&template, &bindings).expect("valid binding should resolve");
    assert_eq!(resolved[0].mark_match, Some("dropdown-term".to_string()));
}

#[test]
fn resolve_errors_on_unused_binding() {
    let template = minimal_template(vec![]);
    let bindings = Bindings {
        binding: vec![minimal_binding()],
    };
    let error = resolve(&template, &bindings)
        .err()
        .expect("unused binding should error");
    assert!(error.contains("editor"));
}

#[test]
fn resolve_errors_on_duplicate_slot_within_bindings() {
    let template = minimal_template(vec![minimal_step()]);
    let bindings = Bindings {
        binding: vec![minimal_binding(), minimal_binding()],
    };
    let error = resolve(&template, &bindings)
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
    let bindings = Bindings { binding: vec![] };

    let resolved = resolve(&template, &bindings).expect("valid template should resolve");
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].id, None);
    assert_eq!(resolved[0].target_id, Some("editor".to_string()));
    assert_eq!(resolved[0].command, None);
    assert_eq!(resolved[0].con_id, None);
    assert!(!resolved[0].existing);
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
    // distinct_slot_names() helper, so this also indirectly confirms
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
    // and target_id references are checked by resolve() and
    // to_sway_launch(). Running both here, the way --validate does,
    // means a broken built-in fails cargo test (and the release gate)
    // instead of only the separate live-Sway job.
    let mut checked = 0;
    for file in BUILTIN_TEMPLATES.files() {
        let name = file.path().display();
        let template = parse(file.contents_utf8().expect("template should be UTF-8"))
            .unwrap_or_else(|error| panic!("{name} failed to parse: {error}"));
        let bindings = Bindings {
            binding: distinct_slot_names(&template)
                .into_iter()
                .map(|slot| Binding {
                    slot: slot.to_string(),
                    ..minimal_binding()
                })
                .collect(),
        };
        let steps = resolve(&template, &bindings)
            .unwrap_or_else(|error| panic!("{name} failed to resolve: {error}"));

        let mut resolved_ids = HashMap::new();
        for (index, step) in steps.iter().enumerate() {
            step.to_sway_launch(
                std::time::Duration::from_secs(1),
                std::time::Duration::from_millis(20),
                false,
                &resolved_ids,
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
fn distinct_slot_names_deduplicates_and_ignores_target_id_steps() {
    let mut editor_step = minimal_step();
    editor_step.slot = Some("editor".to_string());
    let mut repeated_step = minimal_step();
    repeated_step.slot = Some("editor".to_string());
    let mut target_id_step = minimal_step();
    target_id_step.slot = None;
    target_id_step.target_id = Some("editor".to_string());

    let template = minimal_template(vec![editor_step, repeated_step, target_id_step]);

    assert_eq!(distinct_slot_names(&template), vec!["editor"]);
}
