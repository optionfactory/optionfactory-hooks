use crate::clickup::Clickup;
use crate::github::Github;
use crate::hooks::Hooks;
use crate::matrix::Matrix;
use crate::{clickup, github, hooks, profile};
use axum::Router;
use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{post, put};
use ring::digest::{SHA256, digest};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

struct Appservice {
    hs_token: String,
    matrix: Arc<Matrix>,
    bots: Bots,
}

/// The features behind the bots, and who may change the names and icons of the bots.
pub struct Bots {
    pub github: Arc<Github>,
    pub clickup: Arc<Clickup>,
    pub hooks: Arc<Hooks>,
    pub admins: Vec<String>,
}

#[derive(Deserialize)]
struct Transaction {
    #[serde(default)]
    events: Vec<Value>,
}

/// The side the homeserver calls: the room events that concern the bots.
pub fn transactions(hs_token: String, matrix: Arc<Matrix>, bots: Bots) -> Router {
    Router::new()
        .route("/_matrix/app/v1/transactions/{txn}", put(receive))
        .route("/_matrix/app/v1/ping", post(ping))
        .with_state(Arc::new(Appservice { hs_token, matrix, bots }))
}

async fn ping(State(appservice): State<Arc<Appservice>>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    if !from_homeserver(&headers, &appservice.hs_token) {
        return (StatusCode::FORBIDDEN, Json(json!({"errcode": "M_FORBIDDEN"})));
    }
    (StatusCode::OK, Json(json!({})))
}

async fn receive(
    State(appservice): State<Arc<Appservice>>,
    headers: HeaderMap,
    Json(transaction): Json<Transaction>,
) -> (StatusCode, Json<Value>) {
    if !from_homeserver(&headers, &appservice.hs_token) {
        return (StatusCode::FORBIDDEN, Json(json!({"errcode": "M_FORBIDDEN"})));
    }
    for event in transaction.events {
        if let Some((prefix, command)) = ["!github", "!clickup", "!hooks"]
            .into_iter()
            .find_map(|prefix| command(&event, &appservice.matrix, prefix).map(|command| (prefix, command)))
        {
            let appservice = appservice.clone();
            tokio::task::spawn_blocking(move || {
                let bots = &appservice.bots;
                match (prefix, command.body.split_whitespace().nth(1)) {
                    ("!hooks", _) => hooks::command(&bots.hooks, &command),
                    ("!github", Some("avatar" | "name")) => {
                        profile::command(&appservice.matrix, &bots.admins, github::BOT, &command)
                    }
                    ("!clickup", Some("avatar" | "name")) => {
                        profile::command(&appservice.matrix, &bots.admins, clickup::BOT, &command)
                    }
                    ("!github", _) => github::command(&bots.github, &command.room, &command.sender, &command.body),
                    _ => clickup::command(&bots.clickup, &command),
                }
            });
            continue;
        }
        if event["type"] == "m.room.encryption" {
            // the bots stay out of encrypted rooms, also of a room that turns encrypted with them in it
            let room = event["room_id"].as_str().unwrap_or_default().to_string();
            let appservice = appservice.clone();
            tokio::task::spawn_blocking(move || {
                let mut bots: Vec<String> = [github::BOT, clickup::BOT, hooks::BOT].map(String::from).to_vec();
                bots.extend(appservice.bots.hooks.bots(&room));
                for bot in bots {
                    if let Err(e) = appservice.matrix.leave(&room, &bot) {
                        eprintln!("{e}");
                    }
                }
            });
            continue;
        }
        if let Some((room, bot, usage)) = invitation(&event, &appservice.matrix) {
            let matrix = appservice.matrix.clone();
            let encrypted = encrypted(&event);
            // answered at once: the homeserver holds the next transactions until this one is acknowledged
            tokio::task::spawn_blocking(move || {
                let outcome = if encrypted {
                    matrix.leave(&room, bot)
                } else {
                    matrix
                        .join(&room, bot)
                        .and_then(|()| matrix.send(&room, bot, &json!({"msgtype": "m.notice", "body": usage})))
                };
                if let Err(e) = outcome {
                    eprintln!("{e}");
                }
            });
        }
    }
    (StatusCode::OK, Json(json!({})))
}

/// The homeserver sends the token in the header; the digests compared, so that the time taken says nothing about it.
fn from_homeserver(headers: &HeaderMap, hs_token: &str) -> bool {
    let bearer = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    digest(&SHA256, bearer.as_bytes()).as_ref() == digest(&SHA256, hs_token.as_bytes()).as_ref()
}

/// A message of a person to one of the bots.
pub struct Command {
    pub room: String,
    pub sender: String,
    /// The text, without the quote of the message it replies to.
    pub body: String,
    /// The event it replies to, if any.
    pub reply_to: Option<String>,
    /// Its own event, to redact it when it carried a secret.
    pub event_id: String,
}

