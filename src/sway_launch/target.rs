//! Which window an invocation acts on, and the one place the rules for
//! choosing it live.
//!
//! The CLI, a `--layout` step and a `--template` binding each name a target
//! through the same handful of fields (a command, a container id, `existing`,
//! an app_id/class/mark matcher and a title). `Target::from_fields()` is the
//! only way to turn those into a `Target`, and the types make every
//! combination it rejects impossible to build any other way: a launch can only
//! be filtered on app_id or class plus a title, an existing window needs at
//! most one matcher and at least one criterion, and a container id carries
//! none.

use super::tree::{window_app_id, window_class};
use super::values::{validate_exec_command, TitlePattern};
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

/// How `--existing` and `--list-windows` pick windows out: at most one
/// app_id/class/mark matcher, narrowed by a title pattern (#14). A `Target`
/// always has at least one of the two; `--list-windows` with neither lists
/// everything.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowCriteria<'a> {
    pub matcher: Option<WindowMatch<'a>>,
    pub title: Option<&'a TitlePattern>,
}

impl WindowCriteria<'_> {
    pub(super) fn matches(&self, node: &Node) -> bool {
        self.matcher.is_none_or(|matcher| matcher.matches(node))
            && self
                .title
                .is_none_or(|title| title.matches(node.name.as_deref()))
    }

    /// `app_id="foot", title=~"x"`: how `--dry-run` names the criteria.
    pub fn describe(&self) -> String {
        let matcher = self
            .matcher
            .map(|matcher| format!("{}={:?}", matcher.field_name(), matcher.value()));
        let title = self
            .title
            .map(|title| format!("title=~{:?}", title.source()));
        matcher
            .into_iter()
            .chain(title)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `app_id "foot" and title matching "x"`: how errors name the criteria.
impl fmt::Display for WindowCriteria<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match (self.matcher, self.title) {
            (Some(matcher), Some(title)) => write!(f, "{} and {}", matcher, title),
            (Some(matcher), None) => write!(f, "{}", matcher),
            (None, Some(title)) => write!(f, "{}", title),
            (None, None) => write!(f, "any window"),
        }
    }
}

/// What `SwayLaunch::run()` acts on: a window it launches, a specific
/// already-open container, or an already-open window found by its criteria —
/// with `wait`, one that may not have opened yet (#108).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Target<'a> {
    Exec {
        command: &'a str,
        filter: Option<LaunchFilter<'a>>,
        title: Option<&'a TitlePattern>,
    },
    ConId(i64),
    Existing {
        criteria: WindowCriteria<'a>,
        wait: bool,
    },
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
            WindowMatch::AppId(app_id) => window_app_id(node) == Some(app_id),
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
        WindowMatch::from(*self).matches(node)
    }
}

impl<'a> From<LaunchFilter<'a>> for WindowMatch<'a> {
    fn from(filter: LaunchFilter<'a>) -> Self {
        match filter {
            LaunchFilter::AppId(value) => WindowMatch::AppId(value),
            LaunchFilter::Class(value) => WindowMatch::Class(value),
        }
    }
}

/// `app_id "foot"`/`class "Code"`, for a launch's own messages: the same text
/// as the `WindowMatch` it names.
impl fmt::Display for LaunchFilter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        WindowMatch::from(*self).fmt(f)
    }
}

/// The target-selection fields every front end has, as it read them. A
/// `--layout` step's `target_id` is resolved to its container id by the
/// caller and passed as `target_id`; the CLI and a binding have none.
#[derive(Copy, Clone, Debug, Default)]
pub struct TargetFields<'a> {
    pub command: Option<&'a str>,
    pub con_id: Option<i64>,
    pub existing: bool,
    pub wait: bool,
    pub target_id: Option<i64>,
    pub app_id: Option<&'a str>,
    pub class: Option<&'a str>,
    pub mark_match: Option<&'a str>,
    pub title: Option<&'a TitlePattern>,
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
    WaitWithoutExisting,
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
                "--con-id can't be combined with --app-id, --class, --mark-match or --title"
                    .to_string()
            }
            TargetError::MatcherWithContainerId if style == FieldStyle::LayoutStep => {
                "must not combine con_id/target_id with app_id/class/mark_match/title".to_string()
            }
            TargetError::MatcherWithContainerId => {
                "must not combine con_id with app_id/class/mark_match/title".to_string()
            }
            TargetError::MarkMatchWithoutExisting if cli => {
                "--mark-match requires --existing".to_string()
            }
            TargetError::MarkMatchWithoutExisting => {
                "mark_match requires existing = true".to_string()
            }
            TargetError::ExistingWithoutMatcher if cli => {
                "--existing requires --app-id, --class, --mark-match or --title".to_string()
            }
            TargetError::ExistingWithoutMatcher => {
                "existing = true requires app_id, class, mark_match or title".to_string()
            }
            TargetError::WaitWithoutExisting if cli => "--wait requires --existing".to_string(),
            TargetError::WaitWithoutExisting => "wait = true requires existing = true".to_string(),
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
    /// most one matcher, and neither it nor a title alongside a container id;
    /// `existing` needs a matcher or a title, and `mark_match` and `wait`
    /// need `existing`; and no blank value.
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

        if fields.wait && !fields.existing {
            return Err(TargetError::WaitWithoutExisting);
        }
        if let Some(con_id) = fields.con_id.or(fields.target_id) {
            return match (matcher, fields.title) {
                (None, None) => Ok(Target::ConId(con_id)),
                _ => Err(TargetError::MatcherWithContainerId),
            };
        }
        if fields.existing {
            if matcher.is_none() && fields.title.is_none() {
                return Err(TargetError::ExistingWithoutMatcher);
            }
            return Ok(Target::Existing {
                criteria: WindowCriteria {
                    matcher,
                    title: fields.title,
                },
                wait: fields.wait,
            });
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
        Ok(Target::Exec {
            command,
            filter,
            title: fields.title,
        })
    }
}

#[cfg(test)]
mod tests;
