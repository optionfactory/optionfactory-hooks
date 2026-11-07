use crate::message::{Line, Message};
use serde_json::{Value, json};

/// The message of an event: `[owner/repo] actor verb object: title`, the state in the verb, colour only on labels.
/// None for what another event already tells (the branch a push creates comes as `create`, the push to a pull request
/// as `push`), for the noise of edits, and for workflow runs, which become reactions on their push (see `ci`).
pub fn message(event: &str, p: &Value) -> Option<Message> {
    let mut m = Line::default();
    let sender = p["sender"]["login"].as_str().unwrap_or("someone");
    let action = p["action"].as_str().unwrap_or_default();
    match p["repository"]["full_name"].as_str() {
        Some(name) => m.prefix(name, &p["repository"]["html_url"]),
        None => m.prefix(
            p["organization"]["login"].as_str().unwrap_or("GitHub"),
            &json!(format!(
                "https://github.com/{}",
                p["organization"]["login"].as_str().unwrap_or_default()
            )),
        ),
    };
    match (event, action) {
        ("ping", _) => return None,
        ("push", _) => {
            let branch = p["ref"].as_str()?.strip_prefix("refs/heads/")?;
            let commits = p["commits"].as_array().cloned().unwrap_or_default();
            if p["deleted"] == true || commits.is_empty() {
                return None;
            }
            let pusher = p["pusher"]["name"].as_str().unwrap_or(sender);
            let count = match commits.len() {
                1 => "1 commit".to_string(),
                n => format!("{n} commits"),
            };
            let verb = if p["forced"] == true { "force-pushed" } else { "pushed" };
            m.text(&format!("{pusher} {verb} ")).link(&count, &p["compare"]);
            m.text(" to ").code(branch).text(":");
            let mut items: Vec<Message> = commits
                .iter()
                .take(10)
                .map(|commit| {
                    let mut item = Line::default();
                    let id = commit["id"].as_str().unwrap_or_default();
                    item.code_link(id.get(..8).unwrap_or(id), &commit["url"]);
                    item.text(&format!(" {}", title(&commit["message"])));
                    let author = commit["author"]["username"]
                        .as_str()
                        .or(commit["author"]["name"].as_str());
                    if let Some(author) = author.filter(|author| *author != pusher) {
                        item.text(&format!(" by {author}"));
                    }
                    item.done()
                })
                .collect();
            if commits.len() > 10 {
                items.push(Line::default().text(&format!("and {} more", commits.len() - 10)).done());
            }
            m.list(items);
        }
        ("pull_request", "synchronize" | "edited") => return None,
        ("pull_request", _) => {
            let pr = &p["pull_request"];
            m.text(&format!("{sender} "));
            match action {
                "opened" if pr["draft"] == true => m.text("drafted "),
                "closed" if pr["merged"] == true => {
                    let commit = pr["merge_commit_sha"]
                        .as_str()
                        .zip(p["repository"]["html_url"].as_str())
                        .map(|(sha, repo)| json!(format!("{repo}/commit/{sha}")));
                    m.link("merged", &commit.unwrap_or_default()).text(" ")
                }
                "ready_for_review" => m.text("marked as ready for review "),
                "converted_to_draft" => m.text("turned into a draft "),
                "review_requested" => {
                    let reviewer = p["requested_reviewer"]["login"]
                        .as_str()
                        .or(p["requested_team"]["name"].as_str())
                        .unwrap_or("someone");
                    m.text(&format!("requested a review from {reviewer} for "))
                }
                "assigned" => m.text(&format!(
                    "assigned {} to ",
                    p["assignee"]["login"].as_str().unwrap_or("someone")
                )),
                "unassigned" => m.text(&format!(
                    "unassigned {} from ",
                    p["assignee"]["login"].as_str().unwrap_or("someone")
                )),
                "labeled" => label(&mut m, "added", &p["label"], "to"),
                "unlabeled" => label(&mut m, "removed", &p["label"], "from"),
                _ => m.text(&format!("{} ", action.replace('_', " "))),
            };
            numbered(&mut m, "pull request", pr);
            if action == "opened" {
                let head = pr["head"]["ref"].as_str().unwrap_or_default();
                let base = pr["base"]["ref"].as_str().unwrap_or_default();
                m.text(" (").code(head).text(" into ").code(base).text(")");
                labels(&mut m, &pr["labels"]);
                m.quote(&pr["body"], 500);
            }
        }
        ("pull_request_review", "submitted") => {
            let review = &p["review"];
            let verb = match review["state"].as_str() {
                Some("approved") => "approved",
                Some("changes_requested") => "requested changes on",
                // the comments of such a review arrive one by one, as pull_request_review_comment
                _ if review["body"].as_str().unwrap_or_default().trim().is_empty() => return None,
                _ => "reviewed",
            };
            m.text(&format!("{sender} ")).link(verb, &review["html_url"]).text(" ");
            numbered(&mut m, "pull request", &p["pull_request"]);
            m.quote(&review["body"], 500);
        }
        ("issue_comment" | "pull_request_review_comment" | "commit_comment", "created") => {
            m.text(&format!("{sender} "))
                .link("commented", &p["comment"]["html_url"])
                .text(" on ");
            match event {
                "issue_comment" if p["issue"]["pull_request"].is_object() => {
                    numbered(&mut m, "pull request", &p["issue"])
                }
                "issue_comment" => numbered(&mut m, "issue", &p["issue"]),
                "pull_request_review_comment" => {
                    if let Some(path) = p["comment"]["path"].as_str() {
                        m.code(path).text(" in ");
                    }
                    numbered(&mut m, "pull request", &p["pull_request"]);
                }
                _ => {
                    let id = p["comment"]["commit_id"].as_str().unwrap_or_default();
                    m.text("commit ").code(id.get(..8).unwrap_or(id));
                }
            }
            m.quote(&p["comment"]["body"], 500);
        }
        ("issue_comment" | "pull_request_review_comment" | "commit_comment", _) => return None,
        ("issues", "edited") => return None,
        ("issues", _) => {
            let issue = &p["issue"];
            m.text(&format!("{sender} "));
            match action {
                "assigned" => m.text(&format!(
                    "assigned {} to ",
                    p["assignee"]["login"].as_str().unwrap_or("someone")
                )),
                "unassigned" => m.text(&format!(
                    "unassigned {} from ",
                    p["assignee"]["login"].as_str().unwrap_or("someone")
                )),
                "labeled" => label(&mut m, "added", &p["label"], "to"),
                "unlabeled" => label(&mut m, "removed", &p["label"], "from"),
                _ => m.text(&format!("{} ", action.replace('_', " "))),
            };
            numbered(&mut m, "issue", issue);
            if action == "opened" {
                labels(&mut m, &issue["labels"]);
                m.quote(&issue["body"], 500);
            }
        }
        ("release", "published") => {
            let release = &p["release"];
            let tag = release["tag_name"].as_str().unwrap_or_default();
            let name = release["name"].as_str().filter(|name| !name.is_empty() && *name != tag);
            let shown = match name {
                Some(name) => format!("{tag}: {name}"),
                None => tag.to_string(),
            };
            m.text(&format!("{sender} published "))
                .link(&shown, &release["html_url"]);
            m.quote(&release["body"], 300);
        }
        ("release", _) => return None,
        ("workflow_run", "completed") => {
            let run = &p["workflow_run"];
            let conclusion = run["conclusion"].as_str().unwrap_or_default();
            if matches!(conclusion, "failure" | "startup_failure" | "timed_out") {
                m.alert();
            }
            let name = run["name"].as_str().unwrap_or("workflow");
            let branch = run["head_branch"].as_str().unwrap_or_default();
            let verb = match conclusion {
                "success" => "passed".to_string(),
                "failure" => "failed".to_string(),
                "startup_failure" => "failed to start".to_string(),
                "timed_out" => "timed out".to_string(),
                "cancelled" => "was cancelled".to_string(),
                "skipped" => "was skipped".to_string(),
                "action_required" => "needs an action".to_string(),
                other => format!("finished ({other})"),
            };
            m.text(&format!("workflow {name} ")).link(&verb, &run["html_url"]);
            m.text(" on ").code(branch);
            if let Some(seconds) = elapsed(&run["run_started_at"], &run["updated_at"]) {
                m.text(&format!(" in {}", duration(seconds)));
            }
        }
        ("workflow_run", _) => return None,
        ("create" | "delete", _) => {
            let kind = p["ref_type"].as_str().unwrap_or("ref");
            let reference = p["ref"].as_str().unwrap_or_default();
            let verb = if event == "create" { "created" } else { "deleted" };
            m.text(&format!("{sender} {verb} {kind} ")).code(reference);
        }
        ("watch", "started") => {
            m.text(&format!("{sender} starred the repository"));
        }
        ("fork", _) => {
            m.text(&format!("{sender} forked the repository into "));
            m.link(
                p["forkee"]["full_name"].as_str().unwrap_or("a fork"),
                &p["forkee"]["html_url"],
            );
        }
        ("repository", _) => {
            m.text(&format!("{sender} {} the repository", action.replace('_', " ")));
        }
        ("member", _) => {
            let member = p["member"]["login"].as_str().unwrap_or("someone");
            m.text(&format!("{sender} {action} {member} as a collaborator"));
        }
        _ => {
            let what = if action.is_empty() {
                event.to_string()
            } else {
                format!("{event}.{action}")
            };
            m.text(&format!("{sender} ")).code(&what);
        }
    }
    Some(m.done())
}

