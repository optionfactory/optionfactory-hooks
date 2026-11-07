use crate::matrix::Matrix;
use crate::{clickup, github};
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
}

#[derive(Deserialize)]
struct Transaction {
    #[serde(default)]
    events: Vec<Value>,
}

/// The side the homeserver calls: the room events that concern the bots.
pub fn transactions(hs_token: String, matrix: Arc<Matrix>) -> Router {
    Router::new()
        .route("/_matrix/app/v1/transactions/{txn}", put(receive))
        .route("/_matrix/app/v1/ping", post(ping))
        .with_state(Arc::new(Appservice { hs_token, matrix }))
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

/// An invitation of one of the bots: the room, the bot and what it says when it joins.
fn invitation(event: &Value, matrix: &Matrix) -> Option<(String, &'static str, &'static str)> {
    if event["type"] != "m.room.member" || event["content"]["membership"] != "invite" {
        return None;
    }
    let invited = event["state_key"].as_str()?;
    let (bot, usage) = [(github::BOT, github::USAGE), (clickup::BOT, clickup::USAGE)]
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
            "room_id": "!room:optionfactory.net",
            "state_key": state_key,
            "content": {"membership": "invite"},
            "unsigned": {"invite_room_state": room_state},
        })
    }

    #[test]
    fn a_bot_answers_invitations_of_itself_only_and_spots_encrypted_rooms() {
        let matrix = Matrix::new("http://homeserver".into(), "optionfactory.net".into(), "token".into());
        let plain = invite("@github:optionfactory.net", json!([{"type": "m.room.name"}]));
        let (room, bot, _) = super::invitation(&plain, &matrix).unwrap();
        assert_eq!((room.as_str(), bot), ("!room:optionfactory.net", "github"));
        assert!(!super::encrypted(&plain));
        assert!(super::invitation(&invite("@roberto:optionfactory.net", json!([])), &matrix).is_none());
        let encrypted = invite("@clickup:optionfactory.net", json!([{"type": "m.room.encryption"}]));
        assert!(super::encrypted(&encrypted));
    }
}
