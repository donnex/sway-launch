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
/// Both plain and `--json` output carry the slot info (see `main.rs`'s
/// `print_builtin_templates()`).
pub struct BuiltinTemplate {
    pub name: &'static str,
    pub description: String,
    pub category: String,
    pub slots: usize,
    pub slot_names: Vec<String>,
}

/// Looks up a built-in template's raw TOML contents by name (no `.toml`
/// extension, e.g. `"quad-grid"`), or `None` if there's no such built-in.
///
/// A name containing a path separator is rejected outright rather than
/// looked up: `include_dir`'s `get_file()` searches nested directories,
/// while `builtin_templates()` below (and `tests/live_sway.rs`'s
/// every_shipped_template_resolves_and_launches_successfully) only ever look
/// at `templates/`'s immediate children. Without this, a template added in a
/// subdirectory would be runnable as `--template sub/name` while staying
/// invisible to `--list-templates` and untested by the live suite. Keeping
/// the lookup flat matches the directory's actual layout and keeps the two
/// halves in agreement by construction.
pub fn builtin(name: &str) -> Option<&'static str> {
    if name.contains('/') || name.contains('\\') {
        return None;
    }
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

// Serialize is test-only — see LayoutStep's matching note in layout.rs.
#[derive(Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
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
/// error naming the slot, rather than left for `run_steps()`'s generic
/// "id already used by an earlier step" check to catch, which would be
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
                for (field, value) in [
                    ("app_id", binding.app_id.as_deref()),
                    ("class", binding.class.as_deref()),
                ] {
                    if let Some(value) = value {
                        sway_launch::require_non_blank(field, value)
                            .map_err(|error| format!("binding for slot {:?}: {}", slot, error))?;
                    }
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
                if binding.mark_match.is_some() && !binding.existing {
                    return Err(format!(
                        "binding for slot {:?}: mark_match requires existing = true",
                        slot
                    ));
                }
                // Caught here rather than left to to_sway_launch(): the
                // resulting LayoutStep would fail there too, but as
                // "step N: existing = true requires app_id, class, or
                // mark_match" — naming a step the user never wrote, for a
                // mistake they made in a bindings file. Same reasoning as
                // the checks above it.
                if binding.existing
                    && binding.app_id.is_none()
                    && binding.class.is_none()
                    && binding.mark_match.is_none()
                {
                    return Err(format!(
                        "binding for slot {:?}: existing = true requires app_id, class, or \
                         mark_match",
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
mod tests;
