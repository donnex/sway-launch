use super::*;

// parse_size / parse_position

#[test]
fn parse_size_accepts_px() {
    assert_eq!(parse_size("300px"), Ok(Size::Pixels(300)));
}

#[test]
fn parse_size_accepts_ppt() {
    assert_eq!(parse_size("20ppt"), Ok(Size::Percent(20)));
}

#[test]
fn parse_size_rejects_zero_for_both_units() {
    // Sway leaves a 0px size unchanged and doesn't produce a zero size from
    // 0ppt either, so both used to exit 0 as a silent no-op.
    assert!(parse_size("0px").is_err());
    assert!(parse_size("0ppt").is_err());
    assert!(parse_size("000px").is_err());
}

#[test]
fn parse_size_accepts_one_for_both_units() {
    assert_eq!(parse_size("1px"), Ok(Size::Pixels(1)));
    assert_eq!(parse_size("1ppt"), Ok(Size::Percent(1)));
}

#[test]
fn parse_size_rejects_missing_unit() {
    assert!(parse_size("300").is_err());
}

#[test]
fn parse_size_rejects_unknown_unit() {
    assert!(parse_size("300pixels").is_err());
}

#[test]
fn parse_size_rejects_negative() {
    assert!(parse_size("-5px").is_err());
}

#[test]
fn parse_size_rejects_decimal() {
    assert!(parse_size("3.5px").is_err());
}

#[test]
fn parse_size_rejects_empty() {
    assert!(parse_size("").is_err());
}

#[test]
fn parse_size_rejects_trailing_garbage() {
    assert!(parse_size("300px ").is_err());
}

#[test]
fn parse_size_rejects_a_value_that_overflows_i32() {
    // Regression test: a value that matches the shape (all digits) while
    // overflowing i32 must be rejected, not wrapped or panicked on.
    assert!(parse_size("99999999999px").is_err());
}

#[test]
fn parse_size_accepts_the_largest_representable_pixel_value() {
    assert_eq!(parse_size("2147483647px"), Ok(Size::Pixels(2147483647)));
}

#[test]
fn parse_size_rejects_one_past_the_largest_representable_value() {
    // The old bound was u32, which accepted everything up to 4294967295
    // -- but Size holds an i32 (matching swayipc::Rect's geometry), so
    // anything above i32::MAX used to validate, reach Sway as a real
    // command, and then wrap negative before width_matches()/
    // height_matches() ever saw it, making it unconfirmable by
    // construction. Both of these were accepted before that was fixed.
    assert!(parse_size("2147483648px").is_err());
    assert!(parse_size("4294967295px").is_err());
}

#[test]
fn parse_size_accepts_100_ppt() {
    assert_eq!(parse_size("100ppt"), Ok(Size::Percent(100)));
}

#[test]
fn parse_size_rejects_ppt_above_100() {
    // Sway clamps a percentage over 100, so a typo like 600ppt for 60ppt
    // used to exit 0 with a surprising size instead of an error.
    assert!(parse_size("101ppt").is_err());
    assert!(parse_size("600ppt").is_err());
    assert!(parse_size("2147483647ppt").is_err());
    assert!(parse_size("4294967295ppt").is_err());
}

#[test]
fn parse_size_rejects_a_leading_plus_sign() {
    // str::parse::<i32>() accepts "+5", but the format is digits only.
    assert!(parse_size("+5px").is_err());
    assert!(parse_size("+5ppt").is_err());
}

#[test]
fn parse_size_rejects_non_ascii_digits() {
    assert!(parse_size("\u{0663}px").is_err());
}

#[test]
fn size_display_round_trips_through_parse_size() {
    for size in [Size::Pixels(300), Size::Pixels(i32::MAX), Size::Percent(20)] {
        assert_eq!(parse_size(&size.to_string()), Ok(size));
    }
}

#[test]
fn parse_position_accepts_center() {
    assert_eq!(parse_position("center"), Ok(Position::Center));
}

#[test]
fn parse_position_accepts_coordinates() {
    assert_eq!(
        parse_position("100,200"),
        Ok(Position::Coordinates { x: 100, y: 200 })
    );
}

#[test]
fn parse_position_rejects_missing_y() {
    assert!(parse_position("100").is_err());
}

