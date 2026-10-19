//! The validated value types the CLI and TOML layers hand in, and the
//! validators that let them.
//!
//! This is the outward-facing contract: `main.rs`'s `#[clap(value_parser)]`
//! attributes and `layout.rs`/`template.rs`'s own field checks all call the
//! `validate_*`/`parse_*` functions here, so a `--height 300px` flag and a
//! `height = "300px"` TOML field are accepted or rejected identically. The
//! `parse_*` functions validate and convert in one step, returning the typed
//! value rather than the string they were given.
//!
//! All pure, all unit-tested — nothing here has any idea a compositor exists.

use super::ipc::quote_sway_string;
use clap::ValueEnum;
use std::fmt;
use std::path::{Path, PathBuf};

// Serialize is test-only, so main.rs's schema-parity test can serialize a
// LayoutStep/TemplateStep to read its field names back — see
// schemas_mirror_args_field_for_field there.
#[derive(Copy, Clone, PartialEq, ValueEnum, serde::Deserialize, Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(rename_all = "lowercase")]
pub enum Split {
    V,
    H,
}

impl fmt::Display for Split {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Split::V => write!(f, "Vertical"),
            Split::H => write!(f, "Horizontal"),
        }
    }
}

/// A `--height`/`--width` value, parsed once via `parse_size()`
/// rather than carried as a string and re-parsed on every use (such as on
/// every iteration of `SwayAction::poll_shows()`'s `Height`/`Width` arms).
/// `Display` renders the same `<n>px`/`<n>ppt` text `sway_command()` needs
/// to interpolate into the Sway command, and that `SwayAction::Display`
/// needs for its human-readable `Sway action: ...` line — this makes
/// `sway_command()` a pure serialization step for these variants, not a
/// second place the format is defined.
///
/// `i32`, not `u32`, deliberately: the geometry these values are ultimately
/// compared against is `swayipc::Rect`'s, which is `i32`. Carrying them as
/// `u32` would mean casting in `poll_shows()`, where a value above
/// `i32::MAX` wraps negative and can never match anything the compositor
/// reports. Negative values never reach here regardless
/// (`parse_size()` rejects the sign outright), so the
/// signed type costs nothing and removes the cast rather than moving the
/// mismatch somewhere else.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Size {
    Pixels(i32),
    Percent(i32),
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Size::Pixels(pixels) => write!(f, "{}px", pixels),
            Size::Percent(percent) => write!(f, "{}ppt", percent),
        }
    }
}

/// A `--position` value, parsed once via `parse_position()`
/// rather than carried as a string — same reasoning as `Size` above.
/// `Display` renders `center` or `<x>,<y>`, the same text a `--position`
/// CLI argument or TOML field would itself use.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Position {
    Center,
    Coordinates { x: i32, y: i32 },
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Position::Center => write!(f, "center"),
            Position::Coordinates { x, y } => write!(f, "{},{}", x, y),
        }
    }
}

