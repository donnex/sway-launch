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

/// The slot name of every `slot` step, in step order — the order `--apps`
/// zips its own comma-separated list against. No de-duplication is needed:
/// `parse()` rejects a template that uses a slot in more than one step, so
/// each name here is already distinct. Shared by `main.rs`'s
/// `bindings_from_apps()` (which needs this exact order to zip `--apps`
/// correctly) and `builtin_templates()` below (which reports it via
/// `--list-templates --json`), so the two never silently disagree on what
/// "slot order" means.
pub fn slot_names(template: &Template) -> Vec<&str> {
    template
        .step
        .iter()
        .filter_map(|step| step.slot.as_deref())
        .collect()
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
            let slot_names: Vec<String> = slot_names(&template)
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

/// A template file's optional `[layout]` table: the workspace and/or output
/// the run builds on. `main.rs` focuses it once before the first step (see
/// `sway_launch::BuildLocation`), so every window opens there and the shape
/// is built in place. It used to be copied into every step as a per-step
/// move instead, which built the shape on the old workspace and scrambled it
/// on arrival.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TemplateLayoutContext {
    pub workspace: Option<String>,
    /// The workspace by number instead of by name (#43).
    pub workspace_number: Option<i64>,
    pub output: Option<String>,
}

impl TemplateLayoutContext {
    /// The workspace this table names, by name or by number. Expects a
    /// table `parse()` has validated, which rejects both at once and a
    /// number out of range.
    pub fn workspace_target(&self) -> Option<sway_launch::WorkspaceTarget<'_>> {
        self.workspace
            .as_deref()
            .map(sway_launch::WorkspaceTarget::Name)
            .or_else(|| {
                self.workspace_number
                    .and_then(|number| sway_launch::validate_workspace_number(number).ok())
                    .map(sway_launch::WorkspaceTarget::Number)
            })
    }
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
    pub workspace_number: Option<i64>,
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
// Serialize is test-only — see LayoutStep's matching note in layout.rs.
#[derive(Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub slot: String,
    #[serde(default, deserialize_with = "crate::layout::deserialize_command")]
    pub command: Option<String>,
    /// The directory `command` starts in, and variables set for it (#113).
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
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
///
/// The `[layout]` table's values are validated here, where they're
/// declared, rather than left to `to_sway_launch()`'s per-step check of the
/// steps that inherit them: that check would blame a step that never set
/// the field, and would never see a value every step overrides, so
/// `--validate` would call a file valid while it held a value the rest of
/// the tool rejects. Same reasoning as `resolve()`'s own binding checks.
///
/// A slot used by more than one step is rejected here too, so everything
/// that counts or lists slots — `--apps`' count check, `--list-templates`,
/// `--validate`, `resolve()` — only ever sees a template whose slots are
/// already distinct. A later step acts on the same window via `target_id`.
pub fn parse(contents: &str) -> Result<Template, String> {
    let template: Template = toml::from_str(contents).map_err(|error| error.to_string())?;
    sway_launch::require_non_blank("template.description", &template.template.description)?;
    sway_launch::require_non_blank("template.category", &template.template.category)?;
    if let Some(workspace) = template.layout.workspace.as_deref() {
        sway_launch::validate_workspace_argument(workspace)
            .map_err(|error| format!("[layout] workspace: {}", error))?;
    }
    if let Some(number) = template.layout.workspace_number {
        if template.layout.workspace.is_some() {
            return Err(
                "[layout]: workspace and workspace_number can't both be set — pick the workspace \
                 by name or by number"
                    .to_string(),
            );
        }
        sway_launch::validate_workspace_number(number)
            .map_err(|error| format!("[layout] workspace_number: {}", error))?;
    }
    if let Some(output) = template.layout.output.as_deref() {
        sway_launch::validate_output_argument(output)
            .map_err(|error| format!("[layout] output: {}", error))?;
    }
    let mut seen_slots = HashSet::new();
    for slot in slot_names(&template) {
        if !seen_slots.insert(slot) {
            return Err(format!("template: slot {:?} is used more than once", slot));
        }
    }
    Ok(template)
}