#[test]
fn parse_position_accepts_negative_x() {
    assert_eq!(
        parse_position("-1,200"),
        Ok(Position::Coordinates { x: -1, y: 200 })
    );
}

#[test]
fn parse_position_accepts_negative_y() {
    assert_eq!(
        parse_position("100,-200"),
        Ok(Position::Coordinates { x: 100, y: -200 })
    );
}

#[test]
fn parse_position_accepts_negative_x_and_y() {
    assert_eq!(
        parse_position("-1920,-200"),
        Ok(Position::Coordinates { x: -1920, y: -200 })
    );
}

#[test]
fn parse_position_rejects_bare_dash() {
    assert!(parse_position("-,200").is_err());
}

#[test]
fn parse_position_rejects_a_coordinate_that_overflows_i32() {
    // Same reasoning as parse_size's overflow test: a coordinate that
    // overflows i32 (max 2147483647, 10 digits) must be rejected.
    assert!(parse_position("99999999999,200").is_err());
}

#[test]
fn parse_position_rejects_unknown_word() {
    assert!(parse_position("middle").is_err());
}

#[test]
fn parse_position_rejects_empty() {
    assert!(parse_position("").is_err());
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
    // live) -- so they must stay accepted, unlike `"`/`\`/control characters below.
    assert!(validate_sway_string_argument("foo, exec bar").is_ok());
    assert!(validate_sway_string_argument("foo; exec bar").is_ok());
}

#[test]
fn validate_sway_string_argument_accepts_a_tab() {
    assert!(validate_sway_string_argument("foo\tbar").is_ok());
}

#[test]
fn validate_sway_string_argument_rejects_other_control_characters() {
    // A template carries no commands and is meant to be shared, but its
    // names reach status bars and `swaymsg` output, so an escape sequence
    // there would be printed raw to whatever terminal displays them.
    for value in ["foo\rbar", "\u{1b}[31mred", "a\u{7f}b", "a\u{9b}b"] {
        let error = validate_sway_string_argument(value)
            .expect_err("a control character other than tab should be rejected");
        assert!(error.contains("control character"), "{value:?}: {error:?}");
    }
}

// escape_control_characters

#[test]
fn escape_control_characters_renders_each_control_character_visibly() {
    assert_eq!(
        escape_control_characters("foo\r\u{1b}[2Kbar\n\t\0", false),
        "foo\\r\\u{1b}[2Kbar\\n\\t\\0"
    );
}

#[test]
fn escape_control_characters_leaves_ordinary_text_untouched() {
    let text = "foot -T \"a,b\" 'x' \\ ünïcode";
    assert_eq!(escape_control_characters(text, false), text);
}

#[test]
fn escape_control_characters_can_keep_newlines_as_line_breaks() {
    assert_eq!(
        escape_control_characters("line one\n# \u{1b}]0;x\u{7}\nline three", true),
        "line one\n# \\u{1b}]0;x\\u{7}\nline three"
    );
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
fn parse_position_rejects_a_leading_plus_sign() {
    assert!(parse_position("+1,200").is_err());
    assert!(parse_position("1,+200").is_err());
}

#[test]
fn parse_position_rejects_a_double_minus_or_extra_coordinate() {
    assert!(parse_position("--1,200").is_err());
    assert!(parse_position("1,2,3").is_err());
    assert!(parse_position("1,").is_err());
    assert!(parse_position(",2").is_err());
}

#[test]
fn position_display_round_trips_through_parse_position() {
    for position in [
        Position::Center,
        Position::Coordinates { x: 100, y: 200 },
        Position::Coordinates { x: -1920, y: -200 },
    ] {
        assert_eq!(parse_position(&position.to_string()), Ok(position));
    }
}

#[test]
fn split_display_h_is_horizontal() {
    assert_eq!(Split::H.to_string(), "Horizontal");
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

// validate_workspace_argument / validate_output_argument

#[test]
fn validate_workspace_argument_rejects_every_sway_keyword_in_any_case() {
    for keyword in [
        "next",
        "NEXT",
        "Prev",
        "next_on_output",
        "PREV_ON_OUTPUT",
        "current",
        "back_and_forth",
        "number",
        "to",
    ] {
        let error = validate_workspace_argument(keyword).unwrap_err();
        assert!(
            error.contains("is a Sway keyword") && error.contains("swaymsg"),
            "{keyword}: {error}"
        );
    }
}

#[test]
fn validate_workspace_argument_accepts_names_that_only_contain_a_keyword() {
    for name in ["3", "web", "number 2", "next-up", "nextcloud"] {
        assert_eq!(validate_workspace_argument(name), Ok(name.to_string()));
    }
}

#[test]
fn validate_workspace_argument_still_applies_the_shared_string_rules() {
    assert!(validate_workspace_argument("$ws1").is_err());
    assert!(validate_workspace_argument("  ").is_err());
}

#[test]
fn validate_output_argument_rejects_directions_and_current_in_any_case() {
    for keyword in ["left", "RIGHT", "Up", "down", "current"] {
        assert!(
            validate_output_argument(keyword)
                .unwrap_err()
                .contains("is a Sway keyword"),
            "{keyword}"
        );
    }
    assert_eq!(
        validate_output_argument("HDMI-A-1"),
        Ok("HDMI-A-1".to_string())
    );
    assert!(validate_output_argument("a\"b").is_err());
}

// render_argv

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|word| word.to_string()).collect()
}

