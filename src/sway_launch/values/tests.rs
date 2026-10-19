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
fn escape_control_characters_renders_each_bidi_formatting_character_visibly() {
    // Regression test (#182): these are Unicode category Cf, not Cc, so
    // `char::is_control` passed them through, and a terminal implementing
    // bidi reordered the line around them.
    for (first, last) in [
        ('\u{061c}', '\u{061c}'),
        ('\u{200e}', '\u{200f}'),
        ('\u{202a}', '\u{202e}'),
        ('\u{2066}', '\u{2069}'),
    ] {
        for character in first..=last {
            let text = format!("a{character}b");
            let expected = format!("a\\u{{{:x}}}b", u32::from(character));
            assert_eq!(escape_control_characters(&text, false), expected);
            assert_eq!(escape_control_characters(&text, true), expected);
        }
    }
}

#[test]
fn escape_control_characters_leaves_neighbouring_format_characters_untouched() {
    for character in ['\u{200d}', '\u{2060}', '\u{2065}', '\u{206a}', '\u{2029}'] {
        let text = format!("a{character}b");
        assert_eq!(
            escape_control_characters(&text, false),
            text,
            "{character:?}"
        );
    }
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
fn validate_exec_command_rejects_a_shell_comment() {
    // The command runs inside ( ... ), whose ) a comment would swallow.
    for command in ["foot # terminal", "#foot", "foot&#x", "foot;#x"] {
        assert!(validate_exec_command(command).is_err(), "{command:?}");
    }
    let error = validate_exec_command("foot # terminal").expect_err("a comment must fail");
    assert!(error.contains("comment"), "{error:?}");
}

#[test]
fn validate_exec_command_accepts_a_hash_that_starts_no_comment() {
    for command in [
        "foot -T a#b",
        "foot -T '#1'",
        "foot -T \"# x\"",
        "foot -T \\#1",
        "xdg-open https://example.com/#top",
    ] {
        assert!(validate_exec_command(command).is_ok(), "{command:?}");
    }
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
        DirectoryCheck::Exists,
    )
    .unwrap();
    assert_eq!(setup.as_deref(), Some("cd '/' && export A='it'\\''s, ok'"));
    assert_eq!(
        launch_setup(true, None, &[], DirectoryCheck::Exists).unwrap(),
        None
    );
}

#[test]
fn launch_setup_refuses_settings_without_a_launch_and_a_missing_directory() {
    let check = DirectoryCheck::Exists;
    assert!(launch_setup(false, Some("/"), &[], check).is_err());
    assert!(launch_setup(false, None, &[("A".to_string(), "1".to_string())], check).is_err());
    let error = launch_setup(true, Some("/nonexistent/sway-launch-test"), &[], check).unwrap_err();
    assert!(error.contains("is not a directory"), "{error}");
}

#[test]
fn launch_setup_checks_only_the_format_of_a_directory_offline() {
    // #146: --validate/--dry-run check a file meant for another machine too.
    let check = DirectoryCheck::FormatOnly;
    let setup = launch_setup(true, Some("/nonexistent/sway-launch-test"), &[], check).unwrap();
    assert_eq!(setup.as_deref(), Some("cd '/nonexistent/sway-launch-test'"));
    assert!(launch_setup(false, Some("/nonexistent"), &[], check).is_err());
    let error = launch_setup(true, Some("/nonexistent/$HOME"), &[], check).unwrap_err();
    assert!(error.contains("config variable"), "{error}");
}

#[test]
fn directory_check_for_offline_skips_the_existence_check_only_offline() {
    assert_eq!(
        DirectoryCheck::for_offline(true),
        DirectoryCheck::FormatOnly
    );
    assert_eq!(DirectoryCheck::for_offline(false), DirectoryCheck::Exists);
}

#[test]
fn resolve_launch_directory_expands_home_and_keeps_absolute_paths() {
    let home = std::env::var("HOME").expect("HOME should be set for the tests");
    assert_eq!(
        resolve_launch_directory("~", DirectoryCheck::Exists).unwrap(),
        std::path::PathBuf::from(&home)
    );
    assert_eq!(
        resolve_launch_directory("/", DirectoryCheck::Exists).unwrap(),
        std::path::PathBuf::from("/")
    );
}