/// Parses `text` as an `i32` written as plain ASCII digits, with a leading
/// `-` only when `allow_negative` is set. `str::parse::<i32>()` alone would
/// also accept a leading `+`, which neither format allows; the overflow check
/// comes free with it, so the bound lives in exactly one place: `i32`, the
/// type the parsed value is held in.
fn parse_decimal_i32(text: &str, allow_negative: bool) -> Option<i32> {
    let digits = match text.strip_prefix('-') {
        Some(digits) if allow_negative => digits,
        _ => text,
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Parses a `--height`/`--width` value, or a `LayoutStep`'s `height`/
/// `width` field — both take the same `<N>px`/`<N>ppt` format, so a CLI flag
/// and a TOML field are accepted or rejected identically. Validation and
/// conversion are one step: there is no separately validated string for a
/// caller to forget to check before converting it.
///
/// `<N>` must fit in an `i32`, matching `Size`'s own representation and
/// therefore the `swayipc::Rect` geometry a pixel size is eventually compared
/// against. A wider bound such as `u32` would let a value in
/// `i32::MAX+1..=u32::MAX` through, reach Sway as a real command, and then
/// wrap negative on its way into `width_matches()`/`height_matches()`, so it
/// could never be confirmed — an accepted domain larger than the one the
/// rest of the program can represent.
///
/// Zero is rejected for both units, and `ppt` is capped at 100: confirmed
/// live that Sway leaves a `0px` height unchanged, doesn't produce a zero
/// width from `0ppt`, and clamps a percentage over 100 — each exiting 0 with
/// an `unconfirmed` status, so a typo like `600ppt` for `60ppt` was a silent
/// no-op or a surprising size instead of an error.
pub fn parse_size(value: &str) -> Result<Size, String> {
    let size = if let Some(pixels) = value.strip_suffix("px") {
        parse_decimal_i32(pixels, false)
            .filter(|&pixels| pixels >= 1)
            .map(Size::Pixels)
    } else if let Some(percent) = value.strip_suffix("ppt") {
        parse_decimal_i32(percent, false)
            .filter(|percent| (1..=100).contains(percent))
            .map(Size::Percent)
    } else {
        None
    };
    size.ok_or_else(|| {
        "Must be <N>px (N >= 1) or <N>ppt (N 1-100), e.g. 300px or 20ppt (ppt = percent)"
            .to_string()
    })
}

/// Parses a `--position` value, or a `LayoutStep`'s `position` field — both
/// take the same `center`/`<x>,<y>` format. `<x>`/`<y>` each allow a leading
/// `-`: the coordinates are relative to the window's workspace (Sway's
/// `move position` without `absolute`), and a negative one is a real
/// position partly past the workspace's left or top edge. Each must fit in an
/// `i32`, the same bound as `parse_size()`.
pub fn parse_position(value: &str) -> Result<Position, String> {
    if value == "center" {
        return Ok(Position::Center);
    }
    value
        .split_once(',')
        .and_then(|(x, y)| {
            Some(Position::Coordinates {
                x: parse_decimal_i32(x, true)?,
                y: parse_decimal_i32(y, true)?,
            })
        })
        .ok_or_else(|| {
            "Must be \"center\" or \"<X>,<Y>\" in pixels (X/Y may be negative). E.g. \
             center/100,200/-100,200"
                .to_string()
        })
}

/// Validates a value that gets interpolated into a Sway IPC command as a
/// quoted string — `--mark`/`--workspace`/`--output`, and the matching
/// `LayoutStep`/`TemplateStep` fields. Rejects a blank value (silently a
/// no-op or a nonsense target otherwise), both characters
/// `quote_sway_string()` escapes, a control character other than tab, and `$`
/// (see the comments
/// in the body).
///
/// The escaping exists to stop a `,`/`;` in a value from being read back as
/// an additional Sway command, and it does that correctly — but confirmed
/// live that Sway's own parser does *not* strip the escape
/// characters back out again: `mark "has\"quote"` stores the literal
/// `has\"quote`, and `mark "has\\backslash"` stores `has\\backslash`. So a
/// value containing `"` or `\` is silently corrupted rather than rejected,
/// which broke the round-trip `--mark`/`--mark-match` is built on (a mark
/// set as `dropdown"term` could never be matched again). Sway offers no way
/// to represent either character inside a quoted command argument, so
/// rejecting up front is the only option that isn't silent corruption.
///
/// `--mark-match` is validated the same way even though it's compared
/// client-side against `Node::marks` and never interpolated into a command:
/// once `--mark` rejects these characters, no mark this tool sets can
/// contain one, so accepting them here would only ever produce a guaranteed
/// no-match. The tradeoff is that a mark containing a backslash set by some
/// *other* tool (`swaymsg mark 'a\b'`) can no longer be retargeted by
/// `--mark-match`; judged worth it for the consistency, since this tool has
/// no way to create such a mark itself.
pub fn validate_sway_string_argument(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        return Err("Must not be empty or whitespace-only".to_string());
    }
    if let Some(character) = value.chars().find(|&c| c == '"' || c == '\\') {
        return Err(format!(
            "Must not contain {:?} — Sway stores it literally rather than unescaping it, so the \
             value would not round-trip",
            character
        ));
    }
    // A newline is rewritten rather than stored: confirmed live
    // that `mark "a<LF>b"` is stored as `a;b`, so --mark-match could never
    // find it again — the same round-trip failure as `"`/`\` above, reached by
    // a different mechanism (Sway's parser normalizing the separator rather
    // than keeping an escape character). NUL is rewritten the same way: Sway's
    // IPC handler splits the payload with C string functions, so an embedded
    // NUL (reachable through TOML's `\u0000`-style escapes, though never
    // through argv) is stored as `;` exactly like a newline.
    //
    // Every other control character except tab is rejected too, for a
    // different reason: tab and carriage return were probed live and do
    // round-trip, but a name is something status bars and `swaymsg -t
    // get_workspaces` later print, and a template — which carries no commands
    // and is meant to be shared — could otherwise plant an escape sequence
    // there for a terminal to act on. Tab is kept because it's printable
    // whitespace a name can plausibly contain.
    if let Some(character) = value.chars().find(|&c| c.is_control() && c != '\t') {
        let reason = if matches!(character, '\n' | '\0') {
            "Sway rewrites a newline or NUL to \";\" when storing the value, so the value would not \
             round-trip"
        } else {
            "a name is printed by status bars and swaymsg, where it would reach the terminal raw"
        };
        return Err(format!(
            "Must not contain a control character other than tab (found {:?}) — {}",
            character, reason
        ));
    }
    // Sway replaces `$name` with a `set $name ...` config variable in every
    // argument of an IPC command, quoted or not, so `--workspace '$ws1'` lands
    // on whatever `$ws1` is set to in the user's config and the confirmation
    // then waits for the literal text, which never appears. Which names are
    // defined depends on each user's config, and templates are meant to be
    // shared, so `$` is rejected outright rather than only when it happens to
    // name a variable.
    if value.contains('$') {
        return Err(
            "Must not contain \"$\" — Sway substitutes its config variables ($name) in the value, \
             so the value would not round-trip"
                .to_string(),
        );
    }
    Ok(value.to_string())
}

/// Values `move container to workspace` reads as keywords rather than as a
/// workspace name, quoted or not, compared case-insensitively the way Sway
/// matches them. Confirmed live on Sway 1.11; `number` and `to` alone are
/// parse errors there, and `number <n>` is an ordinary name.
const WORKSPACE_KEYWORDS: &[&str] = &[
    "next",
    "prev",
    "next_on_output",
    "prev_on_output",
    "current",
    "back_and_forth",
    "number",
    "to",
];

/// A `--title`/`title` pattern (#14): an unanchored regular expression a
/// window's title must match, as in Sway's own `title=` criteria, so
/// `project-a` matches `project-a — Visual Studio Code`. Compiled once, when
/// the flag or file is parsed, so a bad pattern is reported there rather than
/// partway through a run.
///
/// `regex-lite` rather than `regex`: the same syntax for anything a title
/// needs, without the Unicode tables (`\w`, `\d` and `(?i)` are ASCII-only),
/// which would add about a megabyte to the binary for matching a few
/// dozen short strings.
#[derive(Clone)]
pub struct TitlePattern {
    source: String,
    regex: regex_lite::Regex,
}

impl TitlePattern {
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether `title` (a node's `name`) matches. A window with no title
    /// never does.
    pub fn matches(&self, title: Option<&str>) -> bool {
        title.is_some_and(|title| self.regex.is_match(title))
    }
}

impl PartialEq for TitlePattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Eq for TitlePattern {}

impl fmt::Debug for TitlePattern {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "TitlePattern({:?})", self.source)
    }
}

/// `title matching "x"`: how messages name the criterion.
impl fmt::Display for TitlePattern {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "title matching \"{}\"", self.source)
    }
}

impl<'de> serde::Deserialize<'de> for TitlePattern {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let source = String::deserialize(deserializer)?;
        parse_title_pattern(&source).map_err(serde::de::Error::custom)
    }
}

// Test-only, for main.rs's schema-parity test, which serializes a step to
// read its field names back.
#[cfg(test)]
impl serde::Serialize for TitlePattern {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.source)
    }
}

/// Compiles a `--title`/`title` pattern, rejecting a blank one (it would
/// match every titled window) and one that isn't a valid regular expression.
pub fn parse_title_pattern(value: &str) -> Result<TitlePattern, String> {
    if value.trim().is_empty() {
        return Err("must not be empty or whitespace-only".to_string());
    }
    let regex = regex_lite::Regex::new(value)
        .map_err(|error| format!("{:?} is not a valid regular expression: {}", value, error))?;
    Ok(TitlePattern {
        source: value.to_string(),
        regex,
    })
}

