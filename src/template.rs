use crate::layout::LayoutStep;
use crate::sway_launch::Split;
use include_dir::{include_dir, Dir};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

/// The contents of `templates/` at compile time — the single source of
/// truth for both the shipped example files and the built-in templates
/// `--template <name>` (no `.toml` extension) resolves against, so there's
/// nothing to keep in sync between the two.
static BUILTIN_TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");

/// A built-in template's name and one-line description (its file's first
/// header comment line), as listed by `--list-templates`.
pub struct BuiltinTemplate {
    pub name: &'static str,
    pub description: &'static str,
}

/// Looks up a built-in template's raw TOML contents by name (no `.toml`
/// extension, e.g. `"quad-grid"`), or `None` if there's no such built-in.
pub fn builtin(name: &str) -> Option<&'static str> {
    BUILTIN_TEMPLATES
        .get_file(format!("{name}.toml"))
        .and_then(|file| file.contents_utf8())
}

/// Every built-in template's name and description, sorted by name, for
/// `--list-templates`.
pub fn builtin_templates() -> Vec<BuiltinTemplate> {
    let mut templates: Vec<BuiltinTemplate> = BUILTIN_TEMPLATES
        .files()
        .filter_map(|file| {
            let name = file.path().file_stem()?.to_str()?;
            let description = file.contents_utf8()?.lines().next()?.strip_prefix("# ")?;
            Some(BuiltinTemplate { name, description })
        })
        .collect();
    templates.sort_unstable_by_key(|template| template.name);
    templates
}

/// A reusable layout shape: a sequence of steps that describe what to do to
/// a window, without saying which application it belongs to. Combined with
/// `Bindings` (see `resolve()`) to produce ordinary `layout::LayoutStep`s,
/// so a template can be shared/bundled independently of the applications it
/// gets applied to.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    #[serde(default)]
    pub step: Vec<TemplateStep>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateStep {
    /// Names this step's window, so a binding can supply its identity, and
    /// so a later step can reference it via `target_id`. Required unless
    /// `target_id` is set instead.
    pub slot: Option<String>,
    pub target_id: Option<String>,

    pub split: Option<Split>,
    #[serde(default)]
    pub floating: bool,
    #[serde(default)]
    pub sticky: bool,
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default)]
    pub focus: bool,
    pub mark: Option<String>,
    #[serde(default)]
    pub new_column: bool,
    #[serde(default)]
    pub new_row: bool,
    pub workspace: Option<String>,
    pub output: Option<String>,
    pub height: Option<String>,
    pub width: Option<String>,
    pub position: Option<String>,
    #[serde(default)]
    pub scratchpad: bool,

    pub timeout: Option<u64>,
    pub wait_time: Option<u64>,
}

/// A named application binding: supplies the target identity a `slot` needs
/// (`command` to launch, `con_id`/`existing` to act on an already-open
/// window), the same target-selection fields `LayoutStep` has, minus
/// `target_id` (which only ever makes sense on a `TemplateStep`, referencing
/// another slot's resolved window).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub slot: String,
    pub command: Option<String>,
    pub con_id: Option<i64>,
    #[serde(default)]
    pub existing: bool,
    pub app_id: Option<String>,
    pub class: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bindings {
    #[serde(default)]
    pub binding: Vec<Binding>,
}

/// Parses a template file's contents. Kept separate from reading the file
/// itself so the file-not-found and parse-error cases stay distinguishable
/// to the caller.
pub fn parse(contents: &str) -> Result<Template, String> {
    toml::from_str(contents).map_err(|error| error.to_string())
}

/// Parses a bindings file's contents. See `parse()`.
pub fn parse_bindings(contents: &str) -> Result<Bindings, String> {
    toml::from_str(contents).map_err(|error| error.to_string())
}

