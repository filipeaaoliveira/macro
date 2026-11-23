//! Agents' text as the report quotes it in a pull request: its Markdown and
//! code read as written, but it mentions no one, links or closes no issue,
//! opens no tag, and holds no marker of the report.
//!
//! Outside code, `<` and `>` become `&lt;` and `&gt;`; a word joiner, which
//! is invisible, follows each `@` before a name, each `#` before a number,
//! the `GH-` of `GH-123`, and the first letter of a closing keyword (`fix`,
//! `closes`, `resolved`...) before a link. In code, which GitHub links and
//! renders as is, only the opening of an HTML comment is broken, so no
//! marker can be forged there.

#[cfg(test)]
mod test;

/// A word joiner: invisible, and breaks what GitHub would read as a mention
/// or an issue reference.
const WORD_JOINER: char = '\u{2060}';

/// What opens an HTML comment, as every marker of the report does.
const COMMENT_OPEN: &str = "<!--";

/// The words GitHub reads before an issue to close it once merged.
const CLOSING_KEYWORDS: [&str; 9] = [
    "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
];

/// `text` with the opening of each HTML comment broken: what code may hold.
pub(super) fn in_code(text: &str) -> String {
    text.replace(COMMENT_OPEN, &format!("<{WORD_JOINER}!--"))
}

/// Whether `chars` from `at` start with `word`, in any case.
fn starts_with(chars: &[char], at: usize, word: &str) -> bool {
    let mut index = at;
    for expected in word.chars() {
        match chars.get(index) {
            Some(c) if c.eq_ignore_ascii_case(&expected) => index += 1,
            _ => return false,
        }
    }
    true
}

/// Whether `c`, if any, joins a word.
fn word_char(c: Option<&char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || *c == '_')
}

/// The closing keyword `chars` hold at `at`, as a whole word followed by a
/// link (after an optional `:` and spaces): how many characters it takes.
fn closing_keyword(chars: &[char], at: usize) -> Option<usize> {
    if at > 0 && word_char(chars.get(at - 1)) {
        return None;
    }
    let length = CLOSING_KEYWORDS
        .iter()
        .filter(|keyword| starts_with(chars, at, keyword))
        .map(|keyword| keyword.len())
        .find(|length| !word_char(chars.get(at + length)))?;
    let mut next = at + length;
    if chars.get(next) == Some(&':') {
        next += 1;
    }
    while chars.get(next).is_some_and(|c| c.is_whitespace()) {
        next += 1;
    }
    starts_with(chars, next, "http").then_some(length)
}

/// `text` outside code, as the module says.
fn outside_code(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut quoted = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let next = chars.get(index + 1);
        let starts_word = index == 0 || !word_char(chars.get(index - 1));

        if starts_word && starts_with(&chars, index, "gh-") && next_is_digit(&chars, index + 3) {
            quoted.extend(&chars[index..index + 3]);
            quoted.push(WORD_JOINER);
            index += 3;
            continue;
        }
        if let Some(length) = closing_keyword(&chars, index) {
            quoted.push(c);
            quoted.push(WORD_JOINER);
            quoted.extend(&chars[index + 1..index + length]);
            index += length;
            continue;
        }
        match c {
            '<' => quoted.push_str("&lt;"),
            '>' => quoted.push_str("&gt;"),
            '@' if next.is_some_and(|n| n.is_ascii_alphanumeric() || *n == '-') => {
                quoted.push(c);
                quoted.push(WORD_JOINER);
            }
            '#' if next.is_some_and(char::is_ascii_digit) => {
                quoted.push(c);
                quoted.push(WORD_JOINER);
            }
            _ => quoted.push(c),
        }
        index += 1;
    }
    quoted
}

fn next_is_digit(chars: &[char], at: usize) -> bool {
    chars.get(at).is_some_and(char::is_ascii_digit)
}

/// How many backticks `chars` hold from `at`.
fn backticks(chars: &[char], at: usize) -> usize {
    chars[at..].iter().take_while(|c| **c == '`').count()
}

/// One line of agents' text, with its inline code spans (a run of
/// backticks to the next run of as many) kept as code.
pub(super) fn line(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut quoted = String::with_capacity(text.len());
    let mut plain = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '`' {
            plain.push(chars[index]);
            index += 1;
            continue;
        }

        let run = backticks(&chars, index);
        let mut close = index + run;
        let span_end = loop {
            match chars[close..].iter().position(|c| *c == '`') {
                Some(offset) => {
                    let at = close + offset;
                    let length = backticks(&chars, at);
                    if length == run {
                        break Some(at);
                    }
                    close = at + length;
                }
                None => break None,
            }
        };
        let fence: String = chars[index..index + run].iter().collect();
        match span_end {
            Some(end) => {
                quoted.push_str(&outside_code(&std::mem::take(&mut plain)));
                let code: String = chars[index + run..end].iter().collect();
                quoted.push_str(&format!("{fence}{}{fence}", in_code(&code)));
                index = end + run;
            }
            // Backticks no run closes are text.
            None => {
                plain.push_str(&fence);
                index += run;
            }
        }
    }
    quoted.push_str(&outside_code(&plain));
    quoted
}

/// Lines of agents' text, with fenced code blocks (between lines that start
/// with ```` ``` ```` or `~~~`) kept as code.
pub(super) fn lines<'a>(text: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut fence: Option<&str> = None;
    let mut quoted = Vec::new();
    for text_line in text {
        let trimmed = text_line.trim_start();
        let opens = ["```", "~~~"]
            .into_iter()
            .find(|mark| trimmed.starts_with(mark));
        match (fence, opens) {
            (None, Some(mark)) => {
                fence = Some(mark);
                quoted.push(in_code(text_line));
            }
            (Some(open), Some(mark)) if open == mark => {
                fence = None;
                quoted.push(in_code(text_line));
            }
            (Some(_), _) => quoted.push(in_code(text_line)),
            (None, None) => quoted.push(line(text_line)),
        }
    }
    quoted
}
