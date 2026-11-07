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
    pub fn content(&self) -> Value {
        json!({
            "msgtype": if self.alert { "m.text" } else { "m.notice" },
            "body": self.text,
            "format": "org.matrix.custom.html",
            "formatted_body": self.html,
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
        self.text.push_str(text);
        self.html.push_str(&escaped(text));
        self
    }

    pub fn bold(&mut self, text: &str) -> &mut Self {
        self.text.push_str(text);
        self.html.push_str(&format!("<b>{}</b>", escaped(text)));
        self
    }

    pub fn code(&mut self, text: &str) -> &mut Self {
        self.text.push_str(text);
        self.html.push_str(&format!("<code>{}</code>", escaped(text)));
        self
    }

    /// Plain text where there is no URL.
    pub fn link(&mut self, text: &str, url: &Value) -> &mut Self {
        self.text.push_str(text);
        match url.as_str() {
            Some(url) => self
                .html
                .push_str(&format!("<a href=\"{}\">{}</a>", escaped(url), escaped(text))),
            None => self.html.push_str(&escaped(text)),
        }
        self
    }

    pub fn bold_link(&mut self, text: &str, url: &Value) -> &mut Self {
        self.text.push_str(text);
        match url.as_str() {
            Some(url) => self
                .html
                .push_str(&format!("<a href=\"{}\"><b>{}</b></a>", escaped(url), escaped(text))),
            None => self.html.push_str(&format!("<b>{}</b>", escaped(text))),
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

    pub fn break_line(&mut self) -> &mut Self {
        self.text.push('\n');
        self.html.push_str("<br>");
        self
    }

    /// The first `lines` non-empty lines of a text written by someone (a comment, a description), at most 300
    /// characters, as a quote; nothing when it is empty.
    pub fn quote(&mut self, body: &Value, lines: usize) -> &mut Self {
        let kept: Vec<&str> = body
            .as_str()
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(lines)
            .collect();
        if kept.is_empty() {
            return self;
        }
        let joined = kept.join("\n");
        let mut quoted: String = joined.chars().take(300).collect();
        if quoted.len() < joined.len() || body.as_str().unwrap_or_default().trim().len() > joined.len() {
            quoted.push_str(" ...");
        }
        for line in quoted.lines() {
            self.text.push_str(&format!("\n> {line}"));
        }
        let html: Vec<String> = quoted.lines().map(escaped).collect();
        self.html
            .push_str(&format!("<blockquote>{}</blockquote>", html.join("<br>")));
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

#[cfg(test)]
mod tests {
    use super::Line;
    use serde_json::json;

    #[test]
    fn text_and_html_go_side_by_side_and_what_people_wrote_is_escaped() {
        let m = Line::default()
            .bold("alice")
            .text(" commented on ")
            .link("#3", &json!("https://x/3?a=1&b=2"))
            .quote(&json!("first <b>line</b>\n\nsecond\nthird\nfourth"), 2)
            .done();
        assert_eq!(m.text, "alice commented on #3\n> first <b>line</b>\n> second ...");
        assert_eq!(
            m.html,
            "<b>alice</b> commented on <a href=\"https://x/3?a=1&amp;b=2\">#3</a>\
             <blockquote>first &lt;b&gt;line&lt;/b&gt;<br>second ...</blockquote>"
        );
    }
}
