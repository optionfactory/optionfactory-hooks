use serde_json::Value;

/// What a room gets for an event, as plain text and as HTML; `alert` for a normal message instead of a notice.
#[derive(Debug, PartialEq)]
pub struct Message {
    pub text: String,
    pub html: String,
    pub alert: bool,
}

/// The message of an event, or None for what another event already tells (the branch a push creates comes as
/// `create`, the push to a pull request as `push`) or what is not finished yet (a workflow run).
pub fn message(event: &str, p: &Value) -> Option<Message> {
    let mut m = Line::default();
    let sender = p["sender"]["login"].as_str().unwrap_or("someone");
    let action = p["action"].as_str().unwrap_or_default();
    match (event, action) {
        ("ping", _) => return None,
        ("push", _) => {
            let reference = p["ref"].as_str().unwrap_or_default();
            let commits = p["commits"].as_array().cloned().unwrap_or_default();
            let branch = reference.strip_prefix("refs/heads/")?;
            if p["deleted"] == true || commits.is_empty() {
                return None;
            }
            let pusher = p["pusher"]["name"].as_str().unwrap_or(sender);
            let verb = if p["forced"] == true { "force-pushed" } else { "pushed" };
            let count = match commits.len() {
                1 => "1 commit".to_string(),
                n => format!("{n} commits"),
            };
            m.text(&format!("{pusher} {verb} "));
            m.link(&count, &p["compare"]);
            m.text(" to ");
            m.repository(p);
            m.text(&format!(":{branch}"));
            for commit in commits.iter().take(5) {
                let id = commit["id"].as_str().unwrap_or_default();
                let title = commit["message"]
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .next()
                    .unwrap_or_default();
                m.break_line();
                m.link(id.get(..7).unwrap_or(id), &commit["url"]);
                m.text(&format!(" {title}"));
            }
            if commits.len() > 5 {
                m.break_line();
                m.text(&format!("and {} more", commits.len() - 5));
            }
        }
        ("pull_request", "synchronize") => return None,
        ("pull_request", _) => {
            let pr = &p["pull_request"];
            let verb = match action {
                "closed" if pr["merged"] == true => "merged",
                "review_requested" => "requested a review on",
                "ready_for_review" => "marked ready for review",
                _ => action,
            };
            m.text(&format!("{sender} {verb} pull request "));
            m.numbered(pr);
            m.text(" in ");
            m.repository(p);
        }
        ("pull_request_review", "submitted") => {
            let verb = match p["review"]["state"].as_str() {
                Some("approved") => "approved",
                Some("changes_requested") => "requested changes on",
                _ => "reviewed",
            };
            m.text(&format!("{sender} {verb} pull request "));
            m.numbered(&p["pull_request"]);
            m.text(" in ");
            m.repository(p);
            m.quote(&p["review"]["body"]);
        }
        ("issue_comment" | "pull_request_review_comment" | "commit_comment", "created") => {
            m.text(&format!("{sender} "));
            m.link("commented", &p["comment"]["html_url"]);
            m.text(" on ");
            match event {
                "issue_comment" => m.numbered(&p["issue"]),
                "pull_request_review_comment" => m.numbered(&p["pull_request"]),
                _ => {
                    let id = p["comment"]["commit_id"].as_str().unwrap_or_default();
                    m.text(&format!("commit {}", id.get(..7).unwrap_or(id)));
                }
            }
            m.text(" in ");
            m.repository(p);
            m.quote(&p["comment"]["body"]);
        }
        ("issue_comment" | "pull_request_review_comment" | "commit_comment", _) => return None,
        ("issues", _) => {
            m.text(&format!("{sender} {action} issue "));
            m.numbered(&p["issue"]);
            m.text(" in ");
            m.repository(p);
        }
        ("release", "published") => {
            let release = &p["release"];
            let name = release["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .or(release["tag_name"].as_str())
                .unwrap_or_default();
            m.text(&format!("{sender} published release "));
            m.link(name, &release["html_url"]);
            m.text(" of ");
            m.repository(p);
        }
        ("release", _) => return None,
        ("workflow_run", "completed") => {
            let run = &p["workflow_run"];
            let conclusion = run["conclusion"].as_str().unwrap_or("finished");
            let name = run["name"].as_str().unwrap_or("workflow");
            let branch = run["head_branch"].as_str().unwrap_or_default();
            m.alert = matches!(conclusion, "failure" | "timed_out" | "startup_failure");
            m.text("Workflow ");
            m.link(name, &run["html_url"]);
            m.text(&format!(" {conclusion} on "));
            m.repository(p);
            m.text(&format!(":{branch}"));
        }
        ("workflow_run", _) => return None,
        ("create" | "delete", _) => {
            let kind = p["ref_type"].as_str().unwrap_or("ref");
            let reference = p["ref"].as_str().unwrap_or_default();
            let verb = if event == "create" { "created" } else { "deleted" };
            m.text(&format!("{sender} {verb} {kind} {reference} in "));
            m.repository(p);
        }
        _ => {
            let what = if action.is_empty() {
                event.to_string()
            } else {
                format!("{event}.{action}")
            };
            m.text(&format!("{sender} {what} in "));
            if p["repository"].is_object() {
                m.repository(p);
            } else {
                m.text(p["organization"]["login"].as_str().unwrap_or("the organization"));
            }
        }
    }
    Some(Message {
        text: m.text,
        html: m.html,
        alert: m.alert,
    })
}

/// The same line, as text and as HTML.
#[derive(Default)]
struct Line {
    text: String,
    html: String,
    alert: bool,
}

impl Line {
    fn text(&mut self, text: &str) {
        self.text.push_str(text);
        self.html.push_str(&escaped(text));
    }

    fn link(&mut self, text: &str, url: &Value) {
        self.text.push_str(text);
        match url.as_str() {
            Some(url) => self
                .html
                .push_str(&format!("<a href=\"{}\">{}</a>", escaped(url), escaped(text))),
            None => self.html.push_str(&escaped(text)),
        }
    }

    fn repository(&mut self, p: &Value) {
        let name = p["repository"]["full_name"].as_str().unwrap_or("a repository");
        self.link(name, &p["repository"]["html_url"]);
    }

    /// `#12 Title`, linked: an issue or a pull request.
    fn numbered(&mut self, item: &Value) {
        let number = item["number"].as_u64().unwrap_or_default();
        let title = item["title"].as_str().unwrap_or_default();
        self.link(&format!("#{number}"), &item["html_url"]);
        self.text(&format!(" {title}"));
    }

    /// The first line of a comment, at most 200 characters.
    fn quote(&mut self, body: &Value) {
        let first = body
            .as_str()
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default()
            .trim();
        if first.is_empty() {
            return;
        }
        let mut quoted: String = first.chars().take(200).collect();
        if quoted.len() < first.len() {
            quoted.push_str("...");
        }
        self.text.push_str(&format!(": {quoted}"));
        self.html.push_str(&format!(": <em>{}</em>", escaped(&quoted)));
    }

    fn break_line(&mut self) {
        self.text.push('\n');
        self.html.push_str("<br>");
    }
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::message;
    use serde_json::json;

    fn repository() -> serde_json::Value {
        json!({"full_name": "optionfactory/ior-portal", "html_url": "https://github.com/optionfactory/ior-portal"})
    }

    #[test]
    fn a_push_lists_its_commits_and_a_branch_push_without_commits_is_left_to_create() {
        let push = json!({
            "ref": "refs/heads/master",
            "compare": "https://github.com/optionfactory/ior-portal/compare/a...b",
            "pusher": {"name": "rferranti"},
            "repository": repository(),
            "commits": [{"id": "0123456789abcdef", "message": "Fix <login>\n\nbody", "url": "https://c/1"}],
        });
        let m = message("push", &push).unwrap();
        assert_eq!(
            m.text,
            "rferranti pushed 1 commit to optionfactory/ior-portal:master\n0123456 Fix <login>"
        );
        assert!(m.html.contains("<a href=\"https://c/1\">0123456</a> Fix &lt;login&gt;"));
        assert!(!m.alert);
        let created = json!({"ref": "refs/heads/feature", "created": true, "commits": [], "repository": repository()});
        assert_eq!(message("push", &created), None);
    }

    #[test]
    fn a_merged_pull_request_says_merged_and_a_failed_run_alerts() {
        let pr = json!({
            "action": "closed",
            "sender": {"login": "mdagri"},
            "pull_request": {"number": 12, "title": "Renewals", "merged": true, "html_url": "https://pr/12"},
            "repository": repository(),
        });
        assert_eq!(
            message("pull_request", &pr).unwrap().text,
            "mdagri merged pull request #12 Renewals in optionfactory/ior-portal"
        );
        let run = json!({
            "action": "completed",
            "workflow_run": {"name": "build", "conclusion": "failure", "head_branch": "master", "html_url": "https://run"},
            "repository": repository(),
        });
        let m = message("workflow_run", &run).unwrap();
        assert_eq!(m.text, "Workflow build failure on optionfactory/ior-portal:master");
        assert!(m.alert);
        assert_eq!(message("workflow_run", &json!({"action": "requested"})), None);
    }

    #[test]
    fn events_without_a_format_get_a_generic_line() {
        let member =
            json!({"action": "added", "sender": {"login": "rferranti"}, "organization": {"login": "optionfactory"}});
        assert_eq!(
            message("organization", &member).unwrap().text,
            "rferranti organization.added in optionfactory"
        );
        assert_eq!(message("ping", &json!({})), None);
    }
}
