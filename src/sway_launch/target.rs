//! Which window an invocation acts on, and the one place the rules for
//! choosing it live.
//!
//! The CLI, a `--layout` step and a `--template` binding each name a target
//! through the same handful of fields (a command, a container id, `existing`,
//! and an app_id/class/mark matcher). `Target::from_fields()` is the only way
//! to turn those into a `Target`, and the types make every combination it
//! rejects impossible to build any other way: a launch can only be filtered
//! on app_id or class, an existing window needs exactly one matcher, and a
//! container id carries none.

use super::values::validate_exec_command;
use std::fmt;
use swayipc::Node;

/// How `--existing` (or `existing = true`) finds an already-open window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WindowMatch<'a> {
    AppId(&'a str),
    Class(&'a str),
    Mark(&'a str),
}

/// What a launch accepts as its new window. A mark can't be one: a window
/// that has only just mapped has no marks yet.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LaunchFilter<'a> {
    AppId(&'a str),
    Class(&'a str),
}

/// What `SwayLaunch::run()` acts on: a window it launches, a specific
/// already-open container, or an already-open window found by a matcher.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Target<'a> {
    Exec {
        command: &'a str,
        filter: Option<LaunchFilter<'a>>,
    },
    ConId(i64),
    Existing(WindowMatch<'a>),
}

impl WindowMatch<'_> {
    /// The field this matcher came from, as a step or binding spells it.
    pub fn field_name(&self) -> &'static str {
        match self {
            WindowMatch::AppId(_) => "app_id",
            WindowMatch::Class(_) => "class",
            WindowMatch::Mark(_) => "mark_match",
        }
    }

    pub fn value(&self) -> &str {
        match self {
            WindowMatch::AppId(value) | WindowMatch::Class(value) | WindowMatch::Mark(value) => {
                value
            }
        }
    }

    pub(super) fn matches(&self, node: &Node) -> bool {
        match *self {
            WindowMatch::AppId(app_id) => node.app_id.as_deref() == Some(app_id),
            WindowMatch::Class(class) => window_class(node) == Some(class),
            WindowMatch::Mark(mark) => node.marks.iter().any(|held| held == mark),
        }
    }
}

/// `app_id "foot"`, `class "Code"`, `mark "dropdown"`: how errors name the
/// criterion.
impl fmt::Display for WindowMatch<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WindowMatch::AppId(value) => write!(f, "app_id \"{}\"", value),
            WindowMatch::Class(value) => write!(f, "class \"{}\"", value),
            WindowMatch::Mark(value) => write!(f, "mark \"{}\"", value),
        }
    }
}

impl LaunchFilter<'_> {
    pub(super) fn matches(&self, node: &Node) -> bool {
        match *self {
            LaunchFilter::AppId(app_id) => node.app_id.as_deref() == Some(app_id),
            LaunchFilter::Class(class) => window_class(node) == Some(class),
        }
    }
}

/// ` with app_id "foot"`/` with class "Code"`, for a launch's own messages.
impl fmt::Display for LaunchFilter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LaunchFilter::AppId(value) => write!(f, "app_id \"{}\"", value),
            LaunchFilter::Class(value) => write!(f, "class \"{}\"", value),
        }
    }
}

fn window_class(node: &Node) -> Option<&str> {
    node.window_properties
        .as_ref()
        .and_then(|properties| properties.class.as_deref())
}

/// The target-selection fields every front end has, as it read them. A
/// `--layout` step's `target_id` is resolved to its container id by the
/// caller and passed as `target_id`; the CLI and a binding have none.
#[derive(Copy, Clone, Debug, Default)]
pub struct TargetFields<'a> {
    pub command: Option<&'a str>,
    pub con_id: Option<i64>,
    pub existing: bool,
    pub target_id: Option<i64>,
    pub app_id: Option<&'a str>,
    pub class: Option<&'a str>,
    pub mark_match: Option<&'a str>,
}

/// Which front end the fields came from, so a rejection names them the way
/// the user wrote them (`--existing` on the command line, `existing = true`
/// in a file).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FieldStyle {
    Cli,
    LayoutStep,
    Binding,
}

/// Why `Target::from_fields()` rejected a combination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetError {
    NoTarget,
    SeveralTargets,
    SeveralMatchers,
    MatcherWithContainerId,
    MarkMatchWithoutExisting,
    ExistingWithoutMatcher,
    Blank(&'static str),
    Command(String),
}

