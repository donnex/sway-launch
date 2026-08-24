use crate::layout::LayoutStep;
use crate::sway_launch::{self, Split};
use include_dir::{include_dir, Dir};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

/// The contents of `templates/` at compile time — the single source of
/// truth for both the shipped example files and the built-in templates
/// `--template <name>` (no `.toml` extension) resolves against, so there's
/// nothing to keep in sync between the two.
static BUILTIN_TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");

/// A built-in template's name, description, category (from its file's
/// `[template]` table), and slot info, as listed by `--list-templates`.
/// `slots`/`slot_names` are `--json`-only (see `main.rs`'s
/// `print_builtin_templates()`) — plain output stays the three-column
/// name/category/description table it always was.
pub struct BuiltinTemplate {
    pub name: &'static str,
    pub description: String,
    pub category: String,
    pub slots: usize,
    pub slot_names: Vec<String>,
}

/// Looks up a built-in template's raw TOML contents by name (no `.toml`
/// extension, e.g. `"quad-grid"`), or `None` if there's no such built-in.
pub fn builtin(name: &str) -> Option<&'static str> {
    BUILTIN_TEMPLATES
        .get_file(format!("{name}.toml"))
        .and_then(|file| file.contents_utf8())
}

/// The distinct slot names a template's steps declare, in first-appearance
/// order — the same order `--apps` zips its own comma-separated list
/// against. Shared by `main.rs`'s `bindings_from_apps()` (which needs this
/// exact order to zip `--apps` correctly) and `builtin_templates()` below
/// (which reports it via `--list-templates --json`), so the two never
/// silently disagree on what "slot order" means.
pub fn distinct_slot_names(template: &Template) -> Vec<&str> {
    let mut slots = Vec::new();
    for step in &template.step {
        if let Some(slot) = step.slot.as_deref() {
            if !slots.contains(&slot) {
                slots.push(slot);
            }
        }
    }
    slots
}

