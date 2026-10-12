use crate::sway_launch::{self, Split};
use serde::Deserialize;
use std::collections::HashMap;
use std::time;

/// A declarative layout file: a sequence of steps, each the moral
/// equivalent of one `sway-launch` CLI invocation's flags. TOML's
/// array-of-tables syntax (`[[step]]`) maps directly onto `step: Vec<...>`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    #[serde(default)]
    pub step: Vec<LayoutStep>,
}

// Serialize is test-only, purely so main.rs's schema-parity test can read
// this struct's field names back at runtime — see
// schemas_mirror_args_field_for_field there. Nothing serializes a
// LayoutStep in a real run.
#[derive(Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(deny_unknown_fields)]
pub struct LayoutStep {
    pub command: Option<String>,
    pub con_id: Option<i64>,
    #[serde(default)]
    pub existing: bool,
    pub target_id: Option<String>,

    /// Names this step, so a later step can target its window via
    /// `target_id`. Layout-only — there's no CLI equivalent, since a single
    /// `sway-launch` invocation only ever has one step to reference.
    pub id: Option<String>,

    pub app_id: Option<String>,
    pub class: Option<String>,
    pub mark_match: Option<String>,

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
        // Checked before the emptiness-based target logic below, which reads a
        // blank value as "set" (it isn't empty) while every later use reads it
        // as a criterion that can only ever match nothing.
        for (field, value) in [
            ("app_id", self.app_id.as_deref()),
            ("class", self.class.as_deref()),
        ] {
            if let Some(value) = value {
                sway_launch::require_non_blank(field, value)?;
            }
        }

        let app_id_match = self.app_id.as_deref().unwrap_or_default();
        let class_match = self.class.as_deref().unwrap_or_default();
        let mark_match = self.mark_match.as_deref().unwrap_or_default();
        let match_fields_set = [
            !app_id_match.is_empty(),
            !class_match.is_empty(),
            !mark_match.is_empty(),
        ]
        .into_iter()
        .filter(|&set| set)
        .count();
        if match_fields_set > 1 {
            return Err("step must set only one of: app_id, class, mark_match".to_string());
        }
        // Mirrors the CLI's `conflicts_with_all` on `--app-id`/`--class`/
        // `--mark-match` against `--con-id`: a con_id target already names
        // an exact container, so a match criteria alongside it can only be
        // silently ignored, not honored — better to reject it than let a
        // step look like it's matching on identity when it isn't.
        //
        // `target_id` is covered by the identical reasoning: it resolves to
        // `Target::ConId` a few lines below, via an earlier step's already-
        // resolved id, so a match criterion alongside it is discarded just
        // as silently. It has no CLI equivalent to mirror a
        // `conflicts_with_all` from, which is why it needs saying here.
        if (self.con_id.is_some() || self.target_id.is_some())
            && (!app_id_match.is_empty() || !class_match.is_empty() || !mark_match.is_empty())
        {
            return Err(
                "step must not combine con_id/target_id with app_id/class/mark_match".to_string(),
            );
        }
        if !mark_match.is_empty() && !self.existing {
            return Err("mark_match requires existing = true".to_string());
        }

        let target_fields_set = [
            self.command.is_some(),
            self.con_id.is_some(),
            self.existing,
            self.target_id.is_some(),
        ]
        .into_iter()
        .filter(|&set| set)
        .count();
        if target_fields_set > 1 {
            return Err(
                "step must set only one of: command, con_id, existing, target_id".to_string(),
            );
        }

        let target = if let Some(con_id) = self.con_id {
            sway_launch::Target::ConId(con_id)
        } else if let Some(target_id) = self.target_id.as_deref() {
            let con_id = resolved_ids.get(target_id).ok_or_else(|| {
                format!(
                    "target_id {:?} not found — must reference an earlier step's id",
                    target_id
                )
            })?;
            sway_launch::Target::ConId(*con_id)
        } else if self.existing {
            if app_id_match.is_empty() && class_match.is_empty() && mark_match.is_empty() {
                return Err("existing = true requires app_id, class, or mark_match".to_string());
            }
            sway_launch::Target::Existing
        } else {
            let command = self
                .command
                .as_deref()
                .filter(|command| !command.trim().is_empty())
                .ok_or_else(|| {
                    "step needs one of: command, con_id, existing, target_id".to_string()
                })?;
            sway_launch::validate_exec_command(command)
                .map_err(|error| format!("command: {}", error))?;
            sway_launch::Target::Exec { command }
        };

        // Every value below is interpolated into a Sway command as a quoted
        // string (or, for mark_match, compared against one that was) — see
        // validate_sway_string_argument()'s doc comment for why `"`/`\` and
        // a blank value are rejected rather than silently mangled.
        for (field, value) in [
            ("mark", self.mark.as_deref()),
            ("mark_match", self.mark_match.as_deref()),
            ("workspace", self.workspace.as_deref()),
            ("output", self.output.as_deref()),
        ] {
            if let Some(value) = value {
                sway_launch::validate_sway_string_argument(value)
                    .map_err(|error| format!("{}: {}", field, error))?;
            }
        }

        if let Some(height) = self.height.as_deref() {
            sway_launch::validate_size_argument(height)
                .map_err(|error| format!("height: {}", error))?;
        }
        if let Some(width) = self.width.as_deref() {
            sway_launch::validate_size_argument(width)
                .map_err(|error| format!("width: {}", error))?;
        }
        if let Some(position) = self.position.as_deref() {
            sway_launch::validate_position_argument(position)
                .map_err(|error| format!("position: {}", error))?;
        }

        Ok(sway_launch::SwayLaunch {
            target,
            app_id_match,
            class_match,
            mark_match,
            split: self.split,
            floating: self.floating,
            sticky: self.sticky,
            fullscreen: self.fullscreen,
            focus: self.focus,
            mark: self.mark.as_deref().unwrap_or_default(),
            new_column: self.new_column,
            new_row: self.new_row,
            workspace: self.workspace.as_deref(),
            output: self.output.as_deref(),
            height: self.height.as_deref().map(sway_launch::parse_size),
            width: self.width.as_deref().map(sway_launch::parse_size),
            position: self.position.as_deref().map(sway_launch::parse_position),
            scratchpad: self.scratchpad,
            timeout: self
                .timeout
                .map(time::Duration::from_secs)
                .unwrap_or(default_timeout),
            wait_time: self
                .wait_time
                .map(time::Duration::from_millis)
                .unwrap_or(default_wait_time),
            verbose,
        })
    }
}

#[cfg(test)]
mod tests;
