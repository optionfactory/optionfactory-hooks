use crate::message::{Line, Message};
use serde_json::Value;

/// The events the webhook asks for and the bot tells. Left out as noise: edits (description, name, attachments,
/// custom fields, tags, comments), time tracking, deletions, and the changes to lists, folders, spaces and goals.
pub const EVENTS: [&str; 7] = [
    "taskCreated",
    "taskStatusUpdated",
    "taskAssigneeUpdated",
    "taskCommentPosted",
    "taskDueDateUpdated",
    "taskPriorityUpdated",
    "taskMoved",
];

/// The message of a task change, with the task as the API gives it: `[List] user verb task`, statuses in their
/// colours. None for the events outside `EVENTS` and for the first status of a new task, which `taskCreated` tells.
pub fn message(event: &str, payload: &Value, task: &Value) -> Option<Message> {
    let item = &payload["history_items"][0];
    let user = item["user"]["username"].as_str().unwrap_or("someone");
    let mut m = Line::default();
    m.prefix(task["list"]["name"].as_str().unwrap_or("ClickUp"), &Value::Null);
    match event {
        "taskCreated" => {
            m.text(&format!("{user} created "));
            named(&mut m, task);
        }
        "taskStatusUpdated" if !item["before"]["status"].is_string() => return None,
        "taskStatusUpdated" => {
            m.text(&format!("{user} moved "));
            named(&mut m, task);
            m.text(" from ");
            status(&mut m, &item["before"]);
            m.text(" to ");
            let after = if item["after"]["status"].is_string() {
                &item["after"]
            } else {
                &task["status"]
            };
            status(&mut m, after);
        }
        "taskAssigneeUpdated" => {
            match (item["after"]["username"].as_str(), item["before"]["username"].as_str()) {
                (Some(assignee), _) => m.text(&format!("{user} assigned {assignee} to ")),
                (None, Some(removed)) => m.text(&format!("{user} unassigned {removed} from ")),
                _ => m.text(&format!("{user} changed the assignees of ")),
            };
            named(&mut m, task);
        }
        "taskCommentPosted" => {
            m.text(&format!("{user} commented on "));
            named(&mut m, task);
            m.quote(&item["comment"]["text_content"], 500);
        }
        "taskDueDateUpdated" => {
            m.text(&format!("{user} changed the due date of "));
            named(&mut m, task);
        }
        "taskPriorityUpdated" => {
            let priority = &item["after"];
            m.text(&format!("{user} set the priority of "));
            named(&mut m, task);
            m.text(" to ");
            m.pill(
                priority["priority"].as_str().unwrap_or("none"),
                priority["color"].as_str(),
                None,
            );
        }
        "taskMoved" => {
            m.text(&format!("{user} moved "));
            named(&mut m, task);
            m.text(" here");
        }
        _ => return None,
    }
    Some(m.done())
}

/// `task Name`, the name linked.
fn named(m: &mut Line, task: &Value) {
    m.text("task ")
        .link(task["name"].as_str().unwrap_or("without a name"), &task["url"]);
}

/// A status as ClickUp shows it, in its colour.
fn status(m: &mut Line, status: &Value) {
    m.pill(status["status"].as_str().unwrap_or("?"), status["color"].as_str(), None);
}

#[cfg(test)]
mod tests {
    use super::message;
    use serde_json::json;

    fn task() -> serde_json::Value {
        json!({"name": "Checkout", "url": "https://app.clickup.com/t/1vj37mc", "list": {"name": "Widgets Sprint"},
               "status": {"status": "in progress", "color": "#4194f6"}})
    }

    #[test]
    fn a_status_change_shows_both_statuses_in_their_colours() {
        let payload = json!({"history_items": [{"user": {"username": "Dave Doe"},
            "before": {"status": "to do", "color": "#d3d3d3"}, "after": {"status": "in progress", "color": "#4194f6"}}]});
        let m = message("taskStatusUpdated", &payload, &task()).unwrap();
        assert_eq!(
            m.text,
            "[Widgets Sprint] Dave Doe moved task Checkout from [to do] to [in progress]"
        );
        assert!(m.html.starts_with("<b>[Widgets Sprint]</b> Dave Doe moved task <a href=\"https://app.clickup.com/t/1vj37mc\">Checkout</a> from <font data-mx-bg-color=\"#d3d3d3\""));
    }

    #[test]
    fn a_comment_is_quoted_and_time_tracking_is_left_out() {
        let payload = json!({"history_items": [{"user": {"username": "Carol Roe"},
            "comment": {"text_content": "Done on staging.\nCheck it please."}}]});
        assert_eq!(
            message("taskCommentPosted", &payload, &task()).unwrap().text,
            "[Widgets Sprint] Carol Roe commented on task Checkout\n> Done on staging.\n> Check it please."
        );
        assert_eq!(message("taskTimeTrackedUpdated", &payload, &task()), None);
    }

    #[test]
    fn a_new_task_is_told_once_and_edits_are_left_out() {
        let first_status = json!({"history_items": [{"user": {"username": "Dave Doe"}, "before": null,
            "after": {"status": "open", "color": "#d3d3d3"}}]});
        let description = json!({"history_items": [{"user": {"username": "Dave Doe"}, "field": "content"}]});
        assert_eq!(message("taskStatusUpdated", &first_status, &task()), None);
        assert_eq!(message("taskUpdated", &description, &task()), None);
    }
}