/// The reaction of a workflow run on its push, `🟢 build`, and whether the run is over; None for runs not started.
pub fn ci(p: &Value) -> Option<(String, bool)> {
    let run = &p["workflow_run"];
    let name = run["name"].as_str().unwrap_or("workflow");
    let state = match (p["action"].as_str()?, run["conclusion"].as_str()) {
        ("requested", _) => "🟡",
        ("in_progress", _) => "🔵",
        ("completed", Some("success")) => "🟢",
        ("completed", Some("failure" | "startup_failure")) => "🔴",
        ("completed", Some("cancelled")) => "⚫",
        ("completed", Some("timed_out")) => "⏱️",
        ("completed", Some("skipped")) => "⏩",
        ("completed", Some("action_required")) => "⚠️",
        ("completed", _) => "⚪",
        _ => return None,
    };
    Some((format!("{state} {name}"), p["action"] == "completed"))
}

/// `pull request #12: Title`, the reference linked.
fn numbered(m: &mut Line, kind: &str, item: &Value) {
    let number = item["number"].as_u64().unwrap_or_default();
    m.link(&format!("{kind} #{number}"), &item["html_url"]);
    m.text(&format!(": {}", item["title"].as_str().unwrap_or_default()));
}

fn label<'a>(m: &'a mut Line, verb: &str, label: &Value, preposition: &str) -> &'a mut Line {
    m.text(&format!("{verb} "))
        .pill(
            label["name"].as_str().unwrap_or_default(),
            label["color"].as_str(),
            label["description"].as_str(),
        )
        .text(&format!(" {preposition} "))
}

