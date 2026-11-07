mod commands;
mod events;

use crate::Client;
use crate::matrix::Matrix;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub use commands::command;

pub const BOT: &str = "clickup";
pub const USAGE: &str = "I post the task changes of ClickUp. Room moderators: `!clickup subscribe <list or folder, by \
    id or name, or space id>`, `!clickup list`, `!clickup unsubscribe <the same>`. Admins, in a room with only me: \
    `!clickup connect <API token> [workspace id]`, `!clickup disconnect`; anywhere: `!clickup name <name>`, \
    `!clickup avatar` in reply to an image.";
/// Account data of the bot with the connection to the workspace.
const CONNECTION: &str = "net.optionfactory.hooks.clickup";
/// Account data of the bot in each room, with the subscriptions of the room.
const SUBSCRIPTIONS: &str = "net.optionfactory.hooks.clickup.subscriptions";
const API: &str = "https://api.clickup.com/api/v2";

pub struct Clickup {
    /// None until an admin connects a workspace.
    connection: RwLock<Option<Connection>>,
    /// Room ID to the lists, folders and spaces it follows, by id or name.
    subscriptions: RwLock<HashMap<String, Vec<String>>>,
    matrix: Arc<Matrix>,
    admins: Vec<String>,
    public_url: String,
}

/// The workspace and its webhook, with the personal API token of the admin who connected it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Connection {
    token: String,
    team: String,
    team_name: String,
    webhook: String,
    secret: String,
}

impl Clickup {
    /// Registers the bot and reads its connection and the subscriptions of its rooms; blocking.
    pub fn load(matrix: Arc<Matrix>, admins: Vec<String>, public_url: String) -> Result<Clickup, String> {
        matrix.register(BOT)?;
        let connection = matrix
            .account_data(BOT, None, CONNECTION)?
            .and_then(|data| serde_json::from_value::<Connection>(data).ok());
        let mut subscriptions = HashMap::new();
        for room in matrix.joined_rooms(BOT)? {
            let stored = matrix.account_data(BOT, Some(&room), SUBSCRIPTIONS)?;
            let targets: Vec<String> = stored
                .and_then(|data| serde_json::from_value(data["subscriptions"].clone()).ok())
                .unwrap_or_default();
            if !targets.is_empty() {
                subscriptions.insert(room, targets);
            }
        }
        Ok(Clickup {
            connection: RwLock::new(connection),
            subscriptions: RwLock::new(subscriptions),
            matrix,
            admins,
            public_url: public_url.trim_end_matches('/').to_string(),
        })
    }

    fn connection(&self) -> Option<Connection> {
        self.connection.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The webhook of the workspace, created by `!clickup connect`.
pub fn webhook(clickup: Arc<Clickup>) -> Router {
    Router::new().route("/clickup", post(receive)).with_state(clickup)
}

async fn receive(
    State(clickup): State<Arc<Clickup>>,
    Extension(Client(client)): Extension<Client>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(connection) = clickup.connection() else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    let signature = headers
        .get("X-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, connection.secret.as_bytes());
    if !signed(&key, &body, signature) {
        eprintln!("clickup: delivery from {client} with a wrong signature");
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    // the delivery does not say where the task is: the task itself, from the API, does
    tokio::task::spawn_blocking(move || {
        let event = payload["event"].as_str().unwrap_or_default();
        let Some(task_id) = payload["task_id"].as_str() else {
            return;
        };
        let task = match api_get(&connection.token, &format!("task/{task_id}")) {
            Ok(Some(task)) => task,
            Ok(None) => return,
            Err(e) => {
                eprintln!("clickup: task {task_id}: {e}");
                return;
            }
        };
        let rooms = rooms(&clickup.subscriptions.read().unwrap_or_else(|e| e.into_inner()), &task);
        if rooms.is_empty() {
            return;
        }
        let Some(message) = events::message(event, &payload, &task) else {
            return;
        };
        for room in rooms {
            if let Err(e) = clickup.matrix.send(&room, BOT, &message.content()) {
                eprintln!("clickup: {e}");
            }
        }
    });
    StatusCode::OK
}

/// The rooms following the list, the folder or the space of the task.
fn rooms(subscriptions: &HashMap<String, Vec<String>>, task: &Value) -> Vec<String> {
    let places = [
        task["list"]["id"].as_str(),
        task["list"]["name"].as_str(),
        task["folder"]["id"].as_str(),
        task["folder"]["name"].as_str(),
        task["space"]["id"].as_str(),
    ];
    let mut rooms: Vec<String> = subscriptions
        .iter()
        .filter(|(_, targets)| {
            targets
                .iter()
                .any(|target| places.iter().flatten().any(|place| place.eq_ignore_ascii_case(target)))
        })
        .map(|(room, _)| room.clone())
        .collect();
    rooms.sort();
    rooms
}

/// The hex HMAC of the body with the secret of the webhook, without prefix.
fn signed(key: &ring::hmac::Key, body: &[u8], signature: &str) -> bool {
    hex::decode(signature).is_ok_and(|tag| ring::hmac::verify(key, body, &tag).is_ok())
}

/// None when ClickUp answers 404.
fn api_get(token: &str, path: &str) -> Result<Option<Value>, String> {
    match ureq::get(format!("{API}/{path}")).header("Authorization", token).call() {
        Ok(mut response) => response
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())
            .and_then(|json| serde_json::from_str(&json).map_err(|e| e.to_string()))
            .map(Some),
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn api_post(token: &str, path: &str, body: &Value) -> Result<Value, String> {
    let mut response = ureq::post(format!("{API}/{path}"))
        .header("Authorization", token)
        .header("Content-Type", "application/json")
        .send(body.to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    let answer = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&answer).map_err(|e| e.to_string())
}

fn api_delete(token: &str, path: &str) -> Result<(), String> {
    match ureq::delete(format!("{API}/{path}"))
        .header("Authorization", token)
        .call()
    {
        Ok(_) | Err(ureq::Error::StatusCode(404)) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use ring::hmac::{HMAC_SHA256, Key, sign};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn accepts_the_signature_of_the_secret_and_nothing_else() {
        let body = br#"{"event":"taskCreated","task_id":"1vj37mc"}"#;
        let signature = hex::encode(sign(&Key::new(HMAC_SHA256, b"secret"), body));
        assert!(super::signed(&Key::new(HMAC_SHA256, b"secret"), body, &signature));
        assert!(!super::signed(&Key::new(HMAC_SHA256, b"another"), body, &signature));
        assert!(!super::signed(&Key::new(HMAC_SHA256, b"secret"), b"{}", &signature));
    }

    #[test]
    fn tasks_reach_the_rooms_of_their_list_folder_or_space() {
        let subscriptions = HashMap::from([
            ("!widgets".to_string(), vec!["Widgets Sprint".to_string()]),
            ("!gadgets".to_string(), vec!["90150001".to_string()]),
            ("!other".to_string(), vec!["Marketing".to_string()]),
        ]);
        let task = json!({"list": {"id": "901", "name": "widgets sprint"}, "folder": {"id": "77", "name": "Widgets"},
                          "space": {"id": "90150001"}});
        assert_eq!(super::rooms(&subscriptions, &task), ["!gadgets", "!widgets"]);
    }
}
