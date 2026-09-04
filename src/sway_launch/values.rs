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
/// rather than carried as a string and re-parsed on every use (previously,
/// `SwayAction::poll_matches()`'s `Height`/`Width` arms each called
/// `parse_pixel_value()` on the raw string on every poll iteration).
/// `Display` renders the same `<n>px`/`<n>ppt` text `sway_command()` needs
/// to interpolate into the Sway command, and that `SwayAction::Display`
/// needs for its human-readable `Sway action: ...` line — this makes
/// `sway_command()` a pure serialization step for these variants, not a
/// second place the format is defined.
///
/// `i32`, not `u32`, deliberately: the geometry these values are ultimately
/// compared against is `swayipc::Rect`'s, which is `i32`. Carrying them as
/// `u32` meant `poll_matches()` had to cast (`*pixels as i32`), so a value
/// above `i32::MAX` — which `validate_size_argument()` used to accept, since
/// it only checked `u32` — wrapped negative and could never match anything
/// the compositor reported. Negative values never reach here regardless
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
/// `swayipc::Rect` geometry a pixel size is eventually compared against. It
/// used to be `u32`, which was the wrong invariant to promise: a value in
/// `i32::MAX+1..=u32::MAX` passed validation, was rendered into a real Sway
/// command, and then wrapped negative on its way into `width_matches()`/
/// `height_matches()`, so it could never be confirmed — the accepted domain
/// was larger than the domain the rest of the program could represent.
/// Rejecting it here is what keeps validation's promise ("anything that
/// passes is something the whole pipeline can handle") true, rather than only
/// true of the parser.
pub fn validate_size_argument(value: &str) -> Result<String, String> {
    let re = Regex::new(r"^(\d+)(?:px|ppt)$").unwrap();
    match re.captures(value) {
        Some(captures) if captures[1].parse::<i32>().is_ok() => Ok(value.to_string()),
        _ => Err("Must be in format <HEIGHT>px|ppt. E.g. 300px/20ppt. ppt = percent".to_string()),
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
/// live (2026-08-31) that Sway's own parser does *not* strip the escape
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
    // A newline is rewritten rather than stored: confirmed live (2026-09-01)
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
    if value.contains('\n') {
        return Err(
            "Must not contain a newline — Sway rewrites it to \";\" when storing the value, so \
             the value would not round-trip"
                .to_string(),
        );
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
/// it can only ever match nothing. It also used to produce a message that
/// contradicted the command line — an empty `--app-id` was indistinguishable
/// from an absent one after `unwrap_or_default()`, so `--existing --app-id ''`
/// reported "--existing requires --app-id, --class, or --mark-match" at a
/// caller who had just passed `--app-id`.
pub fn validate_non_blank_argument(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        return Err("Must not be empty or whitespace-only".to_string());
    }
    Ok(value.to_string())
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
mod tests {
    use super::*;

    // validate_size_argument / validate_position_argument

    #[test]
    fn validate_size_argument_accepts_px() {
        assert_eq!(validate_size_argument("300px"), Ok("300px".to_string()));
    }

    #[test]
    fn validate_size_argument_accepts_ppt() {
        assert_eq!(validate_size_argument("20ppt"), Ok("20ppt".to_string()));
    }

    #[test]
    fn validate_size_argument_accepts_zero() {
        assert_eq!(validate_size_argument("0px"), Ok("0px".to_string()));
    }

    #[test]
    fn validate_size_argument_rejects_missing_unit() {
        assert!(validate_size_argument("300").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_unknown_unit() {
        assert!(validate_size_argument("300pixels").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_negative() {
        assert!(validate_size_argument("-5px").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_decimal() {
        assert!(validate_size_argument("3.5px").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_empty() {
        assert!(validate_size_argument("").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_trailing_garbage() {
        assert!(validate_size_argument("300px ").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_a_value_that_overflows_i32() {
        // Regression test: the regex's \d+ has no digit-count bound, but
        // parse_size() trusts a validated value to parse infallibly --
        // matches the shape (all digits) while overflowing i32 must still be
        // rejected here.
        assert!(validate_size_argument("99999999999px").is_err());
    }

    #[test]
    fn validate_size_argument_accepts_the_largest_representable_pixel_value() {
        assert_eq!(
            validate_size_argument("2147483647px"),
            Ok("2147483647px".to_string())
        );
    }

    #[test]
    fn validate_size_argument_rejects_one_past_the_largest_representable_value() {
        // The old bound was u32, which accepted everything up to 4294967295
        // -- but Size holds an i32 (matching swayipc::Rect's geometry), so
        // anything above i32::MAX used to validate, reach Sway as a real
        // command, and then wrap negative before width_matches()/
        // height_matches() ever saw it, making it unconfirmable by
        // construction. Both of these were accepted before that was fixed.
        assert!(validate_size_argument("2147483648px").is_err());
        assert!(validate_size_argument("4294967295px").is_err());
    }

    #[test]
    fn validate_size_argument_applies_the_same_bound_to_ppt() {
        assert_eq!(
            validate_size_argument("2147483647ppt"),
            Ok("2147483647ppt".to_string())
        );
        assert!(validate_size_argument("4294967295ppt").is_err());
    }

    #[test]
    fn parse_size_round_trips_the_largest_representable_pixel_value() {
        // parse_size()'s expect() is only sound because validation rejects
        // anything it couldn't parse -- pin the boundary the two agree on.
        assert_eq!(parse_size("2147483647px"), Size::Pixels(i32::MAX));
        assert_eq!(parse_size("2147483647px").to_string(), "2147483647px");
    }

    #[test]
    fn validate_position_argument_accepts_center() {
        assert_eq!(
            validate_position_argument("center"),
            Ok("center".to_string())
        );
    }

    #[test]
    fn validate_position_argument_accepts_coordinates() {
        assert_eq!(
            validate_position_argument("100,200"),
            Ok("100,200".to_string())
        );
    }

    #[test]
    fn validate_position_argument_rejects_missing_y() {
        assert!(validate_position_argument("100").is_err());
    }

    #[test]
    fn validate_position_argument_accepts_negative_x() {
        assert_eq!(
            validate_position_argument("-1,200"),
            Ok("-1,200".to_string())
        );
    }

    #[test]
    fn validate_position_argument_accepts_negative_y() {
        assert_eq!(
            validate_position_argument("100,-200"),
            Ok("100,-200".to_string())
        );
    }

    #[test]
    fn validate_position_argument_accepts_negative_x_and_y() {
        assert_eq!(
            validate_position_argument("-1920,-200"),
            Ok("-1920,-200".to_string())
        );
    }

    #[test]
    fn validate_position_argument_rejects_bare_dash() {
        assert!(validate_position_argument("-,200").is_err());
    }

    #[test]
    fn validate_position_argument_rejects_a_coordinate_that_overflows_i32() {
        // Same reasoning as validate_size_argument's overflow test:
        // parse_position() trusts a validated value to parse infallibly, so
        // a coordinate that overflows i32 (max 2147483647, 10 digits) must
        // be rejected here, not discovered as a panic later.
        assert!(validate_position_argument("99999999999,200").is_err());
    }

    #[test]
    fn validate_position_argument_rejects_unknown_word() {
        assert!(validate_position_argument("middle").is_err());
    }

    #[test]
    fn validate_position_argument_rejects_empty() {
        assert!(validate_position_argument("").is_err());
    }

    // validate_sway_string_argument

    #[test]
    fn validate_sway_string_argument_accepts_an_ordinary_value() {
        assert_eq!(
            validate_sway_string_argument("dropdown-term"),
            Ok("dropdown-term".to_string())
        );
    }

    #[test]
    fn validate_sway_string_argument_accepts_command_separators() {
        // These are exactly what quote_sway_string() exists to neutralize,
        // and Sway does store them literally inside the quotes (confirmed
        // live) -- so they must stay accepted, unlike `"`/`\`/newline below.
        assert!(validate_sway_string_argument("foo, exec bar").is_ok());
        assert!(validate_sway_string_argument("foo; exec bar").is_ok());
    }

    #[test]
    fn validate_sway_string_argument_accepts_tab_and_carriage_return() {
        // Probed live against a real compositor: both round-trip
        // byte-for-byte through `mark`, unlike the newline below -- so
        // rejecting control characters as a class would over-reject.
        assert!(validate_sway_string_argument("foo\tbar").is_ok());
        assert!(validate_sway_string_argument("foo\rbar").is_ok());
    }

    #[test]
    fn validate_sway_string_argument_rejects_a_newline() {
        // Regression test: confirmed live that Sway rewrites a newline inside
        // a quoted value to ";" when storing it (`mark "a<LF>b"` is stored as
        // `a;b`), so --mark/--mark-match could never round-trip such a value
        // -- the same failure as `"`/`\`, missed when those were fixed because
        // the newline tests asserted safe storage rather than the round trip.
        let error = validate_sway_string_argument("foo\nexec bar")
            .expect_err("a newline should be rejected");
        assert!(
            error.contains("newline"),
            "error should name the offending character: {error:?}"
        );
    }

    #[test]
    fn validate_sway_string_argument_rejects_a_double_quote() {
        // Regression test: confirmed live that Sway stores `mark "a\"b"` as
        // the literal `a\"b`, so --mark/--mark-match could never round-trip
        // such a value. See this function's doc comment.
        let error = validate_sway_string_argument("dropdown\"term")
            .expect_err("a double quote should be rejected");
        assert!(
            error.contains('"'),
            "error should name the offending character: {error:?}"
        );
    }

    #[test]
    fn validate_sway_string_argument_rejects_a_backslash() {
        assert!(validate_sway_string_argument("back\\slash").is_err());
    }

    #[test]
    fn validate_sway_string_argument_rejects_empty() {
        assert!(validate_sway_string_argument("").is_err());
    }

    #[test]
    fn validate_sway_string_argument_rejects_whitespace_only() {
        assert!(validate_sway_string_argument("   ").is_err());
    }

    // validate_non_blank_argument

    #[test]
    fn validate_non_blank_argument_accepts_an_ordinary_value() {
        assert_eq!(validate_non_blank_argument("foot"), Ok("foot".to_string()));
    }

    #[test]
    fn validate_non_blank_argument_rejects_empty_and_whitespace_only() {
        assert!(validate_non_blank_argument("").is_err());
        assert!(validate_non_blank_argument("   ").is_err());
    }

    #[test]
    fn validate_non_blank_argument_accepts_quotes_and_backslashes() {
        // Unlike validate_sway_string_argument, these values are compared
        // client-side against a Node's app_id/class and never interpolated
        // into a Sway command, so there's no quoting round trip to break --
        // an XWayland WM_CLASS containing one is legitimate to match on.
        assert!(validate_non_blank_argument("weird\\class").is_ok());
        assert!(validate_non_blank_argument("weird\"class").is_ok());
    }

    // require_non_blank

    #[test]
    fn require_non_blank_accepts_a_real_value() {
        assert!(require_non_blank("field", "foot").is_ok());
    }

    #[test]
    fn require_non_blank_rejects_empty() {
        let error = require_non_blank("field", "").expect_err("empty value should be rejected");
        assert!(
            error.contains("field"),
            "error should name the field: {error:?}"
        );
    }

    #[test]
    fn require_non_blank_rejects_whitespace_only() {
        assert!(require_non_blank("field", "   ").is_err());
    }

    // Split

    #[test]
    fn split_display_v_is_vertical() {
        assert_eq!(Split::V.to_string(), "Vertical");
    }

    #[test]
    fn split_display_h_is_horizontal() {
        assert_eq!(Split::H.to_string(), "Horizontal");
    }

    // parse_size

    #[test]
    fn parse_size_parses_pixels() {
        assert_eq!(parse_size("300px"), Size::Pixels(300));
        assert_eq!(parse_size("0px"), Size::Pixels(0));
    }

    #[test]
    fn parse_size_parses_percent() {
        assert_eq!(parse_size("20ppt"), Size::Percent(20));
    }

    #[test]
    fn size_display_matches_the_format_parse_size_accepts() {
        assert_eq!(Size::Pixels(300).to_string(), "300px");
        assert_eq!(Size::Percent(20).to_string(), "20ppt");
    }

    #[test]
    fn parse_position_parses_center() {
        assert_eq!(parse_position("center"), Position::Center);
    }

    #[test]
    fn parse_position_parses_coordinates() {
        assert_eq!(
            parse_position("100,200"),
            Position::Coordinates { x: 100, y: 200 }
        );
    }

    #[test]
    fn parse_position_parses_negative_coordinates() {
        assert_eq!(
            parse_position("-1920,-200"),
            Position::Coordinates { x: -1920, y: -200 }
        );
    }

    #[test]
    fn position_display_matches_the_format_parse_position_accepts() {
        assert_eq!(Position::Center.to_string(), "center");
        assert_eq!(
            Position::Coordinates { x: 100, y: 200 }.to_string(),
            "100,200"
        );
    }
}