impl TargetError {
    /// The rejection in `style`'s own vocabulary.
    pub fn message(&self, style: FieldStyle) -> String {
        let cli = style == FieldStyle::Cli;
        let target_fields = match style {
            FieldStyle::Cli => "COMMAND, or one of --con-id/--existing",
            FieldStyle::LayoutStep => "command, con_id, existing, target_id",
            FieldStyle::Binding => "command, con_id, existing",
        };
        match self {
            TargetError::NoTarget if cli => format!("Missing {target_fields}"),
            TargetError::NoTarget => format!("needs one of: {target_fields}"),
            TargetError::SeveralTargets if cli => {
                "COMMAND, --con-id and --existing can't be combined".to_string()
            }
            TargetError::SeveralTargets => format!("must set only one of: {target_fields}"),
            TargetError::SeveralMatchers if cli => {
                "--app-id, --class and --mark-match can't be combined".to_string()
            }
            TargetError::SeveralMatchers => {
                "must set only one of: app_id, class, mark_match".to_string()
            }
            TargetError::MatcherWithContainerId if cli => {
                "--con-id can't be combined with --app-id, --class or --mark-match".to_string()
            }
            TargetError::MatcherWithContainerId if style == FieldStyle::LayoutStep => {
                "must not combine con_id/target_id with app_id/class/mark_match".to_string()
            }
            TargetError::MatcherWithContainerId => {
                "must not combine con_id with app_id/class/mark_match".to_string()
            }
            TargetError::MarkMatchWithoutExisting if cli => {
                "--mark-match requires --existing".to_string()
            }
            TargetError::MarkMatchWithoutExisting => {
                "mark_match requires existing = true".to_string()
            }
            TargetError::ExistingWithoutMatcher if cli => {
                "--existing requires --app-id, --class, or --mark-match".to_string()
            }
            TargetError::ExistingWithoutMatcher => {
                "existing = true requires app_id, class, or mark_match".to_string()
            }
            TargetError::Blank(field) if cli => {
                format!("{} must not be empty or whitespace-only", cli_name(field))
            }
            TargetError::Blank(field) => format!("{field} must not be empty or whitespace-only"),
            TargetError::Command(error) if cli => format!("COMMAND: {error}"),
            TargetError::Command(error) => format!("command: {error}"),
        }
    }
}

fn cli_name(field: &str) -> &str {
    match field {
        "command" => "COMMAND",
        "app_id" => "--app-id",
        "class" => "--class",
        "mark_match" => "--mark-match",
        other => other,
    }
}

impl<'a> Target<'a> {
    /// The one place the target rules live: exactly one of a command, a
    /// container id (`con_id` or a resolved `target_id`) or `existing`; at
    /// most one matcher, never alongside a container id; `existing` needs a
    /// matcher and `mark_match` needs `existing`; and no blank value.
    ///
    /// A blank matcher is checked before anything counts fields, since it
    /// would otherwise count as set and then match nothing. A launch command
    /// also goes through `validate_exec_command()`, so all three front ends
    /// reject the same commands.
    pub fn from_fields(fields: TargetFields<'a>) -> Result<Self, TargetError> {
        for (field, value) in [
            ("command", fields.command),
            ("app_id", fields.app_id),
            ("class", fields.class),
            ("mark_match", fields.mark_match),
        ] {
            if value.is_some_and(|value| value.trim().is_empty()) {
                return Err(TargetError::Blank(field));
            }
        }

        let matchers: Vec<WindowMatch<'a>> = [
            fields.app_id.map(WindowMatch::AppId),
            fields.class.map(WindowMatch::Class),
            fields.mark_match.map(WindowMatch::Mark),
        ]
        .into_iter()
        .flatten()
        .collect();
        if matchers.len() > 1 {
            return Err(TargetError::SeveralMatchers);
        }
        let matcher = matchers.first().copied();

        let targets = [
            fields.command.is_some(),
            fields.con_id.is_some(),
            fields.existing,
            fields.target_id.is_some(),
        ]
        .into_iter()
        .filter(|&set| set)
        .count();
        if targets > 1 {
            return Err(TargetError::SeveralTargets);
        }

        if let Some(con_id) = fields.con_id.or(fields.target_id) {
            return match matcher {
                Some(_) => Err(TargetError::MatcherWithContainerId),
                None => Ok(Target::ConId(con_id)),
            };
        }
        if fields.existing {
            return matcher
                .map(Target::Existing)
                .ok_or(TargetError::ExistingWithoutMatcher);
        }
        if matches!(matcher, Some(WindowMatch::Mark(_))) {
            return Err(TargetError::MarkMatchWithoutExisting);
        }
        let command = fields.command.ok_or(TargetError::NoTarget)?;
        validate_exec_command(command).map_err(TargetError::Command)?;
        let filter = match matcher {
            Some(WindowMatch::AppId(app_id)) => Some(LaunchFilter::AppId(app_id)),
            Some(WindowMatch::Class(class)) => Some(LaunchFilter::Class(class)),
            Some(WindowMatch::Mark(_)) | None => None,
        };
        Ok(Target::Exec { command, filter })
    }
}

#[cfg(test)]
mod tests;