/// The labels of a new issue or pull request, after its title.
fn labels(m: &mut Line, labels: &Value) {
    for label in labels.as_array().into_iter().flatten() {
        m.text(" ").pill(
            label["name"].as_str().unwrap_or_default(),
            label["color"].as_str(),
            label["description"].as_str(),
        );
    }
}

/// The first line of a commit message, at most 72 characters; `[…]` when there is more.
fn title(message: &Value) -> String {
    let message = message.as_str().unwrap_or_default();
    let first = message.lines().next().unwrap_or_default().trim();
    let mut title: String = first.chars().take(72).collect();
    if title.len() < first.len() || message.trim().lines().count() > 1 {
        title.push_str(" […]");
    }
    title
}

/// Seconds between two timestamps of GitHub (`2026-11-07T10:31:04Z`).
fn elapsed(from: &Value, to: &Value) -> Option<i64> {
    let seconds = |stamp: &Value| -> Option<i64> {
        let stamp = stamp.as_str()?;
        let number = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<i64>().ok();
        let (y, mo, d) = (number(0..4)?, number(5..7)?, number(8..10)?);
        let (h, mi, s) = (number(11..13)?, number(14..16)?, number(17..19)?);
        // days since 1970-01-01 of a civil date (Howard Hinnant's algorithm)
        let y = if mo <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146097 + doe - 719468;
        Some(days * 86400 + h * 3600 + mi * 60 + s)
    };
    let elapsed = seconds(to)? - seconds(from)?;
    (elapsed >= 0).then_some(elapsed)
}