/// Which workspace a move or a build location names: a workspace by its
/// exact name (`--workspace`), or by its number (`--workspace-number`), the
/// form `workspace number N` keybindings use, so `2` finds a workspace named
/// `2: web` (#43).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceTarget<'a> {
    Name(&'a str),
    Number(i32),
}

impl<'a> WorkspaceTarget<'a> {
    /// The workspace a name-or-number pair names, the name winning. Every
    /// front end has rejected a pair with both halves by the time it calls
    /// this: clap's `conflicts_with` on the CLI, `workspace_choice()` for
    /// the files.
    pub fn from_pair(name: Option<&'a str>, number: Option<i32>) -> Option<Self> {
        name.map(WorkspaceTarget::Name)
            .or(number.map(WorkspaceTarget::Number))
    }
}

impl WorkspaceTarget<'_> {
    /// The workspace argument of a Sway `workspace`/`move workspace` command.
    /// A number is sent unquoted after the `number` keyword; quoted, `number
    /// 2` would be an ordinary name.
    pub(super) fn sway_argument(&self) -> String {
        match self {
            WorkspaceTarget::Name(name) => quote_sway_string(name),
            WorkspaceTarget::Number(number) => format!("number {}", number),
        }
    }

    /// The Sway command that focuses this workspace. Without
    /// `--no-auto-back-and-forth`, a config with `workspace_auto_back_and_forth
    /// yes` makes Sway switch to the previous workspace instead when this one
    /// already has focus (#142, confirmed live).
    pub(super) fn focus_command(&self) -> String {
        format!(
            "workspace --no-auto-back-and-forth {}",
            self.sway_argument()
        )
    }

    /// Whether a workspace called `name`, numbered `number`, is this one.
    /// Names compare ignoring case, as Sway compares them; a number matches
    /// any workspace whose name starts with it, which is what Sway's `num`
    /// field records.
    pub(super) fn matches(&self, name: Option<&str>, number: Option<i32>) -> bool {
        match self {
            WorkspaceTarget::Name(wanted) => {
                name.is_some_and(|name| name.eq_ignore_ascii_case(wanted))
            }
            WorkspaceTarget::Number(wanted) => number == Some(*wanted),
        }
    }

    /// What the `--layout`/`--template` run lock keys this workspace on
    /// (#129): its number when it has one, so `--on-workspace-number 3`, a
    /// run focused on `3: web` and the same workspace after an icon daemon
    /// renamed it (#178) all take one lock; otherwise its name lowercased,
    /// since Sway compares names ignoring case. Two workspaces sharing a
    /// number (`3: web`, `3: mail`) share a lock too, which only ever refuses
    /// a run that `--no-lock` lets through.
    pub fn run_lock_key(&self) -> String {
        match self {
            WorkspaceTarget::Name(name) => sway_workspace_number(name).map_or_else(
                || format!("name {}", name.to_ascii_lowercase()),
                |number| format!("number {}", number),
            ),
            WorkspaceTarget::Number(number) => format!("number {}", number),
        }
    }
}

/// The `num` Sway gives a workspace called `name`: the number its name starts
/// with, or `None` (Sway's -1) when it doesn't start with a digit. Sway reads
/// it with `atoi()` after checking the first character, so `3: web` is 3 and
/// ` 3` is unnumbered; a number past `i32::MAX` counts as none here.
pub(super) fn sway_workspace_number(name: &str) -> Option<i32> {
    let digits = name.len() - name.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    name[..digits].parse().ok()
}

/// The name, or `number N`: how messages and the run lock name it.
impl fmt::Display for WorkspaceTarget<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WorkspaceTarget::Name(name) => write!(f, "{}", name),
            WorkspaceTarget::Number(number) => write!(f, "number {}", number),
        }
    }
}

/// `--workspace-number`/`workspace_number`: a whole number Sway can hold as a
/// workspace's `num`, which is an `int`. Negative numbers are refused: Sway
/// gives a name that doesn't start with a digit the number -1, so `-1` would
/// match every unnumbered workspace.
pub fn parse_workspace_number(value: &str) -> Result<i32, String> {
    let number: i64 = value.trim().parse().map_err(|_| {
        format!(
            "{:?} is not a workspace number (a whole number, 0 or more)",
            value
        )
    })?;
    validate_workspace_number(number)
}

/// `parse_workspace_number()`'s range check, for a TOML integer.
pub fn validate_workspace_number(number: i64) -> Result<i32, String> {
    i32::try_from(number)
        .ok()
        .filter(|number| *number >= 0)
        .ok_or_else(|| {
            format!(
                "{} is not a workspace number (a whole number from 0 to {})",
                number,
                i32::MAX
            )
        })
}

/// A file's `workspace`/`workspace_number` pair (a layout step, a binding,
/// a template's `[layout]` table) as the workspace it names: each half
/// validated as `--workspace`/`--workspace-number` are, and both at once
/// refused, as clap's `conflicts_with` refuses them on the CLI. An error
/// names the field at fault; the caller adds where the field is.
pub fn workspace_choice(
    name: Option<&str>,
    number: Option<i64>,
) -> Result<Option<WorkspaceTarget<'_>>, String> {
    if name.is_some() && number.is_some() {
        return Err(
            "workspace and workspace_number can't both be set — pick the workspace \
                    by name or by number"
                .to_string(),
        );
    }
    if let Some(name) = name {
        validate_workspace_argument(name).map_err(|error| format!("workspace: {}", error))?;
    }
    let number = number
        .map(validate_workspace_number)
        .transpose()
        .map_err(|error| format!("workspace_number: {}", error))?;
    Ok(WorkspaceTarget::from_pair(name, number))
}

/// Refuses a move to another workspace alongside `new_column`/`new_row`
/// (#45), for a layout step and for a binding placing a step's window; the
/// CLI refuses it with clap's `conflicts_with_all`. A relative move only
/// ever acts on the workspace the window opened on; the workspace move
/// after it then drops the window into the target as an ordinary new
/// sibling, so the new row/column never takes effect there.
pub fn check_relative_move(workspace: bool, new_column: bool, new_row: bool) -> Result<(), String> {
    if workspace && (new_column || new_row) {
        return Err(
            "workspace can't be combined with new_column/new_row — the relative move would \
             apply on the workspace the window opened on, not the target; build on the target \
             workspace instead (a template's [layout] table, or --on-workspace)"
                .to_string(),
        );
    }
    Ok(())
}

