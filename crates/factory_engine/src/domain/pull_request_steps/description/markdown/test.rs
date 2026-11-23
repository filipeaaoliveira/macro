use super::{line, lines};

/// Outside code, tags and markers are escaped, and mentions, issue
/// references and closing keywords before a link are broken; an email, an
/// `@` alone and a `#` before a word stay as they are.
#[test]
fn text_mentions_no_one_and_links_no_issue() {
    assert_eq!(
        line("<b>@alice</b> fixes #12, GH-7 and closes: https://github.com/o/r/issues/3"),
        "&lt;b&gt;@\u{2060}alice&lt;/b&gt; fixes #\u{2060}12, GH-\u{2060}7 and c\u{2060}loses: \
         https://github.com/o/r/issues/3"
    );
    assert_eq!(
        line("Mail a@b.io, pay @ 3, see #heading; prefix fixes the fixture"),
        "Mail a@\u{2060}b.io, pay @ 3, see #heading; prefix fixes the fixture"
    );
    assert_eq!(
        line("<!-- /macro-factory-report -->"),
        "&lt;!-- /macro-factory-report --&gt;"
    );
}

/// Inline code and fenced blocks read as written, but no marker can be
/// forged in them.
#[test]
fn code_is_kept_but_holds_no_marker() {
    assert_eq!(
        line("Run `a < b && @x` then ``<!-- x -->``, and `unclosed"),
        "Run `a < b && @x` then ``<\u{2060}!-- x -->``, and `unclosed"
    );
    assert_eq!(
        lines(
            "Before <i>\n```html\n<p>@bob</p>\n<!-- /macro-factory-report -->\n```\nAfter #4"
                .lines()
        ),
        [
            "Before &lt;i&gt;",
            "```html",
            "<p>@bob</p>",
            "<\u{2060}!-- /macro-factory-report -->",
            "```",
            "After #\u{2060}4",
        ]
    );
}