#[test]
fn render_argv_single_quotes_every_word() {
    assert_eq!(
        render_argv(&words(&["foot", "-T", "notes, todo; misc"])).unwrap(),
        "'foot' '-T' 'notes, todo; misc'"
    );
    assert_eq!(render_argv(&words(&["app", ""])).unwrap(), "'app' ''");
}

#[test]
fn render_argv_writes_quotes_and_backslashes_outside_the_quotes() {
    assert_eq!(
        render_argv(&words(&["printf", "it's", "a\\"])).unwrap(),
        "'printf' 'it'\\''s' 'a'\\\\''"
    );
}

#[test]
fn render_argv_output_passes_the_sway_separator_check() {
    let rendered = render_argv(&words(&["sh", "a\\", "b,c", "it's; ok"])).unwrap();
    assert!(validate_exec_command(&rendered).is_ok(), "{rendered}");
}

#[test]
fn render_argv_rejects_what_sway_reads_first_and_a_missing_program() {
    for bad in [
        &["echo", "$HOME"][..],
        &["echo", "a\nb"],
        &["echo", "a\rb"],
        &["echo", "a\0b"],
        &[],
        &["  ", "x"],
    ] {
        assert!(
            render_argv(&words(bad)).is_err(),
            "{bad:?} should be rejected"
        );
    }
}

// parse_env_assignment / launch_setup

#[test]
fn parse_env_assignment_splits_at_the_first_equals_and_checks_the_name() {
    assert_eq!(
        parse_env_assignment("GDK_BACKEND=x11").unwrap(),
        ("GDK_BACKEND".to_string(), "x11".to_string())
    );
    assert_eq!(
        parse_env_assignment("A=b=c").unwrap(),
        ("A".to_string(), "b=c".to_string())
    );
    for bad in ["NOEQUALS", "1A=x", "A-B=x", "=x", "A=$HOME"] {
        assert!(
            parse_env_assignment(bad).is_err(),
            "{bad:?} should be rejected"
        );
    }
}

#[test]
fn launch_setup_quotes_the_directory_and_each_variable() {
    let setup = launch_setup(
        true,
        Some("/"),
        &[("A".to_string(), "it's, ok".to_string())],
    )
    .unwrap();
    assert_eq!(setup.as_deref(), Some("cd '/' && export A='it'\\''s, ok'"));
    assert_eq!(launch_setup(true, None, &[]).unwrap(), None);
}

#[test]
fn launch_setup_refuses_settings_without_a_launch_and_a_missing_directory() {
    assert!(launch_setup(false, Some("/"), &[]).is_err());
    assert!(launch_setup(false, None, &[("A".to_string(), "1".to_string())]).is_err());
    let error = launch_setup(true, Some("/nonexistent/sway-launch-test"), &[]).unwrap_err();
    assert!(error.contains("is not a directory"), "{error}");
}

#[test]
fn resolve_launch_directory_expands_home_and_keeps_absolute_paths() {
    let home = std::env::var("HOME").expect("HOME should be set for the tests");
    assert_eq!(
        resolve_launch_directory("~").unwrap(),
        std::path::PathBuf::from(&home)
    );
    assert_eq!(
        resolve_launch_directory("/").unwrap(),
        std::path::PathBuf::from("/")
    );
}