/// Values `move container to output` reads as directions or the current
/// output rather than as an output name, compared case-insensitively.
const OUTPUT_KEYWORDS: &[&str] = &["left", "right", "up", "down", "current"];

/// `validate_sway_string_argument()`, plus rejecting a Sway keyword
/// (`next`, `back_and_forth`, ...) as a `--workspace` value.
///
/// Sway honours the keywords even inside the quotes `sway-launch` sends, so
/// the window went somewhere other than a workspace of that name and the
/// confirmation then waited out `--timeout` for one that never appeared.
/// Upper-case forms are worse: confirmed live that `move workspace "NEXT"`
/// crashes Sway 1.11 outright.
pub fn validate_workspace_argument(value: &str) -> Result<String, String> {
    let value = validate_sway_string_argument(value)?;
    reject_keyword(&value, WORKSPACE_KEYWORDS, "workspace")?;
    Ok(value)
}

/// `validate_sway_string_argument()`, plus rejecting a Sway direction or
/// `current` as an `--output` value. See `validate_workspace_argument()`.
pub fn validate_output_argument(value: &str) -> Result<String, String> {
    let value = validate_sway_string_argument(value)?;
    reject_keyword(&value, OUTPUT_KEYWORDS, "output")?;
    Ok(value)
}

fn reject_keyword(value: &str, keywords: &[&str], kind: &str) -> Result<(), String> {
    match keywords
        .iter()
        .find(|keyword| keyword.eq_ignore_ascii_case(value))
    {
        Some(keyword) => Err(format!(
            "{:?} is a Sway keyword, not a {} name — for the keyword's own meaning, run \
             swaymsg \"[con_id=<id>] move container to {} {}\" with the printed container id",
            value, kind, kind, keyword
        )),
        None => Ok(()),
    }
}

/// `text` with every control character and bidirectional formatting
/// character rendered as a visible Rust escape (`\r`, `\u{1b}`,
/// `\u{202e}`, ...), for printing user-supplied text — a layout's `command`,
/// a TOML error quoting a file's line — to a terminal.
///
/// A TOML string can carry any character through an escape, so a layout file
/// printed verbatim could use a carriage return and an erase-line sequence to
/// make `--dry-run` show a different command from the one it runs, or put a
/// terminal escape in a comment so that the parse error quoting it acts on
/// the terminal. The bidirectional formatting characters
/// (`BIDI_FORMATTING_CHARACTERS`) aren't control characters to
/// `char::is_control`, but a terminal that implements bidi reorders how a
/// line is drawn around them ("Trojan Source", CVE-2021-42574), so text
/// outside a quoted string can look as if it sits inside one; they are
/// escaped too (#182). `keep_newlines` leaves `\n` as a real line break, for
/// a multi-line message whose newlines are its own formatting rather than
/// file content.
pub fn escape_control_characters(text: &str, keep_newlines: bool) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if is_bidi_formatting_character(character) {
            escaped.extend(character.escape_unicode());
        } else if character.is_control() && !(keep_newlines && character == '\n') {
            escaped.extend(character.escape_debug());
        } else {
            escaped.push(character);
        }
    }
    escaped
}

/// The Unicode bidirectional formatting characters: the Arabic letter mark,
/// the left-to-right and right-to-left marks, the embeddings and overrides
/// (U+202A to U+202E) and the isolates (U+2066 to U+2069).
const BIDI_FORMATTING_CHARACTERS: [std::ops::RangeInclusive<char>; 4] = [
    '\u{061c}'..='\u{061c}',
    '\u{200e}'..='\u{200f}',
    '\u{202a}'..='\u{202e}',
    '\u{2066}'..='\u{2069}',
];

fn is_bidi_formatting_character(character: char) -> bool {
    BIDI_FORMATTING_CHARACTERS
        .iter()
        .any(|range| range.contains(&character))
}

/// Validates the command a launch hands to Sway's `exec` — the positional
/// CLI command, and a layout step's (or a resolved template binding's)
/// `command` field.
///
/// The command is sent raw, not quoted (see `ExecAction::sway_command()` for why),
/// so it goes through Sway's own command parser before the shell ever sees
/// it, and that parser treats a few characters as command separators:
///
/// - an unquoted `,` or `;` ends the `exec` and starts a new Sway command, so
///   `foot -T a,b` launched `foot -T a` and then failed on `b` (leaving the
///   window open and unreported), and `foot; mark x` ran `mark x` against
///   whatever was focused. Scanned with the same rules Sway's `argsep()`
///   uses: `"`/`'` quote, a backslash escapes the next character, and only a
///   separator outside both counts. Quoting or escaping one is enough, since
///   Sway passes an `exec`'s text on with its quotes and backslashes intact,
///   so the shell then reads it exactly as typed.
/// - a newline or NUL anywhere, quoted or not: Sway's IPC handler rewrites
///   each to `;` before quote handling runs, so neither can be protected by
///   quoting. Confirmed live on Sway 1.11 (a quoted newline in a window title
///   arrived as `;`). A carriage return is rejected alongside the newline, as
///   a newline in all but name — a CRLF-edited file is where one comes from.
/// - an unquoted `#` starting a word, a shell comment, which would comment
///   out the `)` of the subshell `ExecAction::sway_command()` runs the
///   command in.
/// - an unclosed quote or a trailing unpaired backslash (#123), which would
///   run on into that `)` the same way.
/// - a backslash before a `'` inside single quotes (#151), the one place
///   Sway's quoting and the shell's disagree: Sway reads `\'` there as an
///   escaped quote and keeps the span open, while the shell has no escapes
///   inside `'...'` and ends it, so the scan above would judge the rest of
///   the command (a `#`, a `,`) as quoted when the shell doesn't.
///
/// Blank values aren't this function's concern: the CLI's own "Missing
/// COMMAND" check and `to_sway_launch()`'s target logic already report those
/// in their own words.
pub fn validate_exec_command(value: &str) -> Result<String, String> {
    if let Some(character) = value.chars().find(|&c| matches!(c, '\n' | '\r' | '\0')) {
        return Err(format!(
            "Must not contain {:?} — Sway turns it into a command separator, so the rest of the \
             line would run as a separate Sway command. Move a multi-line command into a script",
            character
        ));
    }

    if let Some(character) = unquoted_command_separator(value) {
        return Err(format!(
            "Must not contain an unquoted {:?} — Sway splits its commands on it, so the rest \
             would run as a separate Sway command. Wrap the part containing it in quotes, or \
             move the command into a script",
            character
        ));
    }
    if let Some(open) = open_quoting(value) {
        return Err(format!(
            "Must not end in {} — the command runs inside ( ... ), and Sway would read the \
             \")\" as part of it. Close the quote, or quote the backslash",
            open.describe()
        ));
    }
    if has_escaped_quote_in_single_quotes(value) {
        return Err(
            "Must not contain \\' inside single quotes — Sway reads it as an escaped quote, \
             but the shell ends the quote there, so the two would read the rest of the command \
             differently. Put the backslash outside the quotes ('a'\\'b') or use double quotes"
                .to_string(),
        );
    }
    if has_unquoted_comment(value) {
        return Err(
            "Must not contain a shell comment (an unquoted \"#\" starting a word) — the command \
             runs inside ( ... ), which the comment would cut short. Quote the \"#\" or remove \
             the comment"
                .to_string(),
        );
    }

    Ok(value.to_string())
}

