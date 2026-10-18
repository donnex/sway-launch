use crate::sway_launch::{self, Split};
use serde::Deserialize;
use std::collections::HashMap;
use std::time;

/// A step's or binding's `command`: a string, sent to Sway's `exec` as
/// written, or a list of words (#77), quoted by `render_argv()` so each word
/// reaches the program exactly. Both end up as the same string, so nothing
/// past parsing needs to know which form a file used.
pub fn deserialize_command<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct CommandVisitor;

    impl<'de> serde::de::Visitor<'de> for CommandVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str(
                "a command string, or a list of words such as [\"foot\", \"-T\", \"notes\"]",
            )
        }

        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
            Ok(value.to_string())
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<String, A::Error> {
            let mut words = Vec::new();
            while let Some(word) = seq.next_element::<String>()? {
                words.push(word);
            }
            sway_launch::render_argv(&words).map_err(serde::de::Error::custom)
        }
    }

    deserializer.deserialize_any(CommandVisitor).map(Some)
}

/// A step's `sway_commands` (#90), each checked by `validate_sway_command()`
/// as the file is parsed, so an entry Sway could run as more than one window
/// command is reported against the file, not partway through a run.
pub fn deserialize_sway_commands<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let commands: Vec<String> = serde::Deserialize::deserialize(deserializer)?;
    commands
        .iter()
        .map(|command| {
            sway_launch::validate_sway_command(command)
                .map_err(|error| serde::de::Error::custom(format!("{:?}: {}", command, error)))
        })
        .collect()
}

/// A declarative layout file: a sequence of steps, each the moral
/// equivalent of one `sway-launch` CLI invocation's flags. TOML's
/// array-of-tables syntax (`[[step]]`) maps directly onto `step: Vec<...>`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    #[serde(default)]
    pub step: Vec<LayoutStep>,
    /// `concurrent = true` runs the layout as `--concurrent` does (#121):
    /// the file names the applications, so it knows whether they can be
    /// told apart when launched at once.
    #[serde(default)]
    pub concurrent: bool,
}

// Serialize is test-only, purely so main.rs's schema-parity test can read
// this struct's field names back at runtime — see
// schemas_mirror_args_field_for_field there. Nothing serializes a
// LayoutStep in a real run.
#[derive(Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(deny_unknown_fields)]
pub struct LayoutStep {
    #[serde(default, deserialize_with = "deserialize_command")]
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
    pub target_id: Option<String>,

    /// Names this step, so a later step can target its window via
    /// `target_id`. Layout-only — there's no CLI equivalent, since a single
    /// `sway-launch` invocation only ever has one step to reference.
    pub id: Option<String>,

    pub app_id: Option<String>,
    pub class: Option<String>,
    pub mark_match: Option<String>,
    /// A title pattern, compiled as the file is parsed (#14).
    pub title: Option<sway_launch::TitlePattern>,

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
    /// A workspace by number instead of by name (#43).
    pub workspace_number: Option<i64>,
    pub output: Option<String>,
    pub height: Option<String>,
    pub width: Option<String>,
    pub position: Option<String>,
    #[serde(default)]
    pub scratchpad: bool,
    #[serde(default, deserialize_with = "deserialize_sway_commands")]
    pub sway_commands: Vec<String>,

    pub timeout: Option<u64>,
    pub wait_time: Option<u64>,
    /// `--settle` (#17), in ms.
    pub settle: Option<u64>,
}

/// Parses a layout file's contents. Kept separate from reading the file
/// itself so the file-not-found and parse-error cases stay distinguishable
/// to the caller.
pub fn parse(contents: &str) -> Result<Layout, String> {
    toml::from_str(contents).map_err(|error| error.to_string())
}

