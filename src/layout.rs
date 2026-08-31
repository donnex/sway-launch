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

#[derive(Deserialize)]
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
        if self.con_id.is_some()
            && (!app_id_match.is_empty() || !class_match.is_empty() || !mark_match.is_empty())
        {
            return Err("step must not combine con_id with app_id/class/mark_match".to_string());
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
mod tests {
    use super::*;

    #[test]
    fn parse_reads_a_representative_layout() {
        let layout = parse(
            r#"
            [[step]]
            command = "foot"
            app_id = "foot"
            split = "h"

            [[step]]
            command = "foot"
            app_id = "foot"
            "#,
        )
        .expect("valid layout should parse");

        assert_eq!(layout.step.len(), 2);
        assert_eq!(layout.step[0].command, Some("foot".to_string()));
        assert!(matches!(layout.step[0].split, Some(Split::H)));
        assert_eq!(layout.step[1].split, None);
    }

    #[test]
    fn parse_rejects_malformed_toml() {
        assert!(parse("this is not toml [[[").is_err());
    }

    #[test]
    fn parse_rejects_misspelled_step_field() {
        // Regression test: a prior version silently dropped unknown
        // fields, so a typo like "flaoting" for "floating" did nothing
        // instead of erroring.
        let result = parse(
            r#"
            [[step]]
            con_id = 42
            flaoting = true
            "#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn parse_rejects_wrong_table_name() {
        // Regression test: a prior version silently parsed [[steps]]
        // (plural, wrong) as a completely empty, successfully-parsed
        // layout instead of erroring.
        assert!(parse("[[steps]]\ncon_id = 42\n").is_err());
    }

    #[test]
    fn parse_defaults_to_no_steps_when_step_key_is_absent() {
        let layout = parse("").expect("empty file should still parse");
        assert!(layout.step.is_empty());
    }

    fn minimal_step() -> LayoutStep {
        LayoutStep {
            command: Some("foot".to_string()),
            con_id: None,
            existing: false,
            target_id: None,
            id: None,
            app_id: None,
            class: None,
            mark_match: None,
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

    #[test]
    fn to_sway_launch_exec_step_uses_command() {
        let step = minimal_step();
        let sway_launch = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .expect("valid step should convert");
        assert!(matches!(
            sway_launch.target,
            sway_launch::Target::Exec { command: "foot" }
        ));
    }

    #[test]
    fn to_sway_launch_con_id_step_ignores_command() {
        let mut step = minimal_step();
        step.command = None;
        step.con_id = Some(42);
        let sway_launch = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .expect("valid step should convert");
        assert!(matches!(sway_launch.target, sway_launch::Target::ConId(42)));
    }

    #[test]
    fn to_sway_launch_existing_step_requires_app_id_or_class() {
        let mut step = minimal_step();
        step.command = None;
        step.existing = true;
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_existing_step_with_app_id_succeeds() {
        let mut step = minimal_step();
        step.command = None;
        step.existing = true;
        step.app_id = Some("foot".to_string());
        let sway_launch = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .expect("existing step with app_id should convert");
        assert!(matches!(sway_launch.target, sway_launch::Target::Existing));
    }

    #[test]
    fn to_sway_launch_step_without_a_target_errors() {
        let mut step = minimal_step();
        step.command = None;
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_app_id_and_class_together() {
        let mut step = minimal_step();
        step.app_id = Some("foot".to_string());
        step.class = Some("Foot".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_app_id_and_mark_match_together() {
        let mut step = minimal_step();
        step.app_id = Some("foot".to_string());
        step.mark_match = Some("dropdown-term".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_con_id_and_mark_match_together() {
        let mut step = minimal_step();
        step.command = None;
        step.con_id = Some(42);
        step.mark_match = Some("dropdown-term".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_mark_match_without_existing() {
        let mut step = minimal_step();
        step.mark_match = Some("dropdown-term".to_string());
        let error = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .err()
            .expect("mark_match without existing should be rejected");
        assert!(error.contains("mark_match"));
    }

    #[test]
    fn to_sway_launch_rejects_class_and_mark_match_together() {
        let mut step = minimal_step();
        step.class = Some("Foot".to_string());
        step.mark_match = Some("dropdown-term".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_existing_step_with_mark_match_succeeds() {
        let mut step = minimal_step();
        step.command = None;
        step.existing = true;
        step.mark_match = Some("dropdown-term".to_string());
        let sway_launch = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .expect("existing step with mark_match should convert");
        assert!(matches!(sway_launch.target, sway_launch::Target::Existing));
        assert_eq!(sway_launch.mark_match, "dropdown-term");
    }

    #[test]
    fn to_sway_launch_rejects_empty_command() {
        let mut step = minimal_step();
        step.command = Some(String::new());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_whitespace_only_command() {
        let mut step = minimal_step();
        step.command = Some("   ".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_blank_id() {
        let mut step = minimal_step();
        step.id = Some("  ".to_string());
        let error = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .err()
            .expect("blank id should be rejected");
        assert!(
            error.contains("id"),
            "error should name the field: {error:?}"
        );
    }

    #[test]
    fn to_sway_launch_rejects_blank_target_id() {
        let mut step = minimal_step();
        step.command = None;
        step.target_id = Some("   ".to_string());
        let error = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .err()
            .expect("blank target_id should be rejected");
        assert!(
            error.contains("target_id"),
            "error should name the field: {error:?}"
        );
    }

    #[test]
    fn to_sway_launch_rejects_command_and_con_id_together() {
        // Regression test: a prior version silently preferred con_id over
        // command instead of erroring, unlike the CLI's conflicts_with.
        let mut step = minimal_step();
        step.con_id = Some(42);
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_con_id_and_app_id_together() {
        let mut step = minimal_step();
        step.command = None;
        step.con_id = Some(42);
        step.app_id = Some("foot".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_con_id_and_class_together() {
        let mut step = minimal_step();
        step.command = None;
        step.con_id = Some(42);
        step.class = Some("Foot".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_command_and_existing_together() {
        let mut step = minimal_step();
        step.existing = true;
        step.app_id = Some("foot".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_con_id_and_existing_together() {
        let mut step = minimal_step();
        step.command = None;
        step.con_id = Some(42);
        step.existing = true;
        step.app_id = Some("foot".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_command_and_target_id_together() {
        let mut step = minimal_step();
        step.target_id = Some("first".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_target_id_resolves_via_resolved_ids() {
        let mut step = minimal_step();
        step.command = None;
        step.target_id = Some("first".to_string());
        let mut resolved_ids = HashMap::new();
        resolved_ids.insert("first".to_string(), 42);
        let sway_launch = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &resolved_ids,
            )
            .expect("target_id present in resolved_ids should convert");
        assert!(matches!(sway_launch.target, sway_launch::Target::ConId(42)));
    }

    #[test]
    fn to_sway_launch_rejects_unresolved_target_id() {
        let mut step = minimal_step();
        step.command = None;
        step.target_id = Some("missing".to_string());
        let error = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .err()
            .expect("unresolved target_id should error");
        assert!(error.contains("missing"));
    }

    #[test]
    fn to_sway_launch_rejects_a_mark_containing_a_double_quote() {
        // Regression test: confirmed live that Sway stores such a mark with
        // the escape character intact, so --mark/--mark-match could never
        // round-trip it. See validate_sway_string_argument's doc comment.
        let mut step = minimal_step();
        step.mark = Some("dropdown\"term".to_string());
        let error = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .err()
            .expect("a mark containing a double quote should be rejected");
        assert!(
            error.contains("mark"),
            "error should name the field: {error:?}"
        );
    }

    #[test]
    fn to_sway_launch_rejects_a_blank_mark() {
        let mut step = minimal_step();
        step.mark = Some("   ".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_a_blank_workspace() {
        let mut step = minimal_step();
        step.workspace = Some("".to_string());
        let error = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .err()
            .expect("a blank workspace should be rejected");
        assert!(
            error.contains("workspace"),
            "error should name the field: {error:?}"
        );
    }

    #[test]
    fn to_sway_launch_rejects_an_output_containing_a_backslash() {
        let mut step = minimal_step();
        step.output = Some("HDMI\\A-1".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_accepts_a_mark_containing_command_separators() {
        // quote_sway_string() genuinely neutralizes these (confirmed live),
        // so the new validation must not over-reject them.
        let mut step = minimal_step();
        step.mark = Some("foo, exec bar; baz".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_ok());
    }

    #[test]
    fn to_sway_launch_rejects_invalid_height() {
        let mut step = minimal_step();
        step.height = Some("notasize".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_invalid_width() {
        let mut step = minimal_step();
        step.width = Some("notasize".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_invalid_position() {
        let mut step = minimal_step();
        step.position = Some("notaposition".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new()
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_uses_step_timeout_when_set() {
        let mut step = minimal_step();
        step.timeout = Some(7);
        let sway_launch = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .expect("valid step should convert");
        assert_eq!(sway_launch.timeout, time::Duration::from_secs(7));
    }

    #[test]
    fn to_sway_launch_falls_back_to_default_timeout() {
        let step = minimal_step();
        let sway_launch = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
                &HashMap::new(),
            )
            .expect("valid step should convert");
        assert_eq!(sway_launch.timeout, time::Duration::from_secs(5));
    }
}
