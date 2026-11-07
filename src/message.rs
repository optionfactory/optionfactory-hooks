use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde_json::{Value, json};

/// A message for a room, as plain text and as HTML; `alert` for a normal message instead of a notice, which most
/// clients do not push.
#[derive(Debug, PartialEq)]
pub struct Message {
    pub text: String,
    pub html: String,
    pub alert: bool,
}

impl Message {
    /// Without link previews: a card for every link would bury the feed.
    pub fn content(&self) -> Value {
        json!({
            "msgtype": if self.alert { "m.text" } else { "m.notice" },
            "body": self.text,
            "format": "org.matrix.custom.html",
            "formatted_body": self.html,
            "com.beeper.linkpreviews": [],
        })
    }
}

/// Builds a message, the text and the HTML side by side.
#[derive(Default)]
pub struct Line {
    text: String,
    html: String,
    alert: bool,
}

impl Line {
    pub fn text(&mut self, text: &str) -> &mut Self {
        let text = with_emoji(text);
        self.text.push_str(&text);
        self.html.push_str(&escaped(&text));
        self
    }

    pub fn code(&mut self, text: &str) -> &mut Self {
        self.text.push_str(text);
        self.html.push_str(&format!("<code>{}</code>", escaped(text)));
        self
    }

    /// Plain text where there is no URL.
    pub fn link(&mut self, text: &str, url: &Value) -> &mut Self {
        let text = &with_emoji(text);
        self.text.push_str(text);
        match url.as_str() {
            Some(url) => self
                .html
                .push_str(&format!("<a href=\"{}\">{}</a>", escaped(url), escaped(text))),
            None => self.html.push_str(&escaped(text)),
        }
        self
    }

    pub fn code_link(&mut self, text: &str, url: &Value) -> &mut Self {
        self.text.push_str(text);
        match url.as_str() {
            Some(url) => self.html.push_str(&format!(
                "<a href=\"{}\"><code>{}</code></a>",
                escaped(url),
                escaped(text)
            )),
            None => self.html.push_str(&format!("<code>{}</code>", escaped(text))),
        }
        self
    }

    /// `[owner/repo]` or `[List]` at the start of a message, bold and linked.
    pub fn prefix(&mut self, name: &str, url: &Value) -> &mut Self {
        self.text.push_str(&format!("[{name}] "));
        let name = match url.as_str() {
            Some(url) => format!("<a href=\"{}\">{}</a>", escaped(url), escaped(name)),
            None => escaped(name),
        };
        self.html.push_str(&format!("<b>[{name}]</b> "));
        self
    }

    /// A label or a status in its own colour, as GitHub and ClickUp show them; the text black or white, whichever
    /// reads better on it.
    pub fn pill(&mut self, name: &str, color: Option<&str>, description: Option<&str>) -> &mut Self {
        let name = &with_emoji(name);
        self.text.push_str(&format!("[{name}]"));
        let background = color
            .map(|c| c.trim_start_matches('#'))
            .filter(|c| c.len() == 6 && c.chars().all(|c| c.is_ascii_hexdigit()))
            .unwrap_or("ededed");
        let title = description
            .filter(|d| !d.is_empty())
            .map(|d| format!(" title=\"{}\"", escaped(d)))
            .unwrap_or_default();
        self.html.push_str(&format!(
            "<font data-mx-bg-color=\"#{background}\" data-mx-color=\"{}\"{title}>&nbsp;{}&nbsp;</font>",
            readable_on(background),
            escaped(name)
        ));
        self
    }

    /// Items one per line, as a list.
    pub fn list(&mut self, items: Vec<Message>) -> &mut Self {
        if items.is_empty() {
            return self;
        }
        self.html.push_str("<ul>");
        for item in items {
            self.text.push_str(&format!("\n- {}", item.text));
            self.html.push_str(&format!("<li>{}</li>", item.html));
        }
        self.html.push_str("</ul>");
        self
    }

    /// Markdown written by someone (a comment, a description), at most `max` characters of it, as a quote: rendered,
    /// with their HTML shown as text and their images as links, since a room shows only images of its server.
    pub fn quote(&mut self, body: &Value, max: usize) -> &mut Self {
        let source = without_comments(body.as_str().unwrap_or_default());
        let source = source.trim();
        if source.is_empty() {
            return self;
        }
        let mut kept: String = source.chars().take(max).collect();
        if kept.len() < source.len() {
            // cut at the end of a line, not inside a word or a code span
            if let Some(end) = kept.rfind('\n').filter(|end| *end > max / 2) {
                kept.truncate(end);
            }
            kept.push_str("\n\n…");
        }
        for line in kept.lines().filter(|line| !line.trim().is_empty()) {
            self.text.push_str(&format!("\n> {}", with_emoji(line.trim_end())));
        }
        let events =
            Parser::new_ext(&kept, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS).map(
                |event| match event {
                    Event::Html(html) | Event::InlineHtml(html) => Event::Text(html),
                    Event::Text(text) => Event::Text(with_emoji(&text).into()),
                    Event::Start(Tag::Image {
                        link_type,
                        dest_url,
                        title,
                        id,
                    }) => Event::Start(Tag::Link {
                        link_type,
                        dest_url,
                        title,
                        id,
                    }),
                    Event::End(TagEnd::Image) => Event::End(TagEnd::Link),
                    other => other,
                },
            );
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, events);
        self.html
            .push_str(&format!("<blockquote>{}</blockquote>", html.trim_end()));
        self
    }

    pub fn alert(&mut self) -> &mut Self {
        self.alert = true;
        self
    }

    pub fn done(&mut self) -> Message {
        Message {
            text: std::mem::take(&mut self.text),
            html: std::mem::take(&mut self.html),
            alert: self.alert,
        }
    }
}