fn duration(seconds: i64) -> String {
    match seconds {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::{ci, message};
    use serde_json::json;

    fn repository() -> serde_json::Value {
        json!({"name": "widgets", "full_name": "acme/widgets", "html_url": "https://github.com/acme/widgets"})
    }

    #[test]
    fn a_push_lists_its_commits_and_a_branch_push_without_commits_is_left_to_create() {
        let push = json!({
            "ref": "refs/heads/main",
            "compare": "https://github.com/acme/widgets/compare/a...b",
            "pusher": {"name": "alice"},
            "repository": repository(),
            "commits": [
                {"id": "0123456789abcdef", "message": "Fix <login>\n\nbody", "url": "https://c/1",
                 "author": {"username": "alice"}},
                {"id": "fedcba9876543210", "message": "Checkout", "url": "https://c/2",
                 "author": {"username": "dave"}},
            ],
        });
        let m = message("push", &push).unwrap();
        assert_eq!(
            m.text,
            "[acme/widgets] alice pushed 2 commits to main:\n- 01234567 Fix <login> […]\n- fedcba98 Checkout by dave"
        );
        assert!(
            m.html
                .contains("<li><a href=\"https://c/1\"><code>01234567</code></a> Fix &lt;login&gt; […]</li>")
        );
        assert!(!m.alert);
        let created = json!({"ref": "refs/heads/feature", "created": true, "commits": [], "repository": repository()});
        assert_eq!(message("push", &created), None);
    }

    #[test]
    fn a_pull_request_reads_as_a_sentence_with_its_branches_labels_and_description() {
        let pr = json!({
            "action": "opened",
            "sender": {"login": "dave"},
            "pull_request": {"number": 12, "title": "Checkout", "html_url": "https://pr/12",
                             "head": {"ref": "checkout"}, "base": {"ref": "main"},
                             "labels": [{"name": "feature", "color": "a2eeef"}],
                             "body": "Adds the **checkout**."},
            "repository": repository(),
        });
        let m = message("pull_request", &pr).unwrap();
        assert_eq!(
            m.text,
            "[acme/widgets] dave opened pull request #12: Checkout (checkout into main) [feature]\n> Adds the **checkout**."
        );
        assert!(
            m.html
                .contains("<blockquote><p>Adds the <strong>checkout</strong>.</p></blockquote>")
        );
        let merged = json!({"action": "closed", "sender": {"login": "alice"},
            "pull_request": {"number": 12, "title": "Checkout", "merged": true, "merge_commit_sha": "abc"},
            "repository": repository()});
        let m = message("pull_request", &merged).unwrap();
        assert_eq!(m.text, "[acme/widgets] alice merged pull request #12: Checkout");
        assert!(
            m.html
                .contains("<a href=\"https://github.com/acme/widgets/commit/abc\">merged</a>")
        );
    }

    #[test]
    fn workflow_runs_become_reactions_and_a_failure_without_its_push_alerts() {
        let run = |action: &str, conclusion: Option<&str>| {
            json!({"action": action, "repository": repository(),
                   "workflow_run": {"name": "build", "conclusion": conclusion, "head_branch": "main",
                                    "html_url": "https://run",
                                    "run_started_at": "2026-11-07T23:58:30Z", "updated_at": "2026-11-08T00:01:42Z"}})
        };
        assert_eq!(ci(&run("in_progress", None)), Some(("🔵 build".to_string(), false)));
        assert_eq!(
            ci(&run("completed", Some("success"))),
            Some(("🟢 build".to_string(), true))
        );
        let failed = message("workflow_run", &run("completed", Some("failure"))).unwrap();
        assert_eq!(failed.text, "[acme/widgets] workflow build failed on main in 3m 12s");
        assert!(failed.alert);
    }

    #[test]
    fn events_without_a_format_get_a_generic_line_and_edits_nothing() {
        let member = json!({"action": "added", "sender": {"login": "alice"}, "organization": {"login": "acme"}});
        assert_eq!(
            message("organization", &member).unwrap().text,
            "[acme] alice organization.added"
        );
        assert_eq!(message("ping", &json!({})), None);
        assert_eq!(message("issues", &json!({"action": "edited"})), None);
    }
}
