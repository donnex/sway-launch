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
fn validate_size_argument_rejects_zero_for_both_units() {
    // Sway leaves a 0px size unchanged and doesn't produce a zero size from
    // 0ppt either, so both used to exit 0 as a silent no-op.
    assert!(validate_size_argument("0px").is_err());
    assert!(validate_size_argument("0ppt").is_err());
    assert!(validate_size_argument("000px").is_err());
}

#[test]
fn validate_size_argument_accepts_one_for_both_units() {
    assert_eq!(validate_size_argument("1px"), Ok("1px".to_string()));
    assert_eq!(validate_size_argument("1ppt"), Ok("1ppt".to_string()));
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
fn validate_size_argument_accepts_100_ppt() {
    assert_eq!(validate_size_argument("100ppt"), Ok("100ppt".to_string()));
}

#[test]
fn validate_size_argument_rejects_ppt_above_100() {
    // Sway clamps a percentage over 100, so a typo like 600ppt for 60ppt
    // used to exit 0 with a surprising size instead of an error.
    assert!(validate_size_argument("101ppt").is_err());
    assert!(validate_size_argument("600ppt").is_err());
    assert!(validate_size_argument("2147483647ppt").is_err());
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
    let error =
        validate_sway_string_argument("foo\nexec bar").expect_err("a newline should be rejected");
    assert!(
        error.contains("newline"),
        "error should name the offending character: {error:?}"
    );
}

#[test]
fn validate_sway_string_argument_rejects_a_nul() {
    // Regression test: Sway's IPC handler rewrites an embedded NUL to ";"
    // exactly like a newline, so `mark = "ab\u{0}cd"` (reachable via a
    // TOML escape) was stored as `ab;cd` and could never be matched again.
    let error = validate_sway_string_argument("ab\0cd").expect_err("a NUL should be rejected");
    assert!(error.contains("NUL"), "{error:?}");
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
fn validate_sway_string_argument_rejects_a_dollar_sign() {
    // Regression test: confirmed live that Sway substitutes a `set $name`
    // config variable even inside a quoted argument, so `--mark '$mymark'`
    // stored the variable's value and then timed out waiting for the literal.
    for value in ["$ws1", "my$mark", "$"] {
        let error = validate_sway_string_argument(value).expect_err("a $ should be rejected");
        assert!(error.contains('$'), "unexpected error: {}", error);
    }
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

// validate_exec_command

#[test]
fn validate_exec_command_accepts_an_ordinary_command() {
    assert_eq!(
        validate_exec_command("foot -T title"),
        Ok("foot -T title".to_string())
    );
}

#[test]
fn validate_exec_command_accepts_shell_syntax_without_separators() {
    assert!(validate_exec_command("cd ~/project && foot | tee log &").is_ok());
    assert!(validate_exec_command("( exec foot )").is_ok());
}

#[test]
fn validate_exec_command_rejects_an_unquoted_comma() {
    // Regression test: `foot -T a,b` launched `foot -T a`, then failed on
    // `b` as an unknown Sway command, leaving the window open.
    let error = validate_exec_command("foot -T a,b").expect_err("an unquoted , must fail");
    assert!(error.contains("','"), "{error:?}");
}

#[test]
fn validate_exec_command_rejects_an_unquoted_semicolon() {
    // Regression test: `foot -T x; mark INJECTED` ran `mark INJECTED`
    // against whatever window was focused.
    let error =
        validate_exec_command("foot -T x; mark INJECTED").expect_err("an unquoted ; must fail");
    assert!(error.contains("';'"), "{error:?}");
}

#[test]
fn validate_exec_command_accepts_separators_inside_quotes() {
    assert!(validate_exec_command("foot -T \"a,b; c\"").is_ok());
    assert!(validate_exec_command("foot -T 'a,b; c'").is_ok());
    assert!(validate_exec_command("sh -c 'if true; then foot; fi'").is_ok());
}

#[test]
fn validate_exec_command_accepts_a_backslash_escaped_separator() {
    // Sway's argsep() treats an escaped character as literal, and passes
    // the backslash on to the shell, which reads `\,` as `,`.
    assert!(validate_exec_command("foot -T a\\,b").is_ok());
    assert!(validate_exec_command("foot -T a\\;b").is_ok());
}

#[test]
fn validate_exec_command_rejects_a_separator_after_an_escaped_backslash() {
    // `\\` is an escaped backslash, so the comma after it is unescaped.
    assert!(validate_exec_command("foot -T a\\\\,b").is_err());
}

#[test]
fn validate_exec_command_does_not_let_the_other_quote_close_a_quoted_span() {
    // An apostrophe inside double quotes is literal, so the comma after
    // it is still inside the double-quoted span.
    assert!(validate_exec_command("foot -T \"it's,fine\"").is_ok());
    // ...and an escaped quote doesn't close the span either.
    assert!(validate_exec_command("foot -T \"a\\\",b\"").is_ok());
}

#[test]
fn validate_exec_command_rejects_a_separator_after_a_closed_quote() {
    assert!(validate_exec_command("foot -T 'a' , mark x").is_err());
}

#[test]
fn validate_exec_command_rejects_a_newline_even_inside_quotes() {
    // Sway rewrites a newline to ";" before its quote handling runs, so
    // quoting can't protect one: `foot -T "q1<LF>q2"` got the title q1;q2.
    assert!(validate_exec_command("foot -T nl\nmark NLINJ").is_err());
    assert!(validate_exec_command("foot -T \"q1\nq2\"").is_err());
}

#[test]
fn validate_exec_command_rejects_a_carriage_return_and_a_nul() {
    assert!(validate_exec_command("foot\r").is_err());
    // Reachable from TOML, never from argv: a NUL is rewritten to ";"
    // like a newline, and ran `mark NULINJ` with a zero exit status.
    assert!(validate_exec_command("foot -T nulcmd\0mark NULINJ").is_err());
}

// validate_timeout_seconds / validate_timeout_argument

#[test]
fn validate_timeout_seconds_rejects_zero() {
    // 0 can never succeed: the command is sent before the wait begins, so a
    // zero timeout reported failure over a command that had already applied.
    assert!(validate_timeout_seconds(0).is_err());
}

#[test]
fn validate_timeout_seconds_accepts_one_and_the_maximum() {
    assert_eq!(validate_timeout_seconds(1), Ok(1));
    assert_eq!(
        validate_timeout_seconds(MAX_TIMEOUT_SECONDS),
        Ok(MAX_TIMEOUT_SECONDS)
    );
}

#[test]
fn validate_timeout_seconds_rejects_one_past_the_maximum_and_u64_max() {
    assert!(validate_timeout_seconds(MAX_TIMEOUT_SECONDS + 1).is_err());
    assert!(validate_timeout_seconds(u64::MAX).is_err());
}

#[test]
fn every_accepted_timeout_is_a_representable_deadline() {
    // The bound's whole reason to exist: anything that passes must be
    // something Instant::now() + timeout can actually hold.
    assert!(std::time::Instant::now()
        .checked_add(std::time::Duration::from_secs(MAX_TIMEOUT_SECONDS))
        .is_some());
}

#[test]
fn validate_timeout_argument_parses_and_bounds_the_value() {
    assert_eq!(validate_timeout_argument("5"), Ok(5));
    assert!(validate_timeout_argument("0").is_err());
    assert!(validate_timeout_argument("18446744073709551615").is_err());
    assert!(validate_timeout_argument("-1").is_err());
    assert!(validate_timeout_argument("five").is_err());
}
