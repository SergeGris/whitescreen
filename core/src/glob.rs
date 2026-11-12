//! Shell-style pattern matching, for `--monitor`.
//!
//! The shell cannot expand a monitor name: `DP-*` matches files, and there
//! are no files called that, so it arrives here untouched. Doing the
//! expansion here is what makes `--monitor 'DP-*'` mean what it looks like it
//! means.
//!
//! The sh subset that applies to a connector name: `*`, `?`, and `[...]`
//! classes with ranges and negation. Brace expansion is absent on purpose --
//! the shell does that one itself, before the argument ever reaches us.

/// Whether `text` matches the shell pattern `pattern`.
pub fn matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();

    let (mut pi, mut ti) = (0usize, 0usize);
    // Where to resume from after the most recent `*`, and how much of the
    // text that `*` has been made to swallow so far.
    let mut star: Option<(usize, usize)> = None;

    loop {
        if ti == t.len() {
            // Trailing stars can match the empty rest; nothing else can.
            while pi < p.len() && p[pi] == '*' {
                pi += 1;
            }
            return pi == p.len();
        }

        match step(&p, pi, t[ti]) {
            Step::Star => {
                star = Some((pi + 1, ti));
                pi += 1;
            }
            Step::Take(next_pi) => {
                pi = next_pi;
                ti += 1;
            }
            Step::No => match star {
                // Give the star one more character and try again.
                Some((resume, taken)) => {
                    pi = resume;
                    ti = taken + 1;
                    star = Some((resume, taken + 1));
                }
                None => return false,
            },
        }
    }
}

/// Whether a pattern asks for any expansion at all.
///
/// A plain name is not a pattern, and the caller treats the two differently:
/// naming a monitor that is not plugged in is meaningful, but a *pattern*
/// that matches nothing is a typo worth reporting.
pub fn is_pattern(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

enum Step {
    /// A `*`: remember the position and try matching nothing first.
    Star,
    /// One character consumed; carry on at this pattern index.
    Take(usize),
    No,
}

fn step(p: &[char], pi: usize, c: char) -> Step {
    match p.get(pi) {
        None       => Step::No,
        Some('*')  => Step::Star,
        Some('?')  => Step::Take(pi + 1),
        Some('[')  => match class(p, pi, c) {
            Some((end, true))  => Step::Take(end),
            Some((_,   false)) => Step::No,
            // An unterminated '[' is a literal, as in sh.
            None => if c == '[' { Step::Take(pi + 1) } else { Step::No },
        },
        Some(&lit) => if lit == c { Step::Take(pi + 1) } else { Step::No },
    }
}

/// Parse the class starting at `p[start]`, and say whether `c` is in it.
///
/// Returns the index just past the closing `]`, or `None` when there is no
/// closing `]` at all.
fn class(p: &[char], start: usize, c: char) -> Option<(usize, bool)> {
    let mut i = start + 1;

    let negated = matches!(p.get(i), Some('!' | '^'));
    if negated {
        i += 1;
    }

    let mut hit = false;
    let mut first = true;
    while i < p.len() {
        // A ']' straight after the opening bracket is a literal ']', which is
        // the only way sh lets you put one in a class.
        if p[i] == ']' && !first {
            return Some((i + 1, hit != negated));
        }
        first = false;

        let is_range = p.get(i + 1) == Some(&'-') && matches!(p.get(i + 2), Some(&e) if e != ']');
        if is_range {
            if p[i] <= c && c <= p[i + 2] {
                hit = true;
            }
            i += 3;
        } else {
            if p[i] == c {
                hit = true;
            }
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_matches_only_itself() {
        assert!(matches("DP-1", "DP-1"));
        assert!(!matches("DP-1", "DP-2"));
        assert!(!matches("DP-1", "DP-11"));
        assert!(!matches("DP-1", "eDP-1"));
    }

    #[test]
    fn star_spans_anything_including_nothing() {
        assert!(matches("DP-*", "DP-1"));
        assert!(matches("DP-*", "DP-"));
        assert!(matches("*", "HDMI-A-2"));
        assert!(matches("*DP*", "eDP-1"));
        assert!(matches("HDMI-*-*", "HDMI-A-2"));
        assert!(!matches("DP-*", "HDMI-A-1"));
    }

    #[test]
    fn a_star_backtracks_rather_than_giving_up_on_the_first_try() {
        // The greedy first attempt swallows both dashes; matching needs it to
        // hand one back.
        assert!(matches("*-1", "HDMI-A-1"));
        assert!(matches("*A*1", "HDMI-A-2-1"));
        assert!(!matches("*-9", "HDMI-A-1"));
    }

    #[test]
    fn question_mark_is_exactly_one_character() {
        assert!(matches("DP-?", "DP-1"));
        assert!(!matches("DP-?", "DP-12"));
        assert!(!matches("DP-?", "DP-"));
    }

    #[test]
    fn classes_cover_sets_ranges_and_negation() {
        assert!(matches("DP-[12]", "DP-1"));
        assert!(!matches("DP-[12]", "DP-3"));
        assert!(matches("DP-[0-9]", "DP-7"));
        assert!(!matches("DP-[0-9]", "DP-x"));
        assert!(matches("DP-[!0-9]", "DP-x"));
        assert!(!matches("DP-[!0-9]", "DP-7"));
        assert!(matches("DP-[^0-9]", "DP-x"));
    }

    #[test]
    fn an_unterminated_class_is_a_literal_bracket() {
        assert!(matches("DP-[1", "DP-[1"));
        assert!(!matches("DP-[1", "DP-1"));
    }

    #[test]
    fn only_wildcards_count_as_a_pattern() {
        assert!(is_pattern("DP-*"));
        assert!(is_pattern("DP-?"));
        assert!(is_pattern("DP-[12]"));
        assert!(!is_pattern("DP-1"));
        assert!(!is_pattern("HDMI-A-2"));
    }
}
