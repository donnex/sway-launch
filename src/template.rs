use crate::layout::LayoutStep;
use crate::sway_launch::{self, Split};
use include_dir::{include_dir, Dir};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

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
    /// Each `[params]` name with its default (#92).
    pub params: BTreeMap<String, String>,
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
                params: template.params,
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
    /// Named values a step's `height`/`width`/`position` can be set to by
    /// writing `"{name}"` (#92), each with the default given here, which a
    /// bindings file's `[params]` and `--param` override. So one template
    /// covers every ratio of its shape instead of a file per ratio.
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    #[serde(default)]
    pub step: Vec<TemplateStep>,
}

impl Template {
    /// Every step field written as a param reference, with the param it
    /// names, in step order.
    fn param_uses(&self) -> impl Iterator<Item = (usize, &'static str, &str)> {
        self.step.iter().enumerate().flat_map(|(index, step)| {
            [
                ("height", step.height.as_deref()),
                ("width", step.width.as_deref()),
                ("position", step.position.as_deref()),
            ]
            .into_iter()
            .filter_map(move |(field, value)| Some((index, field, param_reference(value?)?)))
        })
    }

    /// Checks `value` for param `name` against every field that uses it, so
    /// an override that one of them can't take fails before anything runs,
    /// reported against whoever supplied it rather than the step.
    pub fn check_param(&self, name: &str, value: &str) -> Result<(), String> {
        if !self.params.contains_key(name) {
            let known: Vec<&str> = self.params.keys().map(String::as_str).collect();
            return Err(format!(
                "unknown param {:?} (this template's params: {})",
                name,
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            ));
        }
        for (_, field, used) in self.param_uses() {
            if used == name {
                check_field_value(field, value)
                    .map_err(|error| format!("param {:?} (used as {}): {}", name, field, error))?;
            }
        }
        Ok(())
    }
}

/// The param a field's value refers to: its whole value written as
/// `{name}`. Anything else is a literal, left for the field's own check.
fn param_reference(value: &str) -> Option<&str> {
    value.strip_prefix('{')?.strip_suffix('}')
}

fn check_field_value(field: &str, value: &str) -> Result<(), String> {
    if field == "position" {
        sway_launch::parse_position(value).map(|_| ())
    } else {
        sway_launch::parse_size(value).map(|_| ())
    }
}

/// `--param NAME=VALUE`, split at the first `=`. Whether the template has
/// that param, and whether the value suits it, is `resolve()`'s to check,
/// since only it has the template.
pub fn parse_param_override(argument: &str) -> Result<(String, String), String> {
    let (name, value) = argument
        .split_once('=')
        .ok_or_else(|| "expected NAME=VALUE, e.g. stack-width=40ppt".to_string())?;
    if !is_valid_param_name(name) {
        return Err(format!(
            "{:?} isn't a valid param name — use letters, digits, - and _",
            name
        ));
    }
    Ok((name.to_string(), value.to_string()))
}

fn is_valid_param_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

/// A template file's optional `[layout]` table: the workspace and/or output
/// the run builds on. `run.rs` focuses it once before the first step (see
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
    /// `workspace_number` as `parse()` validated it, so `workspace_target()`
    /// never has to check it again.
    #[serde(skip)]
    validated_workspace_number: Option<i32>,
}

impl TemplateLayoutContext {
    /// The workspace this table names, by name or by number, as `parse()`
    /// checked it: a table that didn't come through `parse()` names a
    /// workspace by name only.
    pub fn workspace_target(&self) -> Option<sway_launch::WorkspaceTarget<'_>> {
        sway_launch::WorkspaceTarget::from_pair(
            self.workspace.as_deref(),
            self.validated_workspace_number,
        )
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
    #[serde(default, deserialize_with = "crate::layout::deserialize_sway_commands")]
    pub sway_commands: Vec<String>,

    pub timeout: Option<u64>,
    pub wait_time: Option<u64>,
    pub settle: Option<u64>,
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
    /// With `existing`, wait for the window to open (#108).
    #[serde(default)]
    pub wait: bool,
    pub app_id: Option<String>,
    pub class: Option<String>,
    pub mark_match: Option<String>,
    pub title: Option<sway_launch::TitlePattern>,
    /// Where this slot's window goes, overriding the template step's own
    /// (#30), so a bindings file can place a built-in's windows on the
    /// user's own workspaces and outputs without copying the template.
    pub workspace: Option<String>,
    pub workspace_number: Option<i64>,
    pub output: Option<String>,
    /// This slot's own `timeout`/`wait_time`, overriding the template
    /// step's and the run's (#19): a slow-starting application's slot can
    /// wait longer without every other slot waiting as long.
    pub timeout: Option<u64>,
    pub wait_time: Option<u64>,
    /// This slot's own `settle` (#17), overriding the template step's: the
    /// binding is what knows the application is a browser.
    pub settle: Option<u64>,
}

impl Binding {
    fn sets_workspace(&self) -> bool {
        self.workspace.is_some() || self.workspace_number.is_some()
    }