/// Every built-in template's name, description, category, and slot info,
/// sorted by name, for `--list-templates`. Parses each shipped file in full
/// (rather than the old convention of scanning its first header-comment
/// line) to read the required `[template]` table — a shipped file that
/// fails to parse, or is missing that table, is a bug in this repo's own
/// template content, not a runtime condition to handle gracefully, so it
/// panics rather than silently dropping the template from the list.
pub fn builtin_templates() -> Vec<BuiltinTemplate> {
    let mut templates: Vec<BuiltinTemplate> = BUILTIN_TEMPLATES
        .files()
        .filter_map(|file| {
            let name = file.path().file_stem()?.to_str()?;
            let contents = file
                .contents_utf8()
                .unwrap_or_else(|| panic!("built-in template {name:?} is not valid UTF-8"));
            let template = parse(contents).unwrap_or_else(|error| {
                panic!("built-in template {name:?} failed to parse: {error}")
            });
            let slot_names: Vec<String> = distinct_slot_names(&template)
                .into_iter()
                .map(str::to_string)
                .collect();
            Some(BuiltinTemplate {
                name,
                description: template.template.description,
                category: template.template.category,
                slots: slot_names.len(),
                slot_names,
            })
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
    pub template: TemplateMetadata,
    #[serde(default)]
    pub layout: TemplateLayoutContext,
    #[serde(default)]
    pub step: Vec<TemplateStep>,
}

/// A template file's optional `[layout]` table: a `workspace`/`output`
/// applied to every resolved step that doesn't set its own — closes the
/// "works if the workspace/output happens to already be in the right
/// state" gap a template has no other way to express (see README.md's
/// "Recreatable layouts" section), letting a template pin itself to a
/// specific workspace/output instead of always operating on whatever's
/// currently focused when it runs. A step's own `workspace`/`output` field
/// still wins when set — this is a fallback applied per-field in
/// `resolve()`, not a step-level override switch, so a step can use the
/// template's workspace but its own output, or vice versa.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TemplateLayoutContext {
    pub workspace: Option<String>,
    pub output: Option<String>,
}

/// A template file's required `[template]` table: a one-line `description`
/// (shown by `--list-templates`/`--show-template --json`; must be a
/// complete, self-contained sentence ending in `.` — see
/// `builtin_templates_every_description_is_a_complete_sentence` below) and a
/// `category` grouping it alongside similarly-shaped templates in
/// README.md's "Templates" table (e.g. `"Grid"`, `"Master/stack"`,
/// `"Sidebar"`, `"Floating"`, `"Multi-workspace/output"`, `"Retargeting"` —
/// not a closed enum, since a new category can appear alongside new
/// template shapes without a code change). Required on every template file,
/// including one a user writes themselves — this replaced the old
/// "first header-comment line = description" convention entirely, rather
/// than falling back to it when the table is absent, so there's exactly one
/// source of truth and no silent divergence between the two.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateMetadata {
    pub description: String,
    pub category: String,
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
    pub mark_match: Option<String>,
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
    let template: Template = toml::from_str(contents).map_err(|error| error.to_string())?;
    sway_launch::require_non_blank("template.description", &template.template.description)?;
    sway_launch::require_non_blank("template.category", &template.template.category)?;
    Ok(template)
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
/// mechanism named layout steps already use.
///
/// Every resolved step's `workspace`/`output` falls back to the template's
/// own `[layout]` table (`TemplateLayoutContext`) when the step doesn't set
/// its own — applied per-field via `Option::or_else()`, so a step can mix a
/// template-level workspace with its own output, or vice versa.
///
/// A repeated `slot` name is rejected here directly, with a template-shaped
/// error naming the slot — this used to be left for `run_steps()`'s generic
/// "id already used by an earlier step" check to catch instead, which is
/// technically correct but describes an implementation detail (a
/// resolved-layout-step id collision) rather than the actual template
/// authoring mistake a user made. `used_slots` already exists to track which
/// bindings get consumed (see the "unused binding" check below); its own
/// `insert()` return value already distinguishes "first time seeing this
/// slot" from "already used," so no separate check/data structure is
/// needed.
pub fn resolve(template: &Template, bindings: &Bindings) -> Result<Vec<LayoutStep>, String> {
    let mut bindings_by_slot = HashMap::new();
    for binding in &bindings.binding {
        sway_launch::require_non_blank("binding.slot", &binding.slot)?;
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
        if let Some(slot) = step.slot.as_deref() {
            sway_launch::require_non_blank("slot", slot)?;
        }
        if let Some(target_id) = step.target_id.as_deref() {
            sway_launch::require_non_blank("target_id", target_id)?;
        }

        let (id, target_id, command, con_id, existing, app_id, class, mark_match) =
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
                if let Some(command) = binding.command.as_deref() {
                    sway_launch::require_non_blank("command", command)
                        .map_err(|error| format!("binding for slot {:?}: {}", slot, error))?;
                }
                let match_fields_set = [
                    binding.app_id.is_some(),
                    binding.class.is_some(),
                    binding.mark_match.is_some(),
                ]
                .into_iter()
                .filter(|&set| set)
                .count();
                if match_fields_set > 1 {
                    return Err(format!(
                        "binding for slot {:?} must set only one of: app_id, class, mark_match",
                        slot
                    ));
                }

                if !used_slots.insert(slot) {
                    return Err(format!("template: slot {:?} is used more than once", slot));
                }
                (
                    Some(slot.to_string()),
                    None,
                    binding.command.clone(),
                    binding.con_id,
                    binding.existing,
                    binding.app_id.clone(),
                    binding.class.clone(),
                    binding.mark_match.clone(),
                )
            } else {
                (
                    None,
                    step.target_id.clone(),
                    None,
                    None,
                    false,
                    None,
                    None,
                    None,
                )
            };

        steps.push(LayoutStep {
            command,
            con_id,
            existing,
            target_id,
            id,
            app_id,
            class,
            mark_match,
            split: step.split,
            floating: step.floating,
            sticky: step.sticky,
            fullscreen: step.fullscreen,
            focus: step.focus,
            mark: step.mark.clone(),
            new_column: step.new_column,
            new_row: step.new_row,
            workspace: step
                .workspace
                .clone()
                .or_else(|| template.layout.workspace.clone()),
            output: step
                .output
                .clone()
                .or_else(|| template.layout.output.clone()),
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
}
