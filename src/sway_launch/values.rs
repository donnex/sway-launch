//! The validated value types the CLI and TOML layers hand in, and the
//! validators that let them.
//!
//! This is the outward-facing contract: `main.rs`'s `#[clap(value_parser)]`
//! attributes and `layout.rs`/`template.rs`'s own field checks all call the
//! `validate_*` functions here, so a `--height 300px` flag and a
//! `height = "300px"` TOML field are accepted or rejected identically. The
//! `parse_*` functions are the second half of that arrangement: infallible by
//! construction, because a value that reaches them has already passed the
//! matching validator.
//!
//! All pure, all unit-tested — nothing here has any idea a compositor exists.

use clap::ValueEnum;
use regex::Regex;
use std::fmt;

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

/// A validated `--height`/`--width` value, parsed once via `parse_size()`
/// rather than carried as a string and re-parsed on every use (such as on
/// every iteration of `SwayAction::poll_matches()`'s `Height`/`Width` arms).
/// `Display` renders the same `<n>px`/`<n>ppt` text `sway_command()` needs
/// to interpolate into the Sway command, and that `SwayAction::Display`
/// needs for its human-readable `Sway action: ...` line — this makes
/// `sway_command()` a pure serialization step for these variants, not a
/// second place the format is defined.
///
/// `i32`, not `u32`, deliberately: the geometry these values are ultimately
/// compared against is `swayipc::Rect`'s, which is `i32`. Carrying them as
/// `u32` would mean casting in `poll_matches()`, where a value above
/// `i32::MAX` wraps negative and can never match anything the compositor
/// reports. Negative values never reach here regardless
/// (`validate_size_argument()`'s `\d+` rejects the sign outright), so the
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

/// Parses a value already validated by `validate_size_argument()` — never
/// called on unvalidated input. `validate_size_argument()` itself confirms
/// the digits fit in an `i32`, not just that they're digits, specifically so
/// the `.expect()`s here are trusting an already-checked invariant rather
/// than gambling on one.
pub fn parse_size(value: &str) -> Size {
    if let Some(pixels) = value.strip_suffix("px") {
        Size::Pixels(
            pixels
                .parse()
                .expect("validate_size_argument guarantees this parses as i32"),
        )
    } else {
        let percent = value
            .strip_suffix("ppt")
            .expect("validate_size_argument guarantees a px or ppt suffix");
        Size::Percent(
            percent
                .parse()
                .expect("validate_size_argument guarantees this parses as i32"),
        )
    }
}

/// A validated `--position` value, parsed once via `parse_position()`
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

/// Parses a value already validated by `validate_position_argument()` —
/// never called on unvalidated input. `validate_position_argument()` itself
/// confirms `<x>`/`<y>` each fit in an `i32`, so the `.expect()`s here are
/// trusting an already-checked invariant rather than gambling on one.
pub fn parse_position(value: &str) -> Position {
    if value == "center" {
        return Position::Center;
    }
    let (x, y) = value
        .split_once(',')
        .expect("validate_position_argument guarantees a comma-separated pair");
    Position::Coordinates {
        x: x.parse()
            .expect("validate_position_argument guarantees this parses as i32"),
        y: y.parse()
            .expect("validate_position_argument guarantees this parses as i32"),
    }
}

/// Validates a `--height`/`--width` value, or a `LayoutStep`'s `height`/
/// `width` field — both take the same `\d+(px|ppt)` format. Also confirms
/// the digits actually fit in an `i32`, not just that the regex's unbounded
/// `\d+` matched — `parse_size()` trusts a value that passed this check to
/// parse infallibly, so a value that matches the shape but overflows (e.g.
/// 11+ digits) must be rejected here, not discovered as a panic later.
///
/// The bound is `i32`, matching `Size`'s own representation and therefore the
/// `swayipc::Rect` geometry a pixel size is eventually compared against. A
/// wider bound such as `u32` would let a value in `i32::MAX+1..=u32::MAX`
/// pass validation, reach Sway as a real command, and then wrap negative on
/// its way into `width_matches()`/`height_matches()`, so it could never be
/// confirmed — an accepted domain larger than the one the rest of the
/// program can represent.
/// Rejecting it here is what keeps validation's promise ("anything that
/// passes is something the whole pipeline can handle") true, rather than only
/// true of the parser.
///
/// Zero is rejected for both units, and `ppt` is capped at 100: confirmed
/// live that Sway leaves a `0px` height unchanged, doesn't produce a zero
/// width from `0ppt`, and clamps a percentage over 100 — each exiting 0 with
/// an `unconfirmed` status, so a typo like `600ppt` for `60ppt` was a silent
/// no-op or a surprising size instead of an error.
pub fn validate_size_argument(value: &str) -> Result<String, String> {
    let re = Regex::new(r"^(\d+)(px|ppt)$").unwrap();
    let valid = re.captures(value).is_some_and(|captures| {
        match (captures[1].parse::<i32>(), &captures[2]) {
            (Ok(number), "px") => number >= 1,
            (Ok(number), _) => (1..=100).contains(&number),
            (Err(_), _) => false,
        }
    });
    if valid {
        Ok(value.to_string())
    } else {
        Err(
            "Must be <N>px (N >= 1) or <N>ppt (N 1-100), e.g. 300px or 20ppt (ppt = percent)"
                .to_string(),
        )
    }
}