    /// Checks the values this binding overrides `step`'s with, so a mistake
    /// is reported against the bindings file. Moving a window to another
    /// workspace after a relative move is refused for the same reason
    /// `to_sway_launch()` refuses it in a single step.
    fn validate_overrides(&self, step: &TemplateStep) -> Result<(), String> {
        if let Some(timeout) = self.timeout {
            sway_launch::validate_timeout_seconds(timeout)
                .map_err(|error| format!("timeout: {}", error))?;
        }
        if let Some(settle) = self.settle {
            sway_launch::validate_settle_milliseconds(settle)
                .map_err(|error| format!("settle: {}", error))?;
            // Whether a size is in px is only known once params are
            // substituted, so that half is left to to_sway_launch().
            if step.height.is_none() && step.width.is_none() && step.position.is_none() {
                return Err(
                    "settle: this slot's step sets no height, width or position to hold"
                        .to_string(),
                );
            }
        }
        sway_launch::workspace_choice(self.workspace.as_deref(), self.workspace_number)?;
        if let Some(output) = self.output.as_deref() {
            sway_launch::validate_output_argument(output)
                .map_err(|error| format!("output: {}", error))?;
        }
        sway_launch::check_relative_move(self.sets_workspace(), step.new_column, step.new_row)?;
        Ok(())
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Bindings {
    #[serde(default)]
    pub binding: Vec<Binding>,
    /// Overrides for the template's `[params]` defaults (#92).
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    /// `concurrent = true` runs the template as `--concurrent` does (#121).
    /// It lives here rather than in the template, which names no
    /// applications and so can't know whether they're safe to launch at once.
    #[serde(default)]
    pub concurrent: bool,
    /// `no_swaynag = true` keeps a failure off swaynag, as `--no-swaynag`
    /// does (#148). Here rather than in the template for the same reason as
    /// `concurrent`: how a run is started is the bindings' business.
    #[serde(default)]
    pub no_swaynag: bool,
}

impl Bindings {
    /// Anchors every binding's relative `cwd` to `file_directory`, the
    /// bindings file's own directory (#176; see `anchor_launch_directory()`).
    /// Not called for `--apps` or a bindings file read from stdin.
    pub fn anchor_cwds(&mut self, file_directory: &Path) -> Result<(), String> {
        for binding in &mut self.binding {
            if let Some(cwd) = &binding.cwd {
                binding.cwd = Some(
                    sway_launch::anchor_launch_directory(cwd, file_directory).map_err(|error| {
                        format!("binding for slot {:?}: {}", binding.slot, error)
                    })?,
                );
            }
        }
        Ok(())
    }
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
    let mut template: Template = toml::from_str(contents).map_err(|error| error.to_string())?;
    sway_launch::require_non_blank("template.description", &template.template.description)?;
    sway_launch::require_non_blank("template.category", &template.template.category)?;
    let validated_workspace_number = match sway_launch::workspace_choice(
        template.layout.workspace.as_deref(),
        template.layout.workspace_number,
    )
    .map_err(|error| format!("[layout] {}", error))?
    {
        Some(sway_launch::WorkspaceTarget::Number(number)) => Some(number),
        _ => None,
    };
    if let Some(output) = template.layout.output.as_deref() {
        sway_launch::validate_output_argument(output)
            .map_err(|error| format!("[layout] output: {}", error))?;
    }
    for name in template.params.keys() {
        if !is_valid_param_name(name) {
            return Err(format!(
                "[params]: {:?} isn't a valid param name — use letters, digits, - and _",
                name
            ));
        }
    }
    for (index, field, name) in template.param_uses() {
        if !template.params.contains_key(name) {
            return Err(format!(
                "step {}: {}: {{{}}} names no param declared in [params]",
                index + 1,
                field,
                name
            ));
        }
    }
    for (name, default) in &template.params {
        if !template.param_uses().any(|(_, _, used)| used == name) {
            return Err(format!("[params]: {:?} isn't used by any step", name));
        }
        template
            .check_param(name, default)
            .map_err(|error| format!("[params]: {}", error))?;
    }
    let mut seen_slots = HashSet::new();
    for slot in slot_names(&template) {
        if !seen_slots.insert(slot) {
            return Err(format!("template: slot {:?} is used more than once", slot));
        }
    }
    template.layout.validated_workspace_number = validated_workspace_number;
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
    /// A `--param` override, which belongs to neither file.
    Param(String),
    Step {
        index: usize,
        message: String,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::Bindings(message) | ResolveError::Param(message) => {
                formatter.write_str(message)
            }
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
/// A binding's `workspace`/`workspace_number`/`output` replaces the step's
/// own (#30): the bindings file is the user's, the template may be a
/// built-in they can't edit. The workspace pair is replaced as a whole, so a
/// binding's number never meets the step's name. The template's `[layout]`
/// table isn't copied into steps; `run.rs` applies it once, before the
/// first step.
///
/// A step field written as `"{name}"` takes the param's value: `overrides`
/// (`--param`, in order, so a later one wins) over the bindings file's
/// `[params]` over the template's default (#92).
///
/// Expects a template from `parse()`, which has already rejected a slot
/// used by more than one step and checked every param reference and
/// default. Errors say which input they belong to (see `ResolveError`).
pub fn resolve(
    template: &Template,
    bindings: &Bindings,
    overrides: &[(String, String)],
    directory_check: sway_launch::DirectoryCheck,
) -> Result<Vec<LayoutStep>, ResolveError> {
    let mut params: HashMap<&str, &str> = template
        .params
        .iter()
        .map(|(name, default)| (name.as_str(), default.as_str()))
        .collect();
    for (name, value) in &bindings.params {
        template
            .check_param(name, value)
            .map_err(|error| ResolveError::Bindings(format!("[params]: {}", error)))?;
        params.insert(name, value);
    }
    for (name, value) in overrides {
        template
            .check_param(name, value)
            .map_err(ResolveError::Param)?;
        params.insert(name, value);
    }
    let substitute = |value: &Option<String>| {
        value.as_deref().map(|value| {
            param_reference(value)
                .and_then(|name| params.get(name))
                .unwrap_or(&value)
                .to_string()
        })
    };

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

        let binding = if let Some(slot) = step.slot.as_deref() {
            let binding = *bindings_by_slot
                .get(slot)
                .ok_or_else(|| ResolveError::Bindings(format!("no binding for slot {:?}", slot)))?;

            // The same rules a layout step and the CLI go through, checked
            // here so a mistake is reported against the bindings file
            // rather than as a step the user never wrote.
            sway_launch::Target::from_fields(sway_launch::TargetFields {
                command: binding.command.as_deref(),
                con_id: binding.con_id,
                existing: binding.existing,
                wait: binding.wait,
                target_id: None,
                app_id: binding.app_id.as_deref(),
                class: binding.class.as_deref(),
                mark_match: binding.mark_match.as_deref(),
                title: binding.title.as_ref(),
            })
            .map_err(|error| {
                ResolveError::Bindings(format!(
                    "binding for slot {:?}: {}",
                    slot,
                    error.message(sway_launch::FieldStyle::Binding)
                ))
            })?;

            binding.validate_overrides(step).map_err(|error| {
                ResolveError::Bindings(format!("binding for slot {:?}: {}", slot, error))
            })?;

            // A template step has no cwd/env, so a mistake in either is
            // always the binding's (#157); `to_sway_launch()` checks them
            // again, which can then no longer fail.
            let env: Vec<(String, String)> = binding
                .env
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            sway_launch::launch_setup(
                binding.command.is_some(),
                binding.cwd.as_deref(),
                &env,
                directory_check,
            )
            .map_err(|error| {
                ResolveError::Bindings(format!("binding for slot {:?}: {}", slot, error))
            })?;

            used_slots.insert(slot);
            Some(binding)
        } else {
            None
        };

        let (workspace, workspace_number) = match binding {
            Some(binding) if binding.sets_workspace() => {
                (binding.workspace.clone(), binding.workspace_number)
            }
            _ => (step.workspace.clone(), step.workspace_number),
        };
        let output = binding
            .and_then(|binding| binding.output.clone())
            .or_else(|| step.output.clone());

        // Every field written out, so a new one doesn't compile until it's
        // wired here; the target fields are then set by name per branch.
        let actions_and_placement = LayoutStep {
            command: None,
            cwd: None,
            env: Default::default(),
            con_id: None,
            existing: false,
            wait: false,
            target_id: None,
            id: None,
            app_id: None,
            class: None,
            mark_match: None,
            title: None,
            split: step.split,
            floating: step.floating,
            sticky: step.sticky,
            fullscreen: step.fullscreen,
            focus: step.focus,
            mark: step.mark.clone(),
            new_column: step.new_column,
            new_row: step.new_row,
            workspace,
            workspace_number,
            output,
            height: substitute(&step.height),
            width: substitute(&step.width),
            position: substitute(&step.position),
            scratchpad: step.scratchpad,
            sway_commands: step.sway_commands.clone(),
            timeout: binding.and_then(|binding| binding.timeout).or(step.timeout),
            wait_time: binding
                .and_then(|binding| binding.wait_time)
                .or(step.wait_time),
            settle: binding.and_then(|binding| binding.settle).or(step.settle),
        };
        steps.push(match binding {
            Some(binding) => LayoutStep {
                id: step.slot.clone(),
                command: binding.command.clone(),
                cwd: binding.cwd.clone(),
                env: binding.env.clone(),
                con_id: binding.con_id,
                existing: binding.existing,
                wait: binding.wait,
                app_id: binding.app_id.clone(),
                class: binding.class.clone(),
                mark_match: binding.mark_match.clone(),
                title: binding.title.clone(),
                ..actions_and_placement
            },
            None => LayoutStep {
                target_id: step.target_id.clone(),
                ..actions_and_placement
            },
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