#[test]
fn anchor_launch_directory_joins_only_a_relative_path_onto_the_file_s_directory() {
    // #176: a file's relative cwd is the file's, whoever runs it; ~ and
    // absolute paths already say where they are.
    let file_directory = Path::new("/home/example-user/projects/site");
    assert_eq!(
        anchor_launch_directory("src", file_directory),
        Ok("/home/example-user/projects/site/src".to_string())
    );
    assert_eq!(
        anchor_launch_directory("../other", file_directory),
        Ok("/home/example-user/projects/site/../other".to_string())
    );
    // ~user isn't the home-directory form, so it's an ordinary name here,
    // as resolve_launch_directory() treats it.
    assert_eq!(
        anchor_launch_directory("~example-user", file_directory),
        Ok("/home/example-user/projects/site/~example-user".to_string())
    );
    for unchanged in ["~", "~/projects", "/srv/site"] {
        assert_eq!(
            anchor_launch_directory(unchanged, file_directory),
            Ok(unchanged.to_string())
        );
    }
}

#[test]
fn anchor_launch_directory_refuses_a_file_directory_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let file_directory = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff"));
    let error = anchor_launch_directory("src", file_directory).unwrap_err();
    assert!(error.contains("is not valid UTF-8"), "{error}");
}

#[test]
fn parse_workspace_number_accepts_a_whole_number_from_zero() {
    assert_eq!(parse_workspace_number("2"), Ok(2));
    assert_eq!(parse_workspace_number("0"), Ok(0));
    assert_eq!(parse_workspace_number("2147483647"), Ok(i32::MAX));
}

#[test]
fn parse_workspace_number_rejects_anything_else() {
    // -1 is the number Sway gives every workspace whose name doesn't start
    // with a digit, so it would match all of them.
    for value in ["-1", "2147483648", "two", "2: web", "", "1.5"] {
        assert!(
            parse_workspace_number(value).is_err(),
            "{value:?} should be rejected"
        );
    }
}

#[test]
fn workspace_choice_names_the_workspace_by_either_half() {
    assert_eq!(workspace_choice(None, None), Ok(None));
    assert_eq!(
        workspace_choice(Some("web"), None),
        Ok(Some(WorkspaceTarget::Name("web")))
    );
    assert_eq!(
        workspace_choice(None, Some(3)),
        Ok(Some(WorkspaceTarget::Number(3)))
    );
}

#[test]
fn workspace_choice_rejects_both_halves_and_names_the_field_at_fault() {
    let both = workspace_choice(Some("web"), Some(3)).unwrap_err();
    assert!(both.starts_with("workspace and workspace_number"), "{both}");
    let name = workspace_choice(Some("next"), None).unwrap_err();
    assert!(name.starts_with("workspace: "), "{name}");
    for number in [-1, i64::from(i32::MAX) + 1] {
        let error = workspace_choice(None, Some(number)).unwrap_err();
        assert!(error.starts_with("workspace_number: "), "{error}");
    }
}

#[test]
fn workspace_target_from_pair_prefers_the_name() {
    assert_eq!(WorkspaceTarget::from_pair(None, None), None);
    assert_eq!(
        WorkspaceTarget::from_pair(None, Some(2)),
        Some(WorkspaceTarget::Number(2))
    );
    assert_eq!(
        WorkspaceTarget::from_pair(Some("web"), Some(2)),
        Some(WorkspaceTarget::Name("web"))
    );
}

#[test]
fn check_relative_move_refuses_a_workspace_move_after_new_column_or_new_row() {
    assert!(check_relative_move(false, true, true).is_ok());
    assert!(check_relative_move(true, false, false).is_ok());
    assert!(check_relative_move(true, true, false).is_err());
    assert!(check_relative_move(true, false, true).is_err());
}

#[test]
fn workspace_target_matches_a_name_ignoring_case_and_a_number_by_num() {
    assert!(WorkspaceTarget::Name("web").matches(Some("Web"), Some(-1)));
    assert!(!WorkspaceTarget::Name("2").matches(Some("2: web"), Some(2)));
    assert!(WorkspaceTarget::Number(2).matches(Some("2: web"), Some(2)));
    assert!(!WorkspaceTarget::Number(2).matches(Some("12"), Some(12)));
    assert!(!WorkspaceTarget::Number(2).matches(None, None));
}

