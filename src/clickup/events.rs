use crate::message::{Line, Message};
use serde_json::Value;

/// The message of a task change, with the task as the API gives it; None for the noise of time tracking.
pub fn message(event: &str, payload: &Value, task: &Value) -> Option<Message> {
    let item = &payload["history_items"][0];
    let user = item["user"]["username"].as_str().unwrap_or("someone");
    let mut m = Line::default();
    match event {
        "taskTimeTrackedUpdated" | "taskTimeEstimateUpdated" => return None,
        "taskCreated" => {
            m.text("🆕 ").bold(user).text(" created ");
            named(&mut m, task);
        }
        "taskStatusUpdated" => {
            let before = item["before"]["status"].as_str().unwrap_or("?");
            let after = item["after"]["status"]
                .as_str()
                .or(task["status"]["status"].as_str())
                .unwrap_or("?");
            m.text("🔄 ").bold(user).text(" moved ");
            named(&mut m, task);
            m.text(" from ").code(before).text(" to ").code(after);
        }
        "taskAssigneeUpdated" => {
            match (item["after"]["username"].as_str(), item["before"]["username"].as_str()) {
                (Some(assignee), _) => m.text("👤 ").bold(user).text(" assigned ").bold(assignee).text(" to "),
                (None, Some(removed)) => m.text("👤 ").bold(user).text(" took ").bold(removed).text(" off "),
                _ => m.text("👤 ").bold(user).text(" changed the assignees of "),
            };
            named(&mut m, task);
        }
        "taskCommentPosted" => {
            m.text("💬 ").bold(user).text(" commented on ");
            named(&mut m, task);
            m.quote(&item["comment"]["text_content"], 3);
        }
        "taskDueDateUpdated" => {
            m.text("📅 ").bold(user).text(" changed the due date of ");
            named(&mut m, task);
        }
        "taskPriorityUpdated" => {
            let priority = item["after"]["priority"].as_str().unwrap_or("none");
            m.text("⚡ ").bold(user).text(" set the priority of ");
            named(&mut m, task);
            m.text(" to ").code(priority);
        }
        "taskMoved" => {
            m.text("📦 ").bold(user).text(" moved ");
            named(&mut m, task);
        }
        "taskUpdated" => {
            let field = item["field"].as_str().unwrap_or("details");
            m.text("✏️ ")
                .bold(user)
                .text(&format!(" changed the {} of ", field.replace('_', " ")));
            named(&mut m, task);
        }
        _ => {
            m.text("ℹ️ ").bold(user).text(" ").code(event).text(" on ");
            named(&mut m, task);
        }
    }
    Some(m.done())
}

/// The task, bold and linked, and where it is.
fn named(m: &mut Line, task: &Value) {
    m.bold_link(task["name"].as_str().unwrap_or("a task"), &task["url"]);
    if let Some(list) = task["list"]["name"].as_str() {
        m.text(" in ").bold(list);
    }
}

#[cfg(test)]
mod tests {
    use super::message;
    use serde_json::json;

    fn task() -> serde_json::Value {
        json!({"name": "Checkout", "url": "https://app.clickup.com/t/1vj37mc", "list": {"name": "Widgets Sprint"},
               "status": {"status": "in progress"}})
    }

    #[test]
    fn a_status_change_says_from_where_to_where() {
        let payload = json!({"history_items": [{"user": {"username": "Dave Doe"},
            "before": {"status": "to do"}, "after": {"status": "in progress"}}]});
        let m = message("taskStatusUpdated", &payload, &task()).unwrap();
        assert_eq!(
            m.text,
            "🔄 Dave Doe moved Checkout in Widgets Sprint from to do to in progress"
        );
        assert!(
            m.html
                .contains("<a href=\"https://app.clickup.com/t/1vj37mc\"><b>Checkout</b></a>")
        );
    }

    #[test]
    fn a_comment_is_quoted_and_time_tracking_is_left_out() {
        let payload = json!({"history_items": [{"user": {"username": "Carol Roe"},
            "comment": {"text_content": "Done on staging.\nCheck it please."}}]});
        assert_eq!(
            message("taskCommentPosted", &payload, &task()).unwrap().text,
            "💬 Carol Roe commented on Checkout in Widgets Sprint\n> Done on staging.\n> Check it please."
        );
        assert_eq!(message("taskTimeTrackedUpdated", &payload, &task()), None);
    }
}