/// A message of a person (not of a bot) starting with `prefix`, once the quote some apps put at the top of a reply
/// (`> <@sender> text` lines and a blank one) is left out.
fn command(event: &Value, matrix: &Matrix, prefix: &str) -> Option<Command> {
    if event["type"] != "m.room.message" {
        return None;
    }
    let body = event["content"]["body"].as_str()?;
    let body = body
        .lines()
        .skip_while(|line| line.starts_with("> "))
        .collect::<Vec<_>>()
        .join("\n");
    let body = body.trim();
    let sender = event["sender"].as_str()?;
    if !(body == prefix || body.starts_with(&format!("{prefix} "))) || matrix.is_bot(sender) {
        return None;
    }
    Some(Command {
        room: event["room_id"].as_str()?.to_string(),
        sender: sender.to_string(),
        body: body.to_string(),
        reply_to: event["content"]["m.relates_to"]["m.in_reply_to"]["event_id"]
            .as_str()
            .map(str::to_string),
        event_id: event["event_id"].as_str().unwrap_or_default().to_string(),
    })
}

/// An invitation of one of the bots: the room, the bot and what it says when it joins.
fn invitation(event: &Value, matrix: &Matrix) -> Option<(String, &'static str, &'static str)> {
    if event["type"] != "m.room.member" || event["content"]["membership"] != "invite" {
        return None;
    }
    let invited = event["state_key"].as_str()?;
    let (bot, usage) = [
        (github::BOT, github::USAGE),
        (clickup::BOT, clickup::USAGE),
        (hooks::BOT, hooks::USAGE),
    ]
    .into_iter()
    .find(|(bot, _)| matrix.user_id(bot) == invited)?;
    Some((event["room_id"].as_str()?.to_string(), bot, usage))
}

/// The bots stay out of encrypted rooms (the cryptography policy of the ISO documents): the homeserver sends the
/// state of the room along with the invitation.
fn encrypted(event: &Value) -> bool {
    event["unsigned"]["invite_room_state"]
        .as_array()
        .is_some_and(|state| state.iter().any(|e| e["type"] == "m.room.encryption"))
}

#[cfg(test)]
mod tests {
    use crate::matrix::Matrix;
    use serde_json::json;

    fn invite(state_key: &str, room_state: serde_json::Value) -> serde_json::Value {
        json!({
            "type": "m.room.member",
            "room_id": "!room:example.org",
            "state_key": state_key,
            "content": {"membership": "invite"},
            "unsigned": {"invite_room_state": room_state},
        })
    }

    #[test]
    fn a_bot_answers_invitations_of_itself_only_and_spots_encrypted_rooms() {
        let matrix = Matrix::new("http://homeserver".into(), "example.org".into(), "token".into());
        let plain = invite("@github:example.org", json!([{"type": "m.room.name"}]));
        let (room, bot, _) = super::invitation(&plain, &matrix).unwrap();
        assert_eq!((room.as_str(), bot), ("!room:example.org", "github"));
        assert!(!super::encrypted(&plain));
        assert!(super::invitation(&invite("@alice:example.org", json!([])), &matrix).is_none());
        let encrypted = invite("@clickup:example.org", json!([{"type": "m.room.encryption"}]));
        assert!(super::encrypted(&encrypted));
    }

    #[test]
    fn commands_are_messages_of_people_starting_with_the_prefix() {
        let matrix = Matrix::new("http://homeserver".into(), "example.org".into(), "token".into());
        let message = |sender: &str, body: &str| {
            json!({"type": "m.room.message", "room_id": "!room:example.org", "sender": sender,
                   "content": {"msgtype": "m.text", "body": body}})
        };
        let command = super::command(&message("@alice:example.org", " !github list "), &matrix, "!github").unwrap();
        assert_eq!(
            (command.room.as_str(), command.sender.as_str(), command.body.as_str()),
            ("!room:example.org", "@alice:example.org", "!github list")
        );
        let reply = json!({"type": "m.room.message", "room_id": "!room:example.org",
            "sender": "@alice:example.org",
            "content": {"msgtype": "m.text", "body": "> <@alice:example.org> sent an image.\n\n!github avatar",
                        "m.relates_to": {"m.in_reply_to": {"event_id": "$image"}}}});
        let command = super::command(&reply, &matrix, "!github").unwrap();
        assert_eq!(
            (command.body.as_str(), command.reply_to.as_deref()),
            ("!github avatar", Some("$image"))
        );
        assert!(super::command(&message("@alice:example.org", "!githubber"), &matrix, "!github").is_none());
        assert!(super::command(&message("@alice:example.org", "talk about !github"), &matrix, "!github").is_none());
        assert!(super::command(&message("@github:example.org", "!github list"), &matrix, "!github").is_none());
    }
}