#[test]
fn sway_workspace_number_reads_the_leading_digits_as_sway_does() {
    assert_eq!(sway_workspace_number("3"), Some(3));
    assert_eq!(sway_workspace_number("3: web"), Some(3));
    assert_eq!(sway_workspace_number("12web"), Some(12));
    assert_eq!(sway_workspace_number("web 3"), None);
    assert_eq!(sway_workspace_number(" 3"), None);
    assert_eq!(sway_workspace_number("-3"), None);
    assert_eq!(sway_workspace_number(""), None);
    assert_eq!(sway_workspace_number("99999999999"), None);
}

#[test]
fn workspace_target_run_lock_key_agrees_across_spellings_of_one_workspace() {
    let key = |target: WorkspaceTarget| target.run_lock_key();
    assert_eq!(
        key(WorkspaceTarget::Name("Dev")),
        key(WorkspaceTarget::Name("dev"))
    );
    assert_eq!(
        key(WorkspaceTarget::Number(3)),
        key(WorkspaceTarget::Name("3: web"))
    );
    assert_eq!(
        key(WorkspaceTarget::Name("3")),
        key(WorkspaceTarget::Name("3: <icons>"))
    );
    assert_ne!(
        key(WorkspaceTarget::Name("dev")),
        key(WorkspaceTarget::Name("web"))
    );
    assert_ne!(
        key(WorkspaceTarget::Number(3)),
        key(WorkspaceTarget::Number(4))
    );
    assert_ne!(
        key(WorkspaceTarget::Name("number 3")),
        key(WorkspaceTarget::Number(3))
    );
}

#[test]
fn workspace_target_quotes_a_name_but_not_a_number() {
    assert_eq!(
        WorkspaceTarget::Name("2: web").sway_argument(),
        "\"2: web\""
    );
    assert_eq!(WorkspaceTarget::Number(2).sway_argument(), "number 2");
    assert_eq!(WorkspaceTarget::Number(2).to_string(), "number 2");
    assert_eq!(WorkspaceTarget::Name("web").to_string(), "web");
}

#[test]
fn parse_title_pattern_compiles_an_unanchored_regex() {
    let pattern = parse_title_pattern("project-a").expect("a plain word is a valid pattern");
    assert!(pattern.matches(Some("project-a — Visual Studio Code")));
    assert!(!pattern.matches(Some("project-b — Visual Studio Code")));
    assert!(!pattern.matches(None));
    let anchored = parse_title_pattern("^Mozilla").expect("anchors are valid");
    assert!(!anchored.matches(Some("Sway — Mozilla Firefox")));
    assert_eq!(anchored.to_string(), "title matching \"^Mozilla\"");
}

#[test]
fn parse_title_pattern_rejects_a_blank_or_invalid_pattern() {
    assert!(parse_title_pattern("  ").is_err());
    let error = parse_title_pattern("(unclosed").unwrap_err();
    assert!(
        error.starts_with("\"(unclosed\" is not a valid regular expression"),
        "{error}"
    );
}

// validate_settle_milliseconds / validate_settle_argument / settle_duration

#[test]
fn validate_settle_milliseconds_accepts_one_to_a_minute() {
    assert_eq!(validate_settle_milliseconds(1), Ok(1));
    assert_eq!(
        validate_settle_milliseconds(MAX_SETTLE_MILLISECONDS),
        Ok(MAX_SETTLE_MILLISECONDS)
    );
    assert!(validate_settle_milliseconds(0).is_err());
    assert!(validate_settle_milliseconds(MAX_SETTLE_MILLISECONDS + 1).is_err());
}

#[test]
fn validate_settle_argument_rejects_what_is_not_a_whole_number() {
    assert_eq!(validate_settle_argument("300"), Ok(300));
    assert_eq!(
        validate_settle_argument("0.5"),
        Err("Must be a whole number of ms between 1 and 60000".to_string())
    );
}