/// The first `,` or `;` in `value` that Sway's parser would split a command
/// on; see `first_unquoted()`.
fn unquoted_command_separator(value: &str) -> Option<char> {
    first_unquoted(value, |character, _| matches!(character, ',' | ';'))
}

/// Whether `value` holds a `\'` inside single quotes, which Sway reads as an
/// escaped quote and the shell as a backslash and the closing quote; see
/// `validate_exec_command()`.
fn has_escaped_quote_in_single_quotes(value: &str) -> bool {
    scan_unquoted(value, |_, _| false).escaped_quote_in_single_quotes
}

/// Whether `value` holds a shell comment: an unquoted `#` starting a word.
/// `ExecAction::sway_command()` puts the command inside `( ... )`, so a
/// comment would swallow the `)` and nothing would launch.
fn has_unquoted_comment(value: &str) -> bool {
    first_unquoted(value, |character, previous| {
        character == '#'
            && previous.is_none_or(|previous| {
                previous.is_whitespace()
                    || matches!(previous, '&' | '|' | ';' | '(' | ')' | '<' | '>')
            })
    })
    .is_some()
}

/// The first character in `value` outside quotes for which `wanted` is true,
/// given it and the character before it; see `scan_unquoted()`.
fn first_unquoted(value: &str, wanted: impl FnMut(char, Option<char>) -> bool) -> Option<char> {
    scan_unquoted(value, wanted).found
}

/// What a scan of a Sway command can end inside, rather than outside any
/// quoting: whatever Sway reads after the text then continues it (#123).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenQuoting {
    DoubleQuote,
    SingleQuote,
    Backslash,
}

impl OpenQuoting {
    fn describe(self) -> &'static str {
        match self {
            OpenQuoting::DoubleQuote => "an unclosed '\"' quote",
            OpenQuoting::SingleQuote => "an unclosed \"'\" quote",
            OpenQuoting::Backslash => "an unpaired trailing '\\'",
        }
    }
}

/// The result of `scan_unquoted()`: the first wanted character, and, for a
/// scan that read the whole value, what quoting it ended inside, if any, and
/// whether it passed a `\'` inside single quotes, where the shell's reading
/// differs from Sway's.
struct QuoteScan {
    found: Option<char>,
    open: Option<OpenQuoting>,
    escaped_quote_in_single_quotes: bool,
}

/// Scans `value` with the rules Sway's `argsep()` uses, which match the
/// shell's closely enough for this: `"`/`'` quote, a backslash escapes the
/// next character, and only a character outside both counts. The one
/// difference that changes where a quote ends, a `\'` inside single quotes,
/// is recorded rather than modelled. Stops at the
/// first character outside quotes for which `wanted` is true, given it and
/// the character before it.
fn scan_unquoted(value: &str, mut wanted: impl FnMut(char, Option<char>) -> bool) -> QuoteScan {
    let mut in_double_quotes = false;
    let mut in_single_quotes = false;
    let mut escaped = false;
    let mut previous = None;
    let mut escaped_quote_in_single_quotes = false;
    for character in value.chars() {
        if character == '\'' && escaped && in_single_quotes {
            escaped_quote_in_single_quotes = true;
        }
        match character {
            '"' if !in_single_quotes && !escaped => in_double_quotes = !in_double_quotes,
            '\'' if !in_double_quotes && !escaped => in_single_quotes = !in_single_quotes,
            '\\' if !escaped => {
                escaped = true;
                previous = Some(character);
                continue;
            }
            _ if !in_double_quotes
                && !in_single_quotes
                && !escaped
                && wanted(character, previous) =>
            {
                return QuoteScan {
                    found: Some(character),
                    open: None,
                    escaped_quote_in_single_quotes,
                };
            }
            _ => {}
        }
        escaped = false;
        previous = Some(character);
    }
    let open = if escaped {
        Some(OpenQuoting::Backslash)
    } else if in_double_quotes {
        Some(OpenQuoting::DoubleQuote)
    } else if in_single_quotes {
        Some(OpenQuoting::SingleQuote)
    } else {
        None
    };
    QuoteScan {
        found: None,
        open,
        escaped_quote_in_single_quotes,
    }
}

/// What quoting `value` ends inside, if any: an unclosed quote or a trailing
/// unpaired backslash, which would carry on into whatever Sway reads after
/// the value and change how it splits that.
fn open_quoting(value: &str) -> Option<OpenQuoting> {
    scan_unquoted(value, |_, _| false).open
}

/// Why `command` must not be sent as it is, or `None`: Sway's parser would
/// split it into other than `expected` commands, or it ends inside quoting.
/// The pre-send check (#123) behind every command built from text a user
/// supplied — a launch's `exec` (one command) and a batch (one per action):
/// `validate_exec_command()` and `validate_sway_command()` check each value
/// on its own, and this checks what they add up to, before Sway has run any
/// of it. Sway's reply can only show a split after every part already ran.
/// A newline or NUL counts as a separator even inside quotes, since Sway
/// rewrites both to `;` before it looks at quoting.
pub(super) fn sway_command_split_problem(command: &str, expected: usize) -> Option<String> {
    let mut parts = 1;
    let scan = scan_unquoted(command, |character, _| {
        if matches!(character, ',' | ';') {
            parts += 1;
        }
        false
    });
    parts += command
        .chars()
        .filter(|&character| matches!(character, '\n' | '\0'))
        .count();
    if let Some(open) = scan.open {
        return Some(format!(
            "Refused to send {:?}: it ends in {}, so Sway would read it differently than it \
             was built",
            command,
            open.describe()
        ));
    }
    (parts != expected).then(|| {
        format!(
            "Refused to send {:?}: Sway would split it into {} commands, not the {} it was \
             built from",
            command, parts, expected
        )
    })
}

