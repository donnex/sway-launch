use crate::sway_launch::{self, Split};
use serde::Deserialize;
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

    pub app_id: Option<String>,
    pub class: Option<String>,

    pub split: Option<Split>,
    #[serde(default)]
    pub floating: bool,
    #[serde(default)]
    pub fullscreen: bool,
    pub mark: Option<String>,
    #[serde(default)]
    pub new_column: bool,
    #[serde(default)]
    pub new_row: bool,
    pub workspace: Option<String>,
    pub height: Option<String>,
    pub width: Option<String>,
    pub position: Option<String>,

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
    /// `command`/`con_id`/`existing` is set (mirroring the CLI's
    /// `conflicts_with` on the same three) and validating the same
    /// height/width/position formats the CLI flags enforce. Steps without
    /// their own `timeout`/`wait_time` inherit the caller's defaults
    /// (typically the top-level `--timeout`/`--wait-time` CLI values).
    pub fn to_sway_launch(
        &self,
        default_timeout: time::Duration,
        default_wait_time: time::Duration,
        verbose: bool,
    ) -> Result<sway_launch::SwayLaunch<'_>, String> {
        let app_id_match = self.app_id.as_deref().unwrap_or_default();
        let class_match = self.class.as_deref().unwrap_or_default();

        let target_fields_set = [self.command.is_some(), self.con_id.is_some(), self.existing]
            .into_iter()
            .filter(|&set| set)
            .count();
        if target_fields_set > 1 {
            return Err("step must set only one of: command, con_id, existing".to_string());
        }

        let target = if let Some(con_id) = self.con_id {
            sway_launch::Target::ConId(con_id)
        } else if self.existing {
            if app_id_match.is_empty() && class_match.is_empty() {
                return Err("existing = true requires app_id or class".to_string());
            }
            sway_launch::Target::Existing
        } else {
            let command = self
                .command
                .as_deref()
                .ok_or_else(|| "step needs one of: command, con_id, existing".to_string())?;
            sway_launch::Target::Exec { command }
        };

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
            split: self.split,
            floating: self.floating,
            fullscreen: self.fullscreen,
            mark: self.mark.as_deref().unwrap_or_default(),
            new_column: self.new_column,
            new_row: self.new_row,
            workspace: self.workspace.as_deref(),
            height: self.height.as_deref(),
            width: self.width.as_deref(),
            position: self.position.as_deref(),
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
            command = "kitty"
            app_id = "kitty"
            split = "h"

            [[step]]
            command = "kitty"
            app_id = "kitty"
            "#,
        )
        .expect("valid layout should parse");

        assert_eq!(layout.step.len(), 2);
        assert_eq!(layout.step[0].command, Some("kitty".to_string()));
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
            command: Some("kitty".to_string()),
            con_id: None,
            existing: false,
            app_id: None,
            class: None,
            split: None,
            floating: false,
            fullscreen: false,
            mark: None,
            new_column: false,
            new_row: false,
            workspace: None,
            height: None,
            width: None,
            position: None,
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
            )
            .expect("valid step should convert");
        assert!(matches!(
            sway_launch.target,
            sway_launch::Target::Exec { command: "kitty" }
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
                false
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_existing_step_with_app_id_succeeds() {
        let mut step = minimal_step();
        step.command = None;
        step.existing = true;
        step.app_id = Some("kitty".to_string());
        let sway_launch = step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false,
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
                false
            )
            .is_err());
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
                false
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_command_and_existing_together() {
        let mut step = minimal_step();
        step.existing = true;
        step.app_id = Some("kitty".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_con_id_and_existing_together() {
        let mut step = minimal_step();
        step.command = None;
        step.con_id = Some(42);
        step.existing = true;
        step.app_id = Some("kitty".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false
            )
            .is_err());
    }

    #[test]
    fn to_sway_launch_rejects_invalid_height() {
        let mut step = minimal_step();
        step.height = Some("notasize".to_string());
        assert!(step
            .to_sway_launch(
                time::Duration::from_secs(5),
                time::Duration::from_millis(20),
                false
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
                false
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
                false
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
            )
            .expect("valid step should convert");
        assert_eq!(sway_launch.timeout, time::Duration::from_secs(5));
    }
}