pub fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// GitHub's `:shortcodes:` as the emoji GitHub shows for them (`:sparkles: new` as `✨ new`), unknown ones left as they
/// are.
pub fn with_emoji(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let name = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-')))
            .filter(|end| after[*end..].starts_with(':'))
            .map(|end| &after[..end]);
        match name.and_then(|name| emojis::get_by_shortcode(name).map(|emoji| (name, emoji))) {
            Some((name, emoji)) => {
                out.push_str(emoji.as_str());
                rest = &after[name.len() + 1..];
            }
            None => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The templates of pull requests and issues are full of `<!-- -->` hints nobody sees on GitHub.
fn without_comments(text: &str) -> String {
    let mut kept = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        kept.push_str(&rest[..start]);
        rest = rest[start..].find("-->").map_or("", |end| &rest[start + end + 3..]);
    }
    kept.push_str(rest);
    kept
}

/// White text where it reads well enough (WCAG contrast 4.5), as GitHub shows its labels; otherwise whichever of black
/// and white has the higher contrast on the background.
fn readable_on(background: &str) -> &'static str {
    let channel = |i: usize| {
        let c = u8::from_str_radix(&background[i..i + 2], 16).unwrap_or(0) as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
    let (on_white, on_black) = (1.05 / (luminance + 0.05), (luminance + 0.05) / 0.05);
    if on_white >= 4.5 || on_white >= on_black {
        "#ffffff"
    } else {
        "#000000"
    }
}

#[cfg(test)]
mod tests {
    use super::{Line, readable_on};
    use serde_json::json;

    #[test]
    fn text_and_html_go_side_by_side_and_what_people_wrote_is_rendered_but_not_trusted() {
        let m = Line::default()
            .prefix("acme/widgets", &json!("https://github.com/acme/widgets"))
            .text("alice commented on ")
            .link("issue #3", &json!("https://x/3?a=1&b=2"))
            .quote(
                &json!("<!-- template hint -->\nLooks **good**, <script>x</script>\n\n![shot](https://img/1.png)"),
                500,
            )
            .done();
        assert_eq!(
            m.text,
            "[acme/widgets] alice commented on issue #3\n> Looks **good**, <script>x</script>\n> ![shot](https://img/1.png)"
        );
        assert_eq!(
            m.html,
            "<b>[<a href=\"https://github.com/acme/widgets\">acme/widgets</a>]</b> alice commented on \
             <a href=\"https://x/3?a=1&amp;b=2\">issue #3</a><blockquote><p>Looks <strong>good</strong>, \
             &lt;script&gt;x&lt;/script&gt;</p>\n<p><a href=\"https://img/1.png\">shot</a></p></blockquote>"
        );
    }

    #[test]
    fn labels_get_their_colour_and_readable_text() {
        let m = Line::default()
            .pill("bug", Some("d73a4a"), Some("Something isn't working"))
            .done();
        assert_eq!(m.text, "[bug]");
        assert_eq!(
            m.html,
            "<font data-mx-bg-color=\"#d73a4a\" data-mx-color=\"#ffffff\" title=\"Something isn't working\">&nbsp;bug&nbsp;</font>"
        );
        assert_eq!(readable_on("fbca04"), "#000000");
        assert_eq!(readable_on("0e1a2b"), "#ffffff");
    }

    #[test]
    fn shortcodes_become_emoji_but_not_in_code() {
        let m = Line::default()
            .text(":sparkles: new at 10:30:")
            .code(":bug:")
            .quote(&json!(":fire: removed `:memo:` and :not_an_emoji:"), 500)
            .done();
        assert_eq!(m.text, "✨ new at 10:30::bug:\n> 🔥 removed `📝` and :not_an_emoji:");
        assert_eq!(
            m.html,
            "✨ new at 10:30:<code>:bug:</code><blockquote><p>🔥 removed <code>:memo:</code> and :not_an_emoji:</p></blockquote>"
        );
    }

    #[test]
    fn long_quotes_are_cut_at_a_line() {
        let body = json!("first line of the description\nsecond line, which goes on and on and on\nthird");
        let m = Line::default().quote(&body, 50).done();
        assert_eq!(m.text, "\n> first line of the description\n> …");
    }
}