/// The Sway commands `--sway-command`/`sway_commands` may start with: the
/// ones that act on the container their criteria select. Sway runs any
/// command behind `[con_id=N]`, criteria or not, and most of the rest act
/// on the whole session: `exec`, `reload`, `bindsym`, `output`, `input`,
/// `set` and `exit` were all accepted that way and took effect, `exit`
/// ending the session (confirmed live on Sway 1.11). A layout or template
/// file is meant to be shared, so a step may only change its own window.
/// `kill` is left out too: a step that closes its own window has no
/// layout left to build, and so is `swap`, which moves another window into
/// this one's place (#154). `validate_sway_command()` narrows `focus` and
/// `move` further, and `SWAY_CLI_ONLY_COMMANDS` keeps two out of files.
/// Every entry was checked live on Sway 1.9 but `allow_tearing`, which 1.9
/// doesn't have and refuses with its own error.
pub const SWAY_WINDOW_COMMANDS: &[&str] = &[
    "allow_tearing",
    "border",
    "floating",
    "focus",
    "fullscreen",
    "inhibit_idle",
    "layout",
    "mark",
    "max_render_time",
    "move",
    "opacity",
    "resize",
    "scratchpad",
    "shortcuts_inhibitor",
    "split",
    "splith",
    "splitt",
    "splitv",
    "sticky",
    "title_format",
    "unmark",
    "urgent",
];

/// The `SWAY_WINDOW_COMMANDS` a layout or template file can't send,
/// only `--sway-command` (#154): each acts on the step's own window but
/// weakens a control the user set for the session. `inhibit_idle` keeps
/// swayidle's timeouts from firing, so the screen never locks while the
/// window is open (confirmed live on Sway 1.11), and `shortcuts_inhibitor
/// enable` lets the application grab every key, Sway's own bindings
/// included. A file may come from someone else; the command line is the
/// user's own.
pub const SWAY_CLI_ONLY_COMMANDS: &[&str] = &["inhibit_idle", "shortcuts_inhibitor"];

/// Validates one `--sway-command`/`sway_commands` entry (#90): a Sway
/// command sent as given behind `[con_id=N]`, for a window property no flag
/// models (`border none`, `opacity 0.9`).
///
/// It must start with one of `SWAY_WINDOW_COMMANDS`, compared ignoring case
/// as Sway looks commands up (`BORDER none` works, confirmed live). It can't hold an unquoted `,`/`;`, a newline,
/// or any other control character but tab, any of which would let one entry
/// become several Sway commands, the later ones free of the allowlist; nor
/// `$`, which Sway replaces with a config variable from the user's own
/// config. A leading `[` is refused with its own message, since the
/// criteria are what this tool supplies.
///
/// Two allowed commands reach past the window with some arguments, and are
/// refused with them (#154): `focus` with any argument (`focus output X`,
/// `focus left`, `focus parent`) moves focus elsewhere, which in a layout
/// makes the next launch open in the wrong place or fail
/// `focus_moved_during_launch()`, so only a bare `focus` is accepted; and
/// `move workspace ...` can move the window's whole workspace, every other
/// window on it included (`move workspace to output X`, what
/// `ActionKind::WorkspaceToOutput` relies on), so it is refused in every
/// form, `move container to workspace X` being the unambiguous spelling.
///
/// Nor can it end inside an unclosed quote or after an unpaired backslash
/// (#123): a step's entries reach Sway joined into one command, so an open
/// quote would carry on into the next entry and turn a `,` quoted there
/// into a separator, running what follows it free of the allowlist (an
/// `exec`, reproduced live on Sway 1.11).
pub fn validate_sway_command(value: &str) -> Result<String, String> {
    let command = value.trim();
    if command.is_empty() {
        return Err("Must not be empty or whitespace-only".to_string());
    }
    if command.starts_with('[') {
        return Err(
            "Must not start with criteria — sway-launch sends it as [con_id=N] <command> for the \
             step's own window"
                .to_string(),
        );
    }
    if let Some(character) = command.chars().find(|&c| c.is_control() && c != '\t') {
        return Err(format!(
            "Must not contain a control character other than tab (found {:?}) — Sway turns a \
             newline into a command separator",
            character
        ));
    }
    if let Some(character) = unquoted_command_separator(command) {
        return Err(format!(
            "Must not contain an unquoted {:?} — Sway would run the rest as a separate command. \
             Give each command its own entry",
            character
        ));
    }
    if let Some(open) = open_quoting(command) {
        return Err(format!(
            "Must not end in {} — a step's entries are sent to Sway as one command, so it \
             would run on into the next entry. Close the quote",
            open.describe()
        ));
    }
    if command.contains('$') {
        return Err(
            "Must not contain \"$\" — Sway substitutes its config variables ($name) in the \
             command"
                .to_string(),
        );
    }
    let name = command
        .split_whitespace()
        .next()
        .expect("a non-blank command has a first word");
    if !SWAY_WINDOW_COMMANDS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(name))
    {
        return Err(format!(
            "{:?} isn't a command that acts on one window, so a step can't send it. Allowed: {}",
            name,
            SWAY_WINDOW_COMMANDS.join(", ")
        ));
    }
    if let Some(problem) = reaches_beyond_the_window(command) {
        return Err(problem);
    }
    Ok(command.to_string())
}

fn reaches_beyond_the_window(command: &str) -> Option<String> {
    let mut words = command.split_whitespace();
    let name = words.next()?;
    let argument = words.next();
    if name.eq_ignore_ascii_case("focus") && argument.is_some() {
        return Some(
            "\"focus\" with an argument moves focus away from the window, so it isn't a command \
             that acts on one window. A bare \"focus\" focuses the step's own window"
                .to_string(),
        );
    }
    if name.eq_ignore_ascii_case("move")
        && argument.is_some_and(|word| word.eq_ignore_ascii_case("workspace"))
    {
        return Some(
            "\"move workspace\" can move the whole workspace, every window on it, so it isn't a \
             command that acts on one window. Use \"move container to workspace <name>\" (or \
             --workspace) to move just this window"
                .to_string(),
        );
    }
    None
}