impl LayoutStep {
    /// Converts this step into a `SwayLaunch`, enforcing that exactly one of
    /// `command`/`con_id`/`existing`/`target_id` is set (mirroring the
    /// CLI's `conflicts_with` on the first three, plus `target_id` as a
    /// fourth, layout-only target selector) and validating the same
    /// height/width/position formats the CLI flags enforce. Steps without
    /// their own `timeout`/`wait_time` inherit the caller's defaults
    /// (typically the top-level `--timeout`/`--wait-time` CLI values).
    /// `resolved_ids` maps every earlier step's `id` to the container id it
    /// resolved to, for `target_id` lookups.
    pub fn to_sway_launch<'a>(
        &'a self,
        default_timeout: time::Duration,
        default_wait_time: time::Duration,
        verbose: bool,
        resolved_ids: &HashMap<String, i64>,
    ) -> Result<sway_launch::SwayLaunch<'a>, String> {
        if let Some(id) = self.id.as_deref() {
            sway_launch::require_non_blank("id", id)?;
        }
        if let Some(target_id) = self.target_id.as_deref() {
            sway_launch::require_non_blank("target_id", target_id)?;
        }
        if self.workspace.is_some() && self.workspace_number.is_some() {
            return Err(
                "workspace and workspace_number can't both be set — pick the workspace by name \
                 or by number"
                    .to_string(),
            );
        }
        // A relative move only ever acts on the workspace the window opened
        // on; the workspace move after it then drops the window into the
        // target as an ordinary new sibling, so the new row/column never
        // takes effect there.
        if (self.workspace.is_some() || self.workspace_number.is_some())
            && (self.new_column || self.new_row)
        {
            return Err(
                "workspace can't be combined with new_column/new_row — the relative move would \
                 apply on the workspace the window opened on, not the target; build on the target \
                 workspace instead (a template's [layout] table, or --on-workspace)"
                    .to_string(),
            );
        }
        let target_id = self
            .target_id
            .as_deref()
            .map(|target_id| {
                resolved_ids.get(target_id).copied().ok_or_else(|| {
                    format!(
                        "target_id {:?} not found — must reference an earlier step's id",
                        target_id
                    )
                })
            })
            .transpose()?;
        let target = sway_launch::Target::from_fields(sway_launch::TargetFields {
            command: self.command.as_deref(),
            con_id: self.con_id,
            existing: self.existing,
            wait: self.wait,
            target_id,
            app_id: self.app_id.as_deref(),
            class: self.class.as_deref(),
            mark_match: self.mark_match.as_deref(),
            title: self.title.as_ref(),
        })
        .map_err(|error| error.message(sway_launch::FieldStyle::LayoutStep))?;

        // Every value below is interpolated into a Sway command as a quoted
        // string (or, for mark_match, compared against one that was) — see
        // validate_sway_string_argument()'s doc comment for why `"`/`\`/`$`,
        // a control character other than tab and a blank value are rejected rather than silently
        // mangled.
        type Validator = fn(&str) -> Result<String, String>;
        for (field, value, validate) in [
            (
                "mark",
                self.mark.as_deref(),
                sway_launch::validate_sway_string_argument as Validator,
            ),
            (
                "mark_match",
                self.mark_match.as_deref(),
                sway_launch::validate_sway_string_argument,
            ),
            (
                "workspace",
                self.workspace.as_deref(),
                sway_launch::validate_workspace_argument,
            ),
            (
                "output",
                self.output.as_deref(),
                sway_launch::validate_output_argument,
            ),
        ] {
            if let Some(value) = value {
                validate(value).map_err(|error| format!("{}: {}", field, error))?;
            }
        }

        let workspace_number = self
            .workspace_number
            .map(sway_launch::validate_workspace_number)
            .transpose()
            .map_err(|error| format!("workspace_number: {}", error))?;

        if let Some(timeout) = self.timeout {
            sway_launch::validate_timeout_seconds(timeout)
                .map_err(|error| format!("timeout: {}", error))?;
        }
        let height = self
            .height
            .as_deref()
            .map(sway_launch::parse_size)
            .transpose()
            .map_err(|error| format!("height: {}", error))?;
        let width = self
            .width
            .as_deref()
            .map(sway_launch::parse_size)
            .transpose()
            .map_err(|error| format!("width: {}", error))?;
        let position = self
            .position
            .as_deref()
            .map(sway_launch::parse_position)
            .transpose()
            .map_err(|error| format!("position: {}", error))?;
        let settle = sway_launch::settle_duration(self.settle, height, width, position)
            .map_err(|error| format!("settle: {}", error))?;
        let env: Vec<(String, String)> = self
            .env
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let launch_setup =
            sway_launch::launch_setup(self.command.is_some(), self.cwd.as_deref(), &env)?;

        Ok(sway_launch::SwayLaunch {
            target,
            launch_setup,
            split: self.split,
            floating: self.floating,
            sticky: self.sticky,
            fullscreen: self.fullscreen,
            focus: self.focus,
            mark: self.mark.as_deref(),
            new_column: self.new_column,
            new_row: self.new_row,
            workspace: self
                .workspace
                .as_deref()
                .map(sway_launch::WorkspaceTarget::Name)
                .or(workspace_number.map(sway_launch::WorkspaceTarget::Number)),
            output: self.output.as_deref(),
            height,
            width,
            position,
            scratchpad: self.scratchpad,
            sway_commands: &self.sway_commands,
            timeout: self
                .timeout
                .map(time::Duration::from_secs)
                .unwrap_or(default_timeout),
            wait_time: self
                .wait_time
                .map(time::Duration::from_millis)
                .unwrap_or(default_wait_time),
            verbose,
            require_launch_on_focused_workspace: false,
            warn_on_misplaced_launch: false,
            catch_late_window: false,
            excluded_containers: Vec::new(),
            settle,
        })
    }
}

#[cfg(test)]
mod tests;