/// Validates a `--position` value, or a `LayoutStep`'s `position` field —
/// both take the same `center`/`<x>,<y>` format. `<x>`/`<y>` each allow a
/// leading `-`: Sway's coordinate space is global across every output, and
/// an output positioned left of or above the primary one legitimately has a
/// negative origin (confirmed live: `compute_center_position()`, used for
/// `--position center`, already accounts for output origin and can itself
/// land on a negative coordinate on such a layout) — rejecting a
/// user-supplied negative coordinate here would make the tool unable to
/// target a position its own `center` computation can already produce.
/// Also confirms `<x>`/`<y>` each actually fit in an `i32`, not just that
/// the regex's unbounded `\d+` matched — `parse_position()` trusts a value
/// that passed this check to parse infallibly, so a value that matches the
/// shape but overflows must be rejected here, not discovered as a panic
/// later (same reasoning as `validate_size_argument`'s own `i32` check).
pub fn validate_position_argument(value: &str) -> Result<String, String> {
    if value == "center" {
        return Ok(value.to_string());
    }
    let re = Regex::new(r"^(-?\d+),(-?\d+)$").unwrap();
    match re.captures(value) {
        Some(captures)
            if captures[1].parse::<i32>().is_ok() && captures[2].parse::<i32>().is_ok() =>
        {
            Ok(value.to_string())
        }
        _ => Err(
            "Must be \"center\" or \"<X>,<Y>\" in pixels (X/Y may be negative). E.g. \
             center/100,200/-100,200"
                .to_string(),
        ),
    }
}

/// Validates a value that gets interpolated into a Sway IPC command as a
/// quoted string — `--mark`/`--workspace`/`--output`, and the matching
/// `LayoutStep`/`TemplateStep` fields. Rejects a blank value (silently a
/// no-op or a nonsense target otherwise), and both characters
/// `quote_sway_string()` escapes.
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
    // than keeping an escape character).
    //
    // Only the newline. Probed the neighbouring control characters against a
    // live compositor rather than assuming a category: tab and carriage return
    // both round-trip byte-for-byte, as do `,`/`;` and ordinary text, so
    // rejecting control characters as a class would be over-rejection.
    //
    // This was missed when `"`/`\` were fixed because the tests covering
    // newlines asserted the *storage* behaviour (one literal mark, nothing
    // executed — which was and is true) rather than whether the value came
    // back out again.
    //
    // NUL joins it for the same reason: Sway's IPC handler splits the payload
    // with C string functions, so an embedded NUL (reachable through TOML's
    // `\^@`-style escapes, though never through argv) is rewritten to `;`
    // exactly like a newline — `mark = "ab<NUL>cd"` is stored as `ab;cd`.
    if let Some(character) = value.chars().find(|&c| c == '\n' || c == '\0') {
        return Err(format!(
            "Must not contain a newline or NUL (found {:?}) — Sway rewrites it to \";\" when \
             storing the value, so the value would not round-trip",
            character
        ));
    }
    Ok(value.to_string())
}

/// Validates the command a launch hands to Sway's `exec` — the positional
/// CLI command, and a layout step's (or a resolved template binding's)
/// `command` field.
///
/// The command is sent raw, not quoted (see `exec_sway_command()` for why),
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

    let mut in_double_quotes = false;
    let mut in_single_quotes = false;
    let mut escaped = false;
    for character in value.chars() {
        match character {
            '"' if !in_single_quotes && !escaped => in_double_quotes = !in_double_quotes,
            '\'' if !in_double_quotes && !escaped => in_single_quotes = !in_single_quotes,
            '\\' => {
                escaped = !escaped;
                continue;
            }
            ',' | ';' if !in_double_quotes && !in_single_quotes && !escaped => {
                return Err(format!(
                    "Must not contain an unquoted {:?} — Sway splits its commands on it, so the \
                     rest would run as a separate Sway command. Wrap the part containing it in \
                     quotes, or move the command into a script",
                    character
                ));
            }
            _ => {}
        }
        escaped = false;
    }

    Ok(value.to_string())
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

/// Rejects an empty or whitespace-only value for a named field — shared by
/// every layout/template schema field that's a free-form identifier or
/// required string (`id`, `target_id`, `slot`, a binding's `command`, a
/// template's `description`/`category`) rather than something with its own
/// dedicated format, the way `validate_size_argument`/
/// `validate_position_argument` above validate a specific value shape.
pub fn require_non_blank(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} must not be empty or whitespace-only"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