/// `validate_sway_command()` for an entry in a layout or template
/// file, which also refuses `SWAY_CLI_ONLY_COMMANDS` (#154).
pub fn validate_shared_sway_command(value: &str) -> Result<String, String> {
    let command = validate_sway_command(value)?;
    let name = command
        .split_whitespace()
        .next()
        .expect("a validated command has a first word");
    if let Some(cli_only) = SWAY_CLI_ONLY_COMMANDS
        .iter()
        .find(|cli_only| cli_only.eq_ignore_ascii_case(name))
    {
        return Err(format!(
            "{:?} can only be given as --sway-command, not in a file — it overrides the \
             session's idle or shortcut settings, and a file may come from someone else",
            cli_only
        ));
    }
    Ok(command)
}

/// Renders a launch command given as a list of words (#77) into the text
/// sent to Sway's `exec`, so each word reaches the program exactly, with no
/// shell or Sway syntax applying to it. Each word is single-quoted for `sh`;
/// a `'` becomes `'\''` and a `\` is written outside the quotes as `\\`,
/// since Sway's own parser reads a backslash inside single quotes as
/// escaping the next character, so `'a\'` would end the quote early for Sway
/// (confirmed live). A `$` is refused: Sway replaces `$name` with a config
/// variable even inside single quotes (confirmed live). So are a newline, a
/// carriage return and NUL, which Sway turns into command separators first.
pub fn render_argv(words: &[String]) -> Result<String, String> {
    match words.first() {
        None => return Err("Must name at least the program to run".to_string()),
        Some(program) if program.trim().is_empty() => {
            return Err("The first word, the program to run, must not be blank".to_string())
        }
        Some(_) => {}
    }
    let rendered = words
        .iter()
        .map(|word| quote_shell_word(word))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rendered.join(" "))
}

/// One word quoted so it passes through Sway's parser and `sh` unchanged;
/// see `render_argv()` for why each special case is handled as it is.
pub(super) fn quote_shell_word(word: &str) -> Result<String, String> {
    if let Some(character) = word
        .chars()
        .find(|&c| matches!(c, '$' | '\n' | '\r' | '\0'))
    {
        return Err(format!(
            "{:?} in {:?}: Sway would read it before the program does ({}), so it can't be \
             passed through. Use the string form with a script if it's needed",
            character,
            word,
            if character == '$' {
                "as a config variable"
            } else {
                "as a command separator"
            }
        ));
    }
    let mut quoted = String::from("'");
    for character in word.chars() {
        match character {
            '\'' => quoted.push_str("'\\''"),
            '\\' => quoted.push_str("'\\\\'"),
            other => quoted.push(other),
        }
    }
    quoted.push('\'');
    Ok(quoted)
}

/// Checks an environment variable name for `--env`/`env`: a shell name
/// (`[A-Za-z_][A-Za-z0-9_]*`), since it's written as `export NAME=...`.
fn validate_env_name(name: &str) -> Result<(), String> {
    let mut characters = name.chars();
    let valid = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{:?} isn't a valid variable name: use letters, digits and _, not starting with a \
             digit",
            name
        ))
    }
}

/// Parses `--env KEY=VALUE` (#113).
pub fn parse_env_assignment(value: &str) -> Result<(String, String), String> {
    let (name, assigned) = value
        .split_once('=')
        .ok_or_else(|| "Must be KEY=VALUE, e.g. GDK_BACKEND=x11".to_string())?;
    validate_env_name(name)?;
    quote_shell_word(assigned)?;
    Ok((name.to_string(), assigned.to_string()))
}

/// The directory a launched command starts in (`--cwd`/`cwd`, #113): a
/// leading `~` is the home directory and a relative path is relative to the
/// caller's working directory. With `DirectoryCheck::Exists` it must also be
/// an existing directory, checked here so a real run fails before anything
/// launches. `--validate` and `--dry-run` pass `DirectoryCheck::FormatOnly`
/// (#146): they check a file meant for another machine too (CI, a dotfiles
/// repository), where `~/projects/website` needn't exist.
fn resolve_launch_directory(value: &str, check: DirectoryCheck) -> Result<PathBuf, String> {
    let expanded = match value.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| "~ needs $HOME, which isn't set".to_string())?;
            PathBuf::from(home).join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(value),
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(|error| format!("can't read the current directory: {}", error))?
            .join(expanded)
    };
    if check == DirectoryCheck::Exists && !absolute.is_dir() {
        return Err(format!("{} is not a directory", absolute.display()));
    }
    Ok(absolute)
}

/// Whether `launch_setup()` requires `--cwd`/`cwd` to exist on this machine:
/// a real run does, `--validate`/`--dry-run` check only its format (#146).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryCheck {
    /// A real run: the directory must exist.
    Exists,
    /// `--validate`/`--dry-run`: only the value's format is checked.
    FormatOnly,
}

impl DirectoryCheck {
    /// `FormatOnly` for a run that plans without launching.
    pub fn for_offline(offline: bool) -> Self {
        if offline {
            DirectoryCheck::FormatOnly
        } else {
            DirectoryCheck::Exists
        }
    }
}

/// `--cwd`/`--env` (or a step's `cwd`/`env`) checked and rendered for a
/// launch: the directory resolved by `resolve_launch_directory()`, then
/// `render_launch_setup()`. Both only apply to a launched command, so either
/// one without a command is an error. `check` says whether the directory
/// must exist here (see `resolve_launch_directory()`).
pub fn launch_setup(
    launches: bool,
    cwd: Option<&str>,
    env: &[(String, String)],
    check: DirectoryCheck,
) -> Result<Option<String>, String> {
    if !launches && (cwd.is_some() || !env.is_empty()) {
        return Err("cwd and env only apply to a launched command".to_string());
    }
    let directory = cwd
        .map(|value| resolve_launch_directory(value, check))
        .transpose()
        .map_err(|error| format!("cwd: {}", error))?;
    render_launch_setup(directory.as_deref(), env)
}

