//! Text the factory did not write (the request, and what agents wrote) as
//! a pull request shows it: it mentions no one, links or closes no issue,
//! opens no tag or entity, and holds no marker of the report.
//!
//! All of it is escaped the same way, code included, since only GitHub's
//! own parser could tell code from text: `&`, `<` and `>` become `&amp;`,
//! `&lt;` and `&gt;`, and an invisible word joiner follows each `@` before
//! a name, each `#` before a number, the `GH-` of `GH-123`, and the first
//! letter of a closing keyword (`fix`, `closes`, `resolved`...) before a
//! link. Code shows `&lt;` where it held `<`: safety wins over looks. A web
//! address (from its `scheme://` to the next space) and an email address
//! get no word joiner, which would break them; neither pings anyone.

#[cfg(test)]
mod test;

/// A word joiner: invisible, and breaks what GitHub would read as a mention
/// or an issue reference.
const WORD_JOINER: char = '\u{2060}';

/// The words GitHub reads before an issue to close it once merged.
const CLOSING_KEYWORDS: [&str; 9] = [
    "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
];

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

/// Whether `c`, if any, may end the name before an email's `@`.
fn email_char(c: Option<&char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-'))
}

/// Whether a web address starts at `at`: a scheme (a letter, then letters,
/// digits, `+`, `.` or `-`) right before `://`, at the start of a word.
fn url_at(chars: &[char], at: usize) -> bool {
    if at > 0 && word_char(chars.get(at - 1)) {
        return false;
    }
    if !chars.get(at).is_some_and(char::is_ascii_alphabetic) {
        return false;
    }
    let scheme = chars[at..]
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        .count();
    starts_with(chars, at + scheme, "://")
}

/// The closing keyword `chars` hold at `at`, as a whole word followed by a
/// link (after an optional `:` and spaces): a web address, or a Markdown
/// link's `[`. How many characters it takes.
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
    let link = chars.get(next) == Some(&'[') || url_at(chars, next);
    link.then_some(length)
}

/// Push `c` onto `escaped`, as an entity when it is `&`, `<` or `>`.
fn push_escaped(escaped: &mut String, c: char) {
    match c {
        '&' => escaped.push_str("&amp;"),
        '<' => escaped.push_str("&lt;"),
        '>' => escaped.push_str("&gt;"),
        c => escaped.push(c),
    }
}

/// `text` escaped, as the module says.
pub(super) fn escape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut escaped = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let previous = index.checked_sub(1).and_then(|at| chars.get(at));
        let next = chars.get(index + 1);

        if url_at(&chars, index) {
            while let Some(c) = chars.get(index).filter(|c| !c.is_whitespace()) {
                push_escaped(&mut escaped, *c);
                index += 1;
            }
            continue;
        }
        let starts_word = !word_char(previous);
        let issue_number = chars.get(index + 3).is_some_and(char::is_ascii_digit);
        if starts_word && starts_with(&chars, index, "gh-") && issue_number {
            escaped.extend(&chars[index..index + 3]);
            escaped.push(WORD_JOINER);
            index += 3;
            continue;
        }
        if let Some(length) = closing_keyword(&chars, index) {
            escaped.push(c);
            escaped.push(WORD_JOINER);
            escaped.extend(&chars[index + 1..index + length]);
            index += length;
            continue;
        }

        push_escaped(&mut escaped, c);
        let handle = c == '@'
            && !email_char(previous)
            && next.is_some_and(|n| n.is_ascii_alphanumeric() || *n == '-');
        let issue = c == '#' && next.is_some_and(char::is_ascii_digit);
        if handle || issue {
            escaped.push(WORD_JOINER);
        }
        index += 1;
    }
    escaped
}