#[test]
fn settle_duration_needs_pixel_geometry_or_a_position() {
    let pixels = Some(Size::Pixels(500));
    let percent = Some(Size::Percent(50));
    assert_eq!(settle_duration(None, None, None, None), Ok(None));
    assert_eq!(
        settle_duration(Some(300), pixels, None, None),
        Ok(Some(std::time::Duration::from_millis(300)))
    );
    assert!(settle_duration(Some(300), None, pixels, None).is_ok());
    assert!(settle_duration(Some(300), None, None, Some(Position::Center)).is_ok());
    let missing = Err("needs a height or width in px, or a position, to hold".to_string());
    assert_eq!(settle_duration(Some(300), None, None, None), missing);
    assert_eq!(settle_duration(Some(300), percent, percent, None), missing);
    assert!(settle_duration(Some(0), pixels, None, None).is_err());
}

// validate_sway_command

#[test]
fn validate_sway_command_accepts_a_window_command_and_trims_it() {
    assert_eq!(
        validate_sway_command("  border none "),
        Ok("border none".to_string())
    );
    assert!(validate_sway_command("opacity 0.9").is_ok());
    assert!(validate_sway_command("title_format \"%title, %app_id\"").is_ok());
    assert!(validate_sway_command("BORDER pixel 2").is_ok());
}

#[test]
fn validate_sway_command_rejects_a_command_that_acts_on_the_session() {
    // Each of these ran behind [con_id=N] and took effect, `exit` ending
    // the session (confirmed live on Sway 1.11).
    for command in [
        "exit",
        "exec foot",
        "reload",
        "bindsym Mod4+q kill",
        "output * bg #000000 solid_color",
        "workspace 2",
        "kill",
    ] {
        let error = validate_sway_command(command).expect_err(command);
        assert!(error.contains("acts on one window"), "{command}: {error:?}");
    }
}

#[test]
fn validate_sway_command_rejects_a_second_command_in_one_entry() {
    assert!(validate_sway_command("border none, exec foot").is_err());
    assert!(validate_sway_command("border none; exit").is_err());
    assert!(validate_sway_command("border none\nexit").is_err());
    assert!(validate_sway_command("border none\0exit").is_err());
}

#[test]
fn validate_sway_command_rejects_criteria_variables_and_blank() {
    let criteria = validate_sway_command("[app_id=foot] border none").expect_err("criteria");
    assert!(criteria.contains("criteria"), "{criteria:?}");
    let variable = validate_sway_command("opacity $alpha").expect_err("variable");
    assert!(variable.contains("config variables"), "{variable:?}");
    assert!(validate_sway_command("   ").is_err());
}

#[test]
fn validate_sway_command_rejects_subcommands_that_reach_past_the_window() {
    // #154: each acts on another window, a whole workspace, or focus.
    for command in [
        "move workspace to output HDMI-A-1",
        "MOVE Workspace to output HDMI-A-1",
        "move workspace 2",
        "swap container with con_id 1",
        "swap container with mark other",
        "focus output HDMI-A-1",
        "focus left",
        "focus parent",
        "Focus mode_toggle",
    ] {
        let error = validate_sway_command(command).expect_err(command);
        assert!(error.contains("acts on one window"), "{command}: {error:?}");
    }
}

#[test]
fn validate_sway_command_keeps_the_forms_that_act_on_the_window_itself() {
    for command in [
        "focus",
        "move container to workspace 2",
        "move to output HDMI-A-1",
        "move left",
        "move scratchpad",
        "inhibit_idle open",
        "shortcuts_inhibitor enable",
    ] {
        assert!(validate_sway_command(command).is_ok(), "{command}");
    }
}

#[test]
fn validate_shared_sway_command_refuses_the_command_line_only_commands() {
    // #154: a shared file could otherwise keep the screen from locking.
    for command in [
        "inhibit_idle open",
        "Inhibit_Idle visible",
        "shortcuts_inhibitor enable",
    ] {
        let error = validate_shared_sway_command(command).expect_err(command);
        assert!(error.contains("--sway-command"), "{command}: {error:?}");
    }
    assert_eq!(
        validate_shared_sway_command(" border none "),
        Ok("border none".to_string())
    );
    assert!(validate_shared_sway_command("exit").is_err());
    assert!(validate_shared_sway_command("focus left").is_err());
}

#[test]
fn sway_cli_only_commands_are_all_window_commands() {
    for command in SWAY_CLI_ONLY_COMMANDS {
        assert!(SWAY_WINDOW_COMMANDS.contains(command), "{command}");
    }
}