/// What runs in the launch shell before the command itself (#113):
/// `cd '<dir>'` and one `export NAME='value'` per variable, joined with
/// `&&` so a failed `cd` launches nothing. `None` when there's nothing to set.
fn render_launch_setup(
    cwd: Option<&Path>,
    env: &[(String, String)],
) -> Result<Option<String>, String> {
    let mut parts = Vec::new();
    if let Some(directory) = cwd {
        let directory = directory
            .to_str()
            .ok_or_else(|| format!("cwd: {} is not valid UTF-8", directory.display()))?;
        let quoted = quote_shell_word(directory).map_err(|error| format!("cwd: {}", error))?;
        parts.push(format!("cd {}", quoted));
    }
    for (name, value) in env {
        validate_env_name(name).map_err(|error| format!("env: {}", error))?;
        let quoted = quote_shell_word(value).map_err(|error| format!("env {}: {}", name, error))?;
        parts.push(format!("export {}={}", name, quoted));
    }
    Ok((!parts.is_empty()).then(|| parts.join(" && ")))
}

/// `require_non_blank()` in the shape clap's `value_parser` wants, for a flag
/// whose only constraint is "not blank" — `--app-id`/`--class`, the two
/// window-matching criteria. Unlike `validate_sway_string_argument()` above,
/// this deliberately does *not* reject `"`/`\`: these values are compared
/// client-side against a `Node`'s own `app_id`/`class` and never interpolated
/// into a Sway command, so there's no quoting round trip for those characters
/// to break, and an XWayland `WM_CLASS` containing one would be legitimate to
/// match on.
///
/// A blank value still has to go, for the same reason a blank `--mark` did:
/// it can only ever match nothing. Letting it through would also produce a
/// message that contradicts the command line — an empty `--app-id` is
/// indistinguishable from an absent one after `unwrap_or_default()`, so
/// `--existing --app-id ''` would report "--existing requires --app-id,
/// --class, or --mark-match" at a caller who had just passed `--app-id`.
pub fn validate_non_blank_argument(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        return Err("Must not be empty or whitespace-only".to_string());
    }
    Ok(value.to_string())
}

/// The largest `--timeout`/`timeout` value accepted, in seconds — `u32::MAX`,
/// about 136 years. Not a policy ceiling on how long a caller may wait (any
/// such number would be arbitrary, and neither `--timeout` nor `--wait-time`
/// is capped on those grounds); it exists because the deadline is computed as
/// `Instant::now() + timeout`, and `Instant` publishes no maximum, so a value
/// near `u64::MAX` overflowed it. `u32::MAX` seconds is representable on every
/// platform Rust's `Instant` supports, and far beyond any real wait.
pub const MAX_TIMEOUT_SECONDS: u64 = u32::MAX as u64;

/// Validates a `--timeout` value, or a `LayoutStep`'s `timeout` field, in
/// seconds. Zero is rejected because it can never succeed: every
/// timeout-bounded action sends its command first and then waits for the
/// event confirming it, so `0` reported a timeout over a command that had
/// already taken effect — a launched window with no container id printed, and
/// nothing for `--rollback-on-error` to clean up. The upper bound is
/// `MAX_TIMEOUT_SECONDS`, for the reason documented there.
pub fn validate_timeout_seconds(seconds: u64) -> Result<u64, String> {
    if (1..=MAX_TIMEOUT_SECONDS).contains(&seconds) {
        Ok(seconds)
    } else {
        Err(format!(
            "Must be between 1 and {} seconds",
            MAX_TIMEOUT_SECONDS
        ))
    }
}

/// `validate_timeout_seconds()` for the CLI's `--timeout`, which arrives as
/// text — the form `#[clap(value_parser)]` hands over.
pub fn validate_timeout_argument(value: &str) -> Result<u64, String> {
    let seconds = value.parse::<u64>().map_err(|_| {
        format!(
            "Must be a whole number of seconds between 1 and {}",
            MAX_TIMEOUT_SECONDS
        )
    })?;
    validate_timeout_seconds(seconds)
}

/// The longest `--settle` accepted, in milliseconds (#17). A client sets its
/// own size within a fraction of a second of mapping, so a minute is already
/// far past any wait that could help, and keeps a typo from holding a run for
/// days.
pub const MAX_SETTLE_MILLISECONDS: u64 = 60_000;

/// Validates a `--settle` value, or a step's or binding's `settle` field, in
/// milliseconds: `1..=MAX_SETTLE_MILLISECONDS`. Zero would watch for nothing,
/// which leaving the flag off already says.
pub fn validate_settle_milliseconds(milliseconds: u64) -> Result<u64, String> {
    if (1..=MAX_SETTLE_MILLISECONDS).contains(&milliseconds) {
        Ok(milliseconds)
    } else {
        Err(format!(
            "Must be between 1 and {} ms",
            MAX_SETTLE_MILLISECONDS
        ))
    }
}

/// `validate_settle_milliseconds()` for the CLI's `--settle`, which arrives
/// as text.
pub fn validate_settle_argument(value: &str) -> Result<u64, String> {
    let milliseconds = value.parse::<u64>().map_err(|_| {
        format!(
            "Must be a whole number of ms between 1 and {}",
            MAX_SETTLE_MILLISECONDS
        )
    })?;
    validate_settle_milliseconds(milliseconds)
}

/// `settle` as the duration a `SwayLaunch` carries, refused unless there is
/// geometry it can hold: a pixel `height`/`width` or a `position`. A `ppt`
/// size has no pixel figure to compare against, so a settle with only that
/// could never notice the client undoing it, and would only add a wait.
pub fn settle_duration(
    settle: Option<u64>,
    height: Option<Size>,
    width: Option<Size>,
    position: Option<Position>,
) -> Result<Option<std::time::Duration>, String> {
    let Some(milliseconds) = settle else {
        return Ok(None);
    };
    validate_settle_milliseconds(milliseconds)?;
    let pixels = |size: Option<Size>| matches!(size, Some(Size::Pixels(_)));
    if !pixels(height) && !pixels(width) && position.is_none() {
        return Err("needs a height or width in px, or a position, to hold".to_string());
    }
    Ok(Some(std::time::Duration::from_millis(milliseconds)))
}

/// Rejects an empty or whitespace-only value for a named field — shared by
/// every layout/template schema field that's a free-form identifier or
/// required string (`id`, `target_id`, `slot`, a binding's `command`, a
/// template's `description`/`category`) rather than something with its own
/// dedicated format, the way `parse_size()`/`parse_position()` above
/// validate a specific value shape.
pub fn require_non_blank(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} must not be empty or whitespace-only"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
