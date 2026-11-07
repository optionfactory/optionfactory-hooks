use crate::message::{Line, Message};
use serde_json::Value;

/// The message of an event, or None for what another event already tells (the branch a push creates comes as
/// `create`, the push to a pull request as `push`), what is not finished yet (a workflow run) and the noise of edits.
pub fn message(event: &str, p: &Value) -> Option<Message> {
    let mut m = Line::default();
    let sender = p["sender"]["login"].as_str().unwrap_or("someone");
    let action = p["action"].as_str().unwrap_or_default();
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
            if p["forced"] == true {
                m.text("⚠️ ").bold(pusher).text(" force-pushed ");
            } else {
                m.text("⬆️ ").bold(pusher).text(" pushed ");
            }
            m.link(&count, &p["compare"]).text(" to ").code(branch).text(" of ");
            repository(&mut m, p);
            for commit in commits.iter().take(5) {
                let id = commit["id"].as_str().unwrap_or_default();
                let title = first_line(&commit["message"]);
                m.break_line()
                    .text("• ")
                    .code_link(id.get(..7).unwrap_or(id), &commit["url"]);
                m.text(&format!(" {title}"));
                let author = commit["author"]["username"]
                    .as_str()
                    .or(commit["author"]["name"].as_str());
                if let Some(author) = author.filter(|author| *author != pusher) {
                    m.text(&format!(" ({author})"));
                }
            }
            if commits.len() > 5 {
                m.break_line().text(&format!("• and {} more", commits.len() - 5));
            }
        }
        ("pull_request", "synchronize" | "edited") => return None,
        ("pull_request", _) => {
            let pr = &p["pull_request"];
            match action {
                "opened" => m.text("🔀 ").bold(sender).text(" opened "),
                "closed" if pr["merged"] == true => m.text("🟣 ").bold(sender).text(" merged "),
                "closed" => m.text("❌ ").bold(sender).text(" closed "),
                "reopened" => m.text("🔁 ").bold(sender).text(" reopened "),
                "ready_for_review" => m.text("📝 ").bold(sender).text(" marked ready for review "),
                "converted_to_draft" => m.text("📝 ").bold(sender).text(" turned into a draft "),
                "review_requested" => {
                    let reviewer = p["requested_reviewer"]["login"]
                        .as_str()
                        .or(p["requested_team"]["name"].as_str())
                        .unwrap_or("someone");
                    m.text("👀 ")
                        .bold(sender)
                        .text(" asked ")
                        .bold(reviewer)
                        .text(" to review ")
                }
                "assigned" => {
                    let assignee = p["assignee"]["login"].as_str().unwrap_or("someone");
                    m.text("👤 ")
                        .bold(sender)
                        .text(" assigned ")
                        .bold(assignee)
                        .text(" to ")
                }
                "labeled" => {
                    let label = p["label"]["name"].as_str().unwrap_or_default();
                    m.text("🏷️ ").bold(sender).text(" labeled ").code(label).text(" ")
                }
                _ => m
                    .text("🔀 ")
                    .bold(sender)
                    .text(&format!(" {} ", action.replace('_', " "))),
            };
            numbered(&mut m, pr);
            m.text(" in ");
            repository(&mut m, p);
            if action == "opened" {
                let head = pr["head"]["ref"].as_str().unwrap_or_default();
                let base = pr["base"]["ref"].as_str().unwrap_or_default();
                m.break_line().code(head).text(" → ").code(base);
                if let (Some(added), Some(removed), Some(files)) = (
                    pr["additions"].as_u64(),
                    pr["deletions"].as_u64(),
                    pr["changed_files"].as_u64(),
                ) {
                    m.text(&format!(" · +{added} −{removed} in {files} files"));
                }
                m.quote(&pr["body"], 3);
            }
        }
        ("pull_request_review", "submitted") => {
            let review = &p["review"];
            match review["state"].as_str() {
                Some("approved") => m.text("✅ ").bold(sender).text(" approved "),
                Some("changes_requested") => m.text("🛑 ").bold(sender).text(" requested changes on "),
                // the comments of such a review arrive one by one, as pull_request_review_comment
                _ if review["body"].as_str().unwrap_or_default().trim().is_empty() => return None,
                _ => m.text("💬 ").bold(sender).text(" reviewed "),
            };
            numbered(&mut m, &p["pull_request"]);
            m.text(" in ");
            repository(&mut m, p);
            m.quote(&review["body"], 3);
        }
        ("issue_comment" | "pull_request_review_comment" | "commit_comment", "created") => {
            m.text("💬 ")
                .bold(sender)
                .text(" ")
                .link("commented", &p["comment"]["html_url"])
                .text(" on ");
            match event {
                "issue_comment" => numbered(&mut m, &p["issue"]),
                "pull_request_review_comment" => {
                    if let Some(path) = p["comment"]["path"].as_str() {
                        m.code(path).text(" in ");
                    }
                    numbered(&mut m, &p["pull_request"]);
                }
                _ => {
                    let id = p["comment"]["commit_id"].as_str().unwrap_or_default();
                    m.text("commit ").code(id.get(..7).unwrap_or(id));
                }
            }
            m.text(" in ");
            repository(&mut m, p);
            m.quote(&p["comment"]["body"], 3);
        }
        ("issue_comment" | "pull_request_review_comment" | "commit_comment", _) => return None,
        ("issues", "edited") => return None,
        ("issues", _) => {
            let issue = &p["issue"];
            match action {
                "opened" => m.text("📌 ").bold(sender).text(" opened "),
                "closed" => m.text("✔️ ").bold(sender).text(" closed "),
                "reopened" => m.text("🔁 ").bold(sender).text(" reopened "),
                "assigned" => {
                    let assignee = p["assignee"]["login"].as_str().unwrap_or("someone");
                    m.text("👤 ")
                        .bold(sender)
                        .text(" assigned ")
                        .bold(assignee)
                        .text(" to ")
                }
                "labeled" => {
                    let label = p["label"]["name"].as_str().unwrap_or_default();
                    m.text("🏷️ ").bold(sender).text(" labeled ").code(label).text(" ")
                }
                _ => m
                    .text("📌 ")
                    .bold(sender)
                    .text(&format!(" {} ", action.replace('_', " "))),
            };
            numbered(&mut m, issue);
            m.text(" in ");
            repository(&mut m, p);
            if action == "opened" {
                m.quote(&issue["body"], 3);
            }
        }
        ("release", "published") => {
            let release = &p["release"];
            let tag = release["tag_name"].as_str().unwrap_or_default();
            let name = release["name"].as_str().filter(|name| !name.is_empty()).unwrap_or(tag);
            m.text("🚀 ")
                .bold(sender)
                .text(" released ")
                .bold_link(name, &release["html_url"]);
            if name != tag {
                m.text(" (").code(tag).text(")");
            }
            m.text(" of ");
            repository(&mut m, p);
            m.quote(&release["body"], 3);
        }
        ("release", _) => return None,
        ("workflow_run", "completed") => {
            let run = &p["workflow_run"];
            let conclusion = run["conclusion"].as_str().unwrap_or_default();
            let (icon, verb) = match conclusion {
                "success" => ("✅", "passed"),
                "failure" => ("❌", "failed"),
                "startup_failure" => ("❌", "failed to start"),
                "timed_out" => ("⏱️", "timed out"),
                "cancelled" => ("⚪", "was cancelled"),
                "skipped" => ("⏭️", "was skipped"),
                "action_required" => ("✋", "needs an action"),
                _ => ("⚪", "finished"),
            };
            if matches!(conclusion, "failure" | "startup_failure" | "timed_out") {
                m.alert();
            }
            let name = run["name"].as_str().unwrap_or("workflow");
            let branch = run["head_branch"].as_str().unwrap_or_default();
            m.text(&format!("{icon} "))
                .bold_link(name, &run["html_url"])
                .text(&format!(" {verb} on "));
            m.code(branch).text(" of ");
            repository(&mut m, p);
            if let Some(seconds) = elapsed(&run["run_started_at"], &run["updated_at"]) {
                m.text(&format!(" in {}", duration(seconds)));
            }
            let actor = run["triggering_actor"]["login"]
                .as_str()
                .or(run["actor"]["login"].as_str());
            let title = first_line(&run["head_commit"]["message"]);
            m.break_line();
            if let Some(number) = run["run_number"].as_u64() {
                m.text(&format!("run #{number}"));
            }
            if let Some(actor) = actor {
                m.text(" · by ").bold(actor);
            }
            if !title.is_empty() {
                m.text(" · ").text(title);
            }
        }
        ("workflow_run", _) => return None,
        ("create" | "delete", _) => {
            let kind = p["ref_type"].as_str().unwrap_or("ref");
            let reference = p["ref"].as_str().unwrap_or_default();
            let (icon, verb) = match (event, kind) {
                ("create", "tag") => ("🏷️", "tagged"),
                ("create", _) => ("🌱", "created branch"),
                _ => ("🗑️", if kind == "tag" { "deleted tag" } else { "deleted branch" }),
            };
            m.text(&format!("{icon} "))
                .bold(sender)
                .text(&format!(" {verb} "))
                .code(reference);
            m.text(" in ");
            repository(&mut m, p);
        }
        ("watch", "started") => {
            m.text("⭐ ").bold(sender).text(" starred ");
            repository(&mut m, p);
        }
        ("fork", _) => {
            m.text("🍴 ").bold(sender).text(" forked ");
            repository(&mut m, p);
            m.text(" into ").link(
                p["forkee"]["full_name"].as_str().unwrap_or("a fork"),
                &p["forkee"]["html_url"],
            );
        }
        ("repository", _) => {
            m.text("📦 ")
                .bold(sender)
                .text(&format!(" {} the repository ", action.replace('_', " ")));
            repository(&mut m, p);
        }
        ("member", _) => {
            let member = p["member"]["login"].as_str().unwrap_or("someone");
            m.text("👋 ")
                .bold(sender)
                .text(&format!(" {action} "))
                .bold(member)
                .text(" in ");
            repository(&mut m, p);
        }
        _ => {
            let what = if action.is_empty() {
                event.to_string()
            } else {
                format!("{event}.{action}")
            };
            m.text("ℹ️ ").bold(sender).text(" ").code(&what).text(" in ");
            if p["repository"].is_object() {
                repository(&mut m, p);
            } else {
                m.bold(p["organization"]["login"].as_str().unwrap_or("the organization"));
            }
        }
    }
    Some(m.done())
}

