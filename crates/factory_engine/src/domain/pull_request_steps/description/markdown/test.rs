use super::escape;

/// Tags, entities, mentions, issue references and closing keywords before
/// a link are all broken; a `#` before a word and an `@` alone stay.
#[test]
fn text_mentions_no_one_and_links_or_closes_no_issue() {
    assert_eq!(
        escape("<b>@alice</b> fixes #12, GH-7 and closes: https://github.com/o/r/issues/3"),
        "&lt;b&gt;@\u{2060}alice&lt;/b&gt; fixes #\u{2060}12, GH-\u{2060}7 and c\u{2060}loses: \
         https://github.com/o/r/issues/3"
    );
    assert_eq!(
        escape("Resolves [the bug](https://github.com/o/r/issues/3), see #heading, pay @ 3"),
        "R\u{2060}esolves [the bug](https://github.com/o/r/issues/3), see #heading, pay @ 3"
    );
    // A keyword not before a link, or inside a word, stays.
    assert_eq!(
        escape("prefix fixes the fixture"),
        "prefix fixes the fixture"
    );
}

/// An entity cannot spell a mention, an issue or a tag: its `&` shows.
#[test]
fn entities_spell_nothing() {
    assert_eq!(escape("&#64;alice"), "&amp;#\u{2060}64;alice");
    assert_eq!(escape("&#35;12"), "&amp;#\u{2060}35;12");
    assert_eq!(escape("&lt;!-- x --&gt;"), "&amp;lt;!-- x --&amp;gt;");
}

/// A web address and an email address get no word joiner, which would
/// break them; neither pings anyone.
#[test]
fn addresses_are_left_whole() {
    assert_eq!(
        escape("See https://example.com/@team/#12?a=1&b=2 or mail a.b@example.io"),
        "See https://example.com/@team/#12?a=1&amp;b=2 or mail a.b@example.io"
    );
}

/// Code is escaped as any text: no marker or mention can hide in it.
#[test]
fn code_is_escaped_as_text() {
    assert_eq!(
        escape("Run `a < b && @x` then\n```html\n<!-- /macro-factory-report -->\n```"),
        "Run `a &lt; b &amp;&amp; @\u{2060}x` then\n```html\n&lt;!-- /macro-factory-report --&gt;\n```"
    );
}