/// Resolves a `Template` against `Bindings` into ordinary `LayoutStep`s —
/// the actual output type is `layout::LayoutStep`, not a template-specific
/// type, so nothing downstream (`to_sway_launch()`, `run_steps()`) needs to
/// know a template was ever involved. A `slot` step's `id` is set to its
/// slot name, so a later `target_id` step can reference it via the same
/// mechanism named layout steps already use — including the same "id
/// already used by an earlier step" duplicate check a repeated `slot` name
/// would trip at run time, so `resolve()` doesn't need its own duplicate-slot
/// check for that case.
pub fn resolve(template: &Template, bindings: &Bindings) -> Result<Vec<LayoutStep>, String> {
    let mut bindings_by_slot = HashMap::new();
    for binding in &bindings.binding {
        if bindings_by_slot
            .insert(binding.slot.as_str(), binding)
            .is_some()
        {
            return Err(format!("slot {:?} has more than one binding", binding.slot));
        }
    }

    let mut used_slots = HashSet::new();
    let mut steps = Vec::with_capacity(template.step.len());

    for step in &template.step {
        let target_fields_set = [step.slot.is_some(), step.target_id.is_some()]
            .into_iter()
            .filter(|&set| set)
            .count();
        if target_fields_set != 1 {
            return Err("template step must set exactly one of: slot, target_id".to_string());
        }

        let (id, target_id, command, con_id, existing, app_id, class) =
            if let Some(slot) = step.slot.as_deref() {
                let binding = bindings_by_slot
                    .get(slot)
                    .ok_or_else(|| format!("no binding for slot {:?}", slot))?;

                let target_fields_set = [
                    binding.command.is_some(),
                    binding.con_id.is_some(),
                    binding.existing,
                ]
                .into_iter()
                .filter(|&set| set)
                .count();
                if target_fields_set != 1 {
                    return Err(format!(
                        "binding for slot {:?} must set exactly one of: command, con_id, existing",
                        slot
                    ));
                }

                used_slots.insert(slot);
                (
                    Some(slot.to_string()),
                    None,
                    binding.command.clone(),
                    binding.con_id,
                    binding.existing,
                    binding.app_id.clone(),
                    binding.class.clone(),
                )
            } else {
                (None, step.target_id.clone(), None, None, false, None, None)
            };

        steps.push(LayoutStep {
            command,
            con_id,
            existing,
            target_id,
            id,
            app_id,
            class,
            split: step.split,
            floating: step.floating,
            sticky: step.sticky,
            fullscreen: step.fullscreen,
            focus: step.focus,
            mark: step.mark.clone(),
            new_column: step.new_column,
            new_row: step.new_row,
            workspace: step.workspace.clone(),
            output: step.output.clone(),
            height: step.height.clone(),
            width: step.width.clone(),
            position: step.position.clone(),
            scratchpad: step.scratchpad,
            timeout: step.timeout,
            wait_time: step.wait_time,
        });
    }

    let unused: Vec<&str> = bindings_by_slot
        .keys()
        .filter(|slot| !used_slots.contains(*slot))
        .copied()
        .collect();
    if !unused.is_empty() {
        let mut unused = unused;
        unused.sort_unstable();
        return Err(format!(
            "binding(s) for unused slot(s): {}",
            unused.join(", ")
        ));
    }

    Ok(steps)
}

#[cfg(test)]
mod tests {
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

    fn minimal_binding() -> Binding {
        Binding {
            slot: "editor".to_string(),
            command: Some("foot".to_string()),
            con_id: None,
            existing: false,
            app_id: None,
            class: None,
        }
    }

    #[test]
    fn resolve_rejects_step_without_slot_or_target_id() {
        let mut step = minimal_step();
        step.slot = None;
        let template = Template { step: vec![step] };
        let bindings = Bindings { binding: vec![] };
        assert!(resolve(&template, &bindings).is_err());
    }

    #[test]
    fn resolve_rejects_slot_and_target_id_together() {
        let mut step = minimal_step();
        step.target_id = Some("editor".to_string());
        let template = Template { step: vec![step] };
        let bindings = Bindings {
            binding: vec![minimal_binding()],
        };
        assert!(resolve(&template, &bindings).is_err());
    }

    #[test]
    fn resolve_fills_in_the_binding_and_sets_id_to_the_slot_name() {
        let mut step = minimal_step();
        step.floating = true;
        step.mark = Some("pinned".to_string());
        let template = Template { step: vec![step] };
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
    fn resolve_errors_on_missing_binding() {
        let template = Template {
            step: vec![minimal_step()],
        };
        let bindings = Bindings { binding: vec![] };
        let error = resolve(&template, &bindings)
            .err()
            .expect("missing binding should error");
        assert!(error.contains("editor"));
    }

    #[test]
    fn resolve_errors_on_binding_with_no_target_field_naming_the_slot() {
        let template = Template {
            step: vec![minimal_step()],
        };
        let bindings = Bindings {
            binding: vec![Binding {
                slot: "editor".to_string(),
                command: None,
                con_id: None,
                existing: false,
                app_id: None,
                class: None,
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
        let template = Template {
            step: vec![minimal_step()],
        };
        let bindings = Bindings {
            binding: vec![Binding {
                slot: "editor".to_string(),
                command: Some("foot".to_string()),
                con_id: Some(42),
                existing: false,
                app_id: None,
                class: None,
            }],
        };
        let error = resolve(&template, &bindings)
            .err()
            .expect("binding with command and con_id together should error");
        assert!(error.contains("editor"));
    }

    #[test]
    fn resolve_errors_on_unused_binding() {
        let template = Template { step: vec![] };
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
        let template = Template {
            step: vec![minimal_step()],
        };
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
        let template = Template { step: vec![step] };
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
    fn parse_rejects_misspelled_step_field() {
        // Regression test: mirrors layout::parse()'s same-named test —
        // deny_unknown_fields must catch typos here too.
        let result = parse(
            r#"
            [[step]]
            slot = "editor"
            flaoting = true
            "#,
        );
        assert!(result.is_err());
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
        // Regression test: --list-templates prints each description as one
        // line, so a header whose first comment line doesn't end its
        // thought (wraps onto a second line before a period) would print a
        // truncated, broken-looking sentence. See the header comments this
        // was fixed for (e.g. quad-grid.toml, sidebar-left-dual-stack.toml).
        for template in builtin_templates() {
            assert!(
                template.description.trim_end().ends_with('.'),
                "{:?}'s description should be a complete sentence, got {:?}",
                template.name,
                template.description
            );
        }
    }
}