/// The repository, bold and linked, without the owner: the rooms are of one organization.
fn repository(m: &mut Line, p: &Value) {
    let name = p["repository"]["name"]
        .as_str()
        .or(p["repository"]["full_name"].as_str())
        .unwrap_or("a repository");
    m.bold_link(name, &p["repository"]["html_url"]);
}

/// `#12 Title` of an issue or a pull request, linked.
fn numbered(m: &mut Line, item: &Value) {
    let number = item["number"].as_u64().unwrap_or_default();
    let title = item["title"].as_str().unwrap_or_default();
    m.link(&format!("#{number} {title}"), &item["html_url"]);
}

fn first_line(text: &Value) -> &str {
    text.as_str()
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
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
    use super::message;
    use serde_json::json;

    fn repository() -> serde_json::Value {
        json!({"name": "widgets", "full_name": "acme/widgets",
               "html_url": "https://github.com/acme/widgets"})
    }

    #[test]
    fn a_push_lists_its_commits_and_a_branch_push_without_commits_is_left_to_create() {
        let push = json!({
            "ref": "refs/heads/master",
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
            "⬆️ alice pushed 2 commits to master of widgets\n• 0123456 Fix <login>\n• fedcba9 Checkout (dave)"
        );
        assert!(
            m.html
                .contains("<a href=\"https://c/1\"><code>0123456</code></a> Fix &lt;login&gt;")
        );
        assert!(!m.alert);
        let created = json!({"ref": "refs/heads/feature", "created": true, "commits": [], "repository": repository()});
        assert_eq!(message("push", &created), None);
    }

    #[test]
    fn an_opened_pull_request_shows_its_branches_size_and_description() {
        let pr = json!({
            "action": "opened",
            "sender": {"login": "dave"},
            "pull_request": {"number": 12, "title": "Checkout", "html_url": "https://pr/12",
                             "head": {"ref": "checkout"}, "base": {"ref": "master"},
                             "additions": 120, "deletions": 30, "changed_files": 4,
                             "body": "Adds the checkout.\nSee #11."},
            "repository": repository(),
        });
        assert_eq!(
            message("pull_request", &pr).unwrap().text,
            "🔀 dave opened #12 Checkout in widgets\ncheckout → master · +120 −30 in 4 files\n> Adds the checkout.\n> See #11."
        );
        let merged = json!({"action": "closed", "sender": {"login": "alice"},
            "pull_request": {"number": 12, "title": "Checkout", "merged": true}, "repository": repository()});
        assert_eq!(
            message("pull_request", &merged).unwrap().text,
            "🟣 alice merged #12 Checkout in widgets"
        );
    }

    #[test]
    fn a_failed_run_alerts_with_its_duration() {
        let run = json!({
            "action": "completed",
            "workflow_run": {"name": "build", "conclusion": "failure", "head_branch": "master",
                             "html_url": "https://run", "run_number": 42,
                             "run_started_at": "2026-11-07T23:58:30Z", "updated_at": "2026-11-08T00:01:42Z",
                             "triggering_actor": {"login": "alice"},
                             "head_commit": {"message": "Fix the build\n\nbody"}},
            "repository": repository(),
        });
        let m = message("workflow_run", &run).unwrap();
        assert_eq!(
            m.text,
            "❌ build failed on master of widgets in 3m 12s\nrun #42 · by alice · Fix the build"
        );
        assert!(m.alert);
        assert_eq!(message("workflow_run", &json!({"action": "requested"})), None);
    }

    #[test]
    fn events_without_a_format_get_a_generic_line_and_edits_nothing() {
        let member = json!({"action": "added", "sender": {"login": "alice"}, "organization": {"login": "acme"}});
        assert_eq!(
            message("organization", &member).unwrap().text,
            "ℹ️ alice organization.added in acme"
        );
        assert_eq!(message("ping", &json!({})), None);
        assert_eq!(message("issues", &json!({"action": "edited"})), None);
    }
}
