use super::{BOT, CONNECTION, Clickup, Connection, SUBSCRIPTIONS, USAGE, api_delete, api_get, api_post};
use crate::appservice::Command;
use crate::matrix::moderator;
use serde_json::json;

/// A message starting with `!clickup` in a room of the bot; blocking. The answer goes to the same room.
pub fn command(clickup: &Clickup, command: &Command) {
    let words: Vec<&str> = command.body.split_whitespace().collect();
    let answer = match words.get(1).copied() {
        Some("connect") => connect(clickup, command, &words[2..]),
        Some("disconnect") => disconnect(clickup, command),
        Some("subscribe") => subscribe(clickup, command, &words[2..].join(" ")),
        Some("unsubscribe") => unsubscribe(clickup, command, &words[2..].join(" ")),
        Some("list") => Ok(list(clickup, &command.room)),
        _ => Ok(USAGE.to_string()),
    };
    let answer = answer.unwrap_or_else(|e| {
        eprintln!("clickup: {e}");
        "That did not work: the log of the service has the reason.".to_string()
    });
    if let Err(e) = clickup
        .matrix
        .send(&command.room, BOT, &json!({"msgtype": "m.notice", "body": answer}))
    {
        eprintln!("clickup: {e}");
    }
}

/// Creates the webhook of the workspace with the personal API token of an admin, sent in a room with the bot alone.
fn connect(clickup: &Clickup, command: &Command, args: &[&str]) -> Result<String, String> {
    if !clickup.admins.contains(&command.sender) {
        return Ok("Only the admins of the service can do that.".to_string());
    }
    if !clickup.matrix.alone_with(&command.room, BOT, &command.sender)? {
        return Ok(
            "Send the token only in a room with you and me alone: delete that message, create a room \
            without encryption and invite me there."
                .to_string(),
        );
    }
    // the message carries the token: out of the room before anything else, if the bot may
    let redacted = clickup.matrix.redact(&command.room, &command.event_id, BOT).is_ok();
    let delete_it = if redacted {
        ""
    } else {
        " I cannot remove your message with the token: delete it yourself."
    };
    if clickup.connection().is_some() {
        return Ok(format!(
            "A workspace is connected already: `!clickup disconnect` first.{delete_it}"
        ));
    }
    let (token, team) = match args {
        [token] => (*token, None),
        [token, team] => (*token, Some(*team)),
        _ => return Ok(format!("{USAGE}{delete_it}")),
    };
    let teams = api_get(token, "team")?.unwrap_or_default();
    let teams = teams["teams"].as_array().cloned().unwrap_or_default();
    let chosen = match team {
        Some(team) => teams.iter().find(|t| t["id"] == team),
        None if teams.len() == 1 => teams.first(),
        None => {
            let names: Vec<String> = teams
                .iter()
                .map(|t| {
                    format!(
                        "{} ({})",
                        t["name"].as_str().unwrap_or_default(),
                        t["id"].as_str().unwrap_or_default()
                    )
                })
                .collect();
            return Ok(format!(
                "The token reaches several workspaces: {}. `!clickup connect <API token> <workspace id>`.{delete_it}",
                names.join(", ")
            ));
        }
    };
    let Some(chosen) = chosen else {
        return Ok(format!("The token reaches no such workspace.{delete_it}"));
    };
    let team = chosen["id"].as_str().unwrap_or_default().to_string();
    let team_name = chosen["name"].as_str().unwrap_or_default().to_string();
    let endpoint = format!("{}/clickup", clickup.public_url);
    let created = api_post(
        token,
        &format!("team/{team}/webhook"),
        &json!({"endpoint": endpoint, "events": ["*"]}),
    )?;
    let (Some(webhook), Some(secret)) = (created["id"].as_str(), created["webhook"]["secret"].as_str()) else {
        return Err(format!("webhook of {team}: unexpected answer {created}"));
    };
    let connection = Connection {
        token: token.to_string(),
        team,
        team_name: team_name.clone(),
        webhook: webhook.to_string(),
        secret: secret.to_string(),
    };
    clickup.matrix.set_account_data(
        BOT,
        None,
        CONNECTION,
        &serde_json::to_value(&connection).map_err(|e| e.to_string())?,
    )?;
    *clickup.connection.write().unwrap_or_else(|e| e.into_inner()) = Some(connection);
    Ok(format!(
        "Connected to the workspace {team_name}: its task changes reach the rooms that subscribe.{delete_it}"
    ))
}