/// Parses a bindings file's contents. See `parse()`.
pub fn parse_bindings(contents: &str) -> Result<Bindings, String> {
    toml::from_str(contents).map_err(|error| error.to_string())
}

/// Why `resolve()` failed, split by which input holds the mistake so the
/// caller can name the right file: a binding problem belongs to the
/// `--bindings` file (or `--apps`), a step problem to the template, at the
/// 1-based step number `run_steps()` would report for the same step — each
/// template step resolves to exactly one layout step, so the two agree.
#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    Bindings(String),
    Step { index: usize, message: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::Bindings(message) => formatter.write_str(message),
            ResolveError::Step { index, message } => {
                write!(formatter, "step {}: {}", index + 1, message)
            }
        }
    }
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
/// Expects a template from `parse()`, which has already rejected a slot
/// used by more than one step. Errors say which input they belong to (see
/// `ResolveError`).
pub fn resolve(template: &Template, bindings: &Bindings) -> Result<Vec<LayoutStep>, ResolveError> {
    let mut bindings_by_slot = HashMap::new();
    for binding in &bindings.binding {
        sway_launch::require_non_blank("binding.slot", &binding.slot)
            .map_err(ResolveError::Bindings)?;
        if bindings_by_slot
            .insert(binding.slot.as_str(), binding)
            .is_some()
        {
            return Err(ResolveError::Bindings(format!(
                "slot {:?} has more than one binding",
                binding.slot
            )));
        }
    }

    let mut used_slots = HashSet::new();
    let mut steps = Vec::with_capacity(template.step.len());

    for (index, step) in template.step.iter().enumerate() {
        let step_error = |message: String| ResolveError::Step { index, message };
        let target_fields_set = [step.slot.is_some(), step.target_id.is_some()]
            .into_iter()
            .filter(|&set| set)
            .count();
        if target_fields_set != 1 {
            return Err(step_error(
                "template step must set exactly one of: slot, target_id".to_string(),
            ));
        }
        if let Some(slot) = step.slot.as_deref() {
            sway_launch::require_non_blank("slot", slot).map_err(step_error)?;
        }
        if let Some(target_id) = step.target_id.as_deref() {
            sway_launch::require_non_blank("target_id", target_id).map_err(step_error)?;
        }

        let (id, target_id, command, con_id, existing, app_id, class, mark_match, cwd, env) =
            if let Some(slot) = step.slot.as_deref() {
                let binding = bindings_by_slot.get(slot).ok_or_else(|| {
                    ResolveError::Bindings(format!("no binding for slot {:?}", slot))
                })?;

                // The same rules a layout step and the CLI go through, checked
                // here so a mistake is reported against the bindings file
                // rather than as a step the user never wrote.
                sway_launch::Target::from_fields(sway_launch::TargetFields {
                    command: binding.command.as_deref(),
                    con_id: binding.con_id,
                    existing: binding.existing,
                    target_id: None,
                    app_id: binding.app_id.as_deref(),
                    class: binding.class.as_deref(),
                    mark_match: binding.mark_match.as_deref(),
                })
                .map_err(|error| {
                    ResolveError::Bindings(format!(
                        "binding for slot {:?}: {}",
                        slot,
                        error.message(sway_launch::FieldStyle::Binding)
                    ))
                })?;

                if binding.command.is_none() && (binding.cwd.is_some() || !binding.env.is_empty()) {
                    return Err(ResolveError::Bindings(format!(
                        "binding for slot {:?}: cwd and env only apply to a launched command",
                        slot
                    )));
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
                    binding.mark_match.clone(),
                    binding.cwd.clone(),
                    binding.env.clone(),
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
                    None,
                    Default::default(),
                )
            };

        steps.push(LayoutStep {
            command,
            cwd,
            env,
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
            workspace: step.workspace.clone(),
            workspace_number: step.workspace_number,
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
        return Err(ResolveError::Bindings(format!(
            "binding(s) for unused slot(s): {}",
            unused.join(", ")
        )));
    }

    Ok(steps)
}

#[cfg(test)]
mod tests;