#[test]
fn sway_window_commands_is_sorted_and_unique() {
    let mut sorted = SWAY_WINDOW_COMMANDS.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, SWAY_WINDOW_COMMANDS);
}

#[test]
fn validate_sway_command_rejects_an_entry_ending_inside_quoting() {
    // #123: joined with the next entry, the open quote would turn a `,`
    // quoted there into a separator and run what follows it.
    for (command, described) in [
        ("title_format \"a", "unclosed '\"' quote"),
        ("title_format 'a", "unclosed \"'\" quote"),
        ("title_format a\\", "unpaired trailing '\\'"),
    ] {
        let error = validate_sway_command(command).expect_err(command);
        assert!(error.contains(described), "{command}: {error:?}");
    }
    assert!(validate_sway_command("title_format a\\\\").is_ok());
    assert!(validate_sway_command("title_format \"it's\"").is_ok());
}

#[test]
fn validate_exec_command_rejects_a_command_ending_inside_quoting() {
    for command in ["foot -T \"a", "foot -T 'a", "foot -T a\\"] {
        let error = validate_exec_command(command).expect_err(command);
        assert!(error.contains("Must not end in"), "{command}: {error:?}");
    }
    assert!(validate_exec_command("foot -T 'a\\\\'").is_ok());
}

#[test]
fn validate_exec_command_rejects_an_escaped_quote_inside_single_quotes() {
    // #151: Sway reads `\'` inside '...' as an escaped quote, the shell as
    // the closing one, so the shell would read the first `#x'` as a comment
    // swallowing the subshell's `)`, while the scan saw it quoted.
    for command in [
        "foot -T 'a\\' #x'",
        "foot -T 'a\\', b'",
        "foot -T 'a\\\\\\' #x'",
    ] {
        let error = validate_exec_command(command).expect_err(command);
        assert!(
            error.contains("inside single quotes"),
            "{command}: {error:?}"
        );
    }
    for command in [
        "foot -T 'a'\\''b'",
        "foot -T \"a\\'b\"",
        "foot -T 'a\\b'",
        "foot -T 'a\\\\' b",
    ] {
        assert!(validate_exec_command(command).is_ok(), "{command:?}");
    }
    // Still an unclosed quote to Sway, reported as that.
    let error = validate_exec_command("foot -T 'a\\' #x").expect_err("open quote");
    assert!(error.contains("Must not end in"), "{error:?}");
}

// sway_command_split_problem

#[test]
fn sway_command_split_problem_accepts_what_splits_as_built() {
    assert_eq!(
        sway_command_split_problem("[con_id=1] border none", 1),
        None
    );
    assert_eq!(
        sway_command_split_problem("[con_id=1] mark \"a, b\", border none", 2),
        None
    );
    assert_eq!(
        sway_command_split_problem("exec export X=1 && ( foot -T 'a;b' )", 1),
        None
    );
}

#[test]
fn sway_command_split_problem_refuses_a_quote_split_across_two_entries() {
    // The #123 template: each entry is balanced on its own but the first,
    // so the joined command splits into four, `exec` among them.
    let joined = "[con_id=1] title_format \"a, border \", exec touch /tmp/x, border \"";
    let error = sway_command_split_problem(joined, 2).expect("must be refused");
    assert!(error.contains("unclosed '\"' quote"), "{error:?}");

    let balanced_at_the_end =
        "[con_id=1] title_format \"a, border \", exec touch /tmp/x, border none";
    let error = sway_command_split_problem(balanced_at_the_end, 2).expect("refused");
    assert!(error.contains("into 3 commands, not the 2"), "{error:?}");
}

#[test]
fn sway_command_split_problem_refuses_extra_separators_and_open_quoting() {
    assert!(sway_command_split_problem("exec foot; exit", 1)
        .expect("refused")
        .contains("into 2 commands"));
    assert!(sway_command_split_problem("exec foot -T \"a\nb\"", 1)
        .expect("refused")
        .contains("into 2 commands"));
    assert!(sway_command_split_problem("exec foot \\", 1)
        .expect("refused")
        .contains("unpaired trailing"));
    assert!(sway_command_split_problem("exec foot 'a", 1)
        .expect("refused")
        .contains("unclosed \"'\" quote"));
}
