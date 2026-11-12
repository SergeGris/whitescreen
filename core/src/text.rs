//! Filling placeholders in a translated string.
//!
//! Here rather than beside the gettext calls because both front ends need
//! it and neither owns it: the lookup is the platform's business, the
//! substitution is not.
//!
//! Anything with a value in it uses `{0}`, `{1}` … rather than Rust's own
//! `format!`. Two reasons, and the second is the important one: `format!`
//! needs a literal, so a translated template cannot be passed to it at all;
//! and word order differs between languages, so a translator has to be able
//! to move the values around, which numbered placeholders allow and bare
//! `{}` does not.

/// Fill the numbered placeholders in a translated template.
///
/// `{0}` takes the first argument, `{1}` the second, and so on. A placeholder
/// with no argument behind it is left exactly as it is: a translator can
/// mistype one, and a visible `{7}` is a bug report, where a panic would be a
/// crash in somebody's hands.
pub fn fill(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];

        // The shortest thing that can be a placeholder is "{0}".
        let Some(close) = rest.find('}') else {
            // An unclosed brace is literal text; nothing else can be done
            // with it, and dropping it would silently eat the rest.
            break;
        };
        match rest[1..close].parse::<usize>().ok().and_then(|i| args.get(i)) {
            Some(arg) => out.push_str(arg),
            None      => out.push_str(&rest[..=close]),
        }
        rest = &rest[close + 1..];
    }

    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::fill;

    #[test]
    fn placeholders_take_their_argument() {
        assert_eq!(fill("Step {0} — {1}", &["3", "#FF0000"]), "Step 3 — #FF0000");
    }

    #[test]
    fn a_translator_may_reorder_or_repeat_them() {
        // The reason for numbering them at all.
        assert_eq!(fill("{1} ({0})", &["DP-1", "1920"]), "1920 (DP-1)");
        assert_eq!(fill("{0}/{0}", &["x"]), "x/x");
    }

    #[test]
    fn a_template_with_nothing_to_fill_is_returned_as_it_is() {
        assert_eq!(fill("Press any key to exit", &[]), "Press any key to exit");
        assert_eq!(fill("", &["x"]), "");
    }

    #[test]
    fn a_placeholder_with_no_argument_is_left_visible() {
        // Mistyped by a translator: better seen than panicked on.
        assert_eq!(fill("a {9} b", &["x"]), "a {9} b");
        assert_eq!(fill("{notanumber}", &["x"]), "{notanumber}");
    }

    #[test]
    fn stray_braces_survive() {
        assert_eq!(fill("100% {", &[]), "100% {");
        assert_eq!(fill("}{", &[]), "}{");
        assert_eq!(fill("{} and {0}", &["x"]), "{} and x");
    }
}

/// `s` as a JSON string literal, quotes and all.
///
/// Shared because both front ends print a machine-readable monitor list and
/// the names in it come from the system: a display whose model contains a
/// quote would otherwise produce output that is not JSON.
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything below 0x20 has to be escaped, and \u is the only
            // form JSON has for the ones without a short escape.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod json_tests {
    use super::json_string;

    #[test]
    fn plain_text_is_just_quoted() {
        assert_eq!(json_string("DP-1"), "\"DP-1\"");
        assert_eq!(json_string(""), "\"\"");
    }

    #[test]
    fn what_json_requires_escaping_is_escaped() {
        assert_eq!(json_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(json_string("tab\there"), "\"tab\\there\"");
        assert_eq!(json_string("line\nbreak"), "\"line\\nbreak\"");
        // A control character has no short escape of its own.
        assert_eq!(json_string("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn a_backslash_in_a_windows_device_name_survives() {
        // \\.\DISPLAY1 is the shape of every monitor key on Windows.
        assert_eq!(json_string(r"\\.\DISPLAY1"), r#""\\\\.\\DISPLAY1""#);
    }
}