fn disconnect(clickup: &Clickup, command: &Command) -> Result<String, String> {
    if !clickup.admins.contains(&command.sender) {
        return Ok("Only the admins of the service can do that.".to_string());
    }
    let Some(connection) = clickup.connection() else {
        return Ok("No workspace is connected.".to_string());
    };
    api_delete(&connection.token, &format!("webhook/{}", connection.webhook))?;
    clickup.matrix.set_account_data(BOT, None, CONNECTION, &json!({}))?;
    *clickup.connection.write().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(format!(
        "Disconnected from {}: the webhook is gone. Revoke the API token in ClickUp if it should not be used any more.",
        connection.team_name
    ))
}

fn subscribe(clickup: &Clickup, command: &Command, target: &str) -> Result<String, String> {
    if !moderator(&clickup.matrix.state(&command.room, BOT)?, &command.sender) {
        return Ok("Only the moderators of the room can change its subscriptions.".to_string());
    }
    let target = target.trim().trim_matches('"');
    if target.is_empty() {
        return Ok(USAGE.to_string());
    }
    let mut targets = current(clickup, &command.room);
    if targets.iter().any(|t| t.eq_ignore_ascii_case(target)) {
        return Ok(format!("This room follows {target} already."));
    }
    targets.push(target.to_string());
    save(clickup, &command.room, targets)?;
    let pending = if clickup.connection().is_some() {
        ""
    } else {
        " No workspace is connected yet: an admin runs `!clickup connect`."
    };
    Ok(format!(
        "This room gets the task changes of {target} (a list or folder by id or name, or a space by id).{pending}"
    ))
}

fn unsubscribe(clickup: &Clickup, command: &Command, target: &str) -> Result<String, String> {
    if !moderator(&clickup.matrix.state(&command.room, BOT)?, &command.sender) {
        return Ok("Only the moderators of the room can change its subscriptions.".to_string());
    }
    let target = target.trim().trim_matches('"');
    let mut targets = current(clickup, &command.room);
    let before = targets.len();
    targets.retain(|t| !t.eq_ignore_ascii_case(target));
    if targets.len() == before {
        return Ok(format!("This room does not follow {target}."));
    }
    save(clickup, &command.room, targets)?;
    Ok(format!("This room no longer gets the task changes of {target}."))
}

fn list(clickup: &Clickup, room: &str) -> String {
    let targets = current(clickup, room);
    if targets.is_empty() {
        return "This room follows nothing in ClickUp.".to_string();
    }
    format!("This room follows: {}.", targets.join(", "))
}

fn current(clickup: &Clickup, room: &str) -> Vec<String> {
    let subscriptions = clickup.subscriptions.read().unwrap_or_else(|e| e.into_inner());
    subscriptions.get(room).cloned().unwrap_or_default()
}

/// Stored first, then taken into use: the lock is never held during a call to the homeserver.
fn save(clickup: &Clickup, room: &str, targets: Vec<String>) -> Result<(), String> {
    clickup
        .matrix
        .set_account_data(BOT, Some(room), SUBSCRIPTIONS, &json!({"subscriptions": targets}))?;
    let mut all = clickup.subscriptions.write().unwrap_or_else(|e| e.into_inner());
    if targets.is_empty() {
        all.remove(room);
    } else {
        all.insert(room.to_string(), targets);
    }
    Ok(())
}
