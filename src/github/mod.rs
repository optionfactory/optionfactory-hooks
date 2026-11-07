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
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};

pub use commands::command;

pub const BOT: &str = "github";
pub const USAGE: &str = "I post the events of GitHub. Room moderators: `!github subscribe <owner or owner/repository> \
    [event,...]`, `!github list`, `!github unsubscribe <owner or owner/repository>`. Admins, in a room with only me: \
    `!github enable`, `!github rotate`; anywhere: `!github name <name>`, `!github avatar` in reply to an image.";
/// Account data of the bot with the secret of the webhook.
const SECRET: &str = "net.optionfactory.hooks.github";
/// Account data of the bot in each room, with the subscriptions of the room.
const SUBSCRIPTIONS: &str = "net.optionfactory.hooks.github.subscriptions";
/// Taken only when a subscription names them: a push brings a dozen of them. A run is reported by `workflow_run`.
const NOISE: [&str; 7] = [
    "check_run",
    "check_suite",
    "workflow_job",
    "status",
    "deployment",
    "deployment_status",
    "page_build",
];

pub struct Github {
    /// None until an admin enables the webhook.
    key: RwLock<Option<ring::hmac::Key>>,
    /// Room ID to its subscriptions, as in the account data of the bot in that room.
    subscriptions: RwLock<HashMap<String, Vec<Subscription>>>,
    /// The push messages of the latest commits, for the reactions of their workflow runs.
    pushes: Mutex<Pushes>,
    matrix: Arc<Matrix>,
    admins: Vec<String>,
    public_url: String,
}

/// In memory only: after a restart a run whose push is not known posts its outcome as a message.
#[derive(Default)]
struct Pushes {
    /// (room, head commit) to the event of the push message.
    messages: HashMap<(String, String), String>,
    /// (room, head commit, workflow) to the reaction showing its state.
    reactions: HashMap<(String, String, String), String>,
    /// The oldest first, to forget beyond `REMEMBERED`.
    order: VecDeque<(String, String)>,
}

const REMEMBERED: usize = 500;

impl Pushes {
    fn remember(&mut self, room: &str, commit: &str, event: String) {
        let key = (room.to_string(), commit.to_string());
        self.messages.insert(key.clone(), event);
        self.order.push_back(key);
        while self.order.len() > REMEMBERED {
            if let Some((room, commit)) = self.order.pop_front() {
                self.messages.remove(&(room.clone(), commit.clone()));
                self.reactions.retain(|(r, c, _), _| *r != room || *c != commit);
            }
        }
    }
}

/// An organization (`acme`) or a repository (`acme/widgets`), with the events wanted; none means
/// all but the noise.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Subscription {
    target: String,
    #[serde(default)]
    events: Vec<String>,
}

impl Github {
    /// Registers the bot and reads its secret and the subscriptions of its rooms; blocking.
    pub fn load(matrix: Arc<Matrix>, admins: Vec<String>, public_url: String) -> Result<Github, String> {
        matrix.register(BOT)?;
        let secret = matrix.account_data(BOT, None, SECRET)?;
        let key = secret
            .and_then(|data| data["secret"].as_str().map(str::to_string))
            .map(|secret| ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes()));
        let mut subscriptions = HashMap::new();
        for room in matrix.joined_rooms(BOT)? {
            let stored = matrix.account_data(BOT, Some(&room), SUBSCRIPTIONS)?;
            let stored: Vec<Subscription> = stored
                .and_then(|data| serde_json::from_value(data["subscriptions"].clone()).ok())
                .unwrap_or_default();
            if !stored.is_empty() {
                subscriptions.insert(room, stored);
            }
        }
        Ok(Github {
            key: RwLock::new(key),
            subscriptions: RwLock::new(subscriptions),
            pushes: Mutex::new(Pushes::default()),
            matrix,
            admins,
            public_url: public_url.trim_end_matches('/').to_string(),
        })
    }
}

/// The webhook of the organization, one secret for every repository.
pub fn webhook(github: Arc<Github>) -> Router {
    Router::new().route("/github", post(receive)).with_state(github)
}

async fn receive(
    State(github): State<Arc<Github>>,
    Extension(Client(client)): Extension<Client>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let signature = headers
        .get("X-Hub-Signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let signed = match &*github.key.read().unwrap_or_else(|e| e.into_inner()) {
        Some(key) => signed(key, &body, signature),
        None => return StatusCode::SERVICE_UNAVAILABLE,
    };
    if !signed {
        eprintln!("github: delivery from {client} with a wrong signature");
        return StatusCode::UNAUTHORIZED;
    }
    let event = headers
        .get("X-GitHub-Event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let rooms = rooms(
        &github.subscriptions.read().unwrap_or_else(|e| e.into_inner()),
        &event,
        &payload,
    );
    if rooms.is_empty() {
        return StatusCode::ACCEPTED;
    }
    // answered at once: GitHub gives up on a delivery after ten seconds
    tokio::task::spawn_blocking(move || {
        let outcome = if event == "workflow_run" {
            workflow_run(&github, &rooms, &payload)
        } else {
            announce(&github, &rooms, &event, &payload)
        };
        if let Err(e) = outcome {
            eprintln!("github: {e}");
        }
    });
    StatusCode::ACCEPTED
}

/// The message of the event in each room; a push is remembered by its head commit, for its workflow runs.
fn announce(github: &Github, rooms: &[String], event: &str, payload: &Value) -> Result<(), String> {
    let Some(message) = events::message(event, payload) else {
        return Ok(());
    };
    for room in rooms {
        let sent = github.matrix.send(room, BOT, &message.content())?;
        if let (true, Some(commit)) = (event == "push", payload["after"].as_str()) {
            let mut pushes = github.pushes.lock().unwrap_or_else(|e| e.into_inner());
            pushes.remember(room, commit, sent);
        }
    }
    Ok(())
}

/// A reaction on the push of the commit (`🔵 build`, then `🟢 build`), replaced as the run goes on; where the push is
/// not known, the outcome as a message once the run is over.
fn workflow_run(github: &Github, rooms: &[String], payload: &Value) -> Result<(), String> {
    let Some((key, done)) = events::ci(payload) else {
        return Ok(());
    };
    let run = &payload["workflow_run"];
    let commit = run["head_sha"].as_str().unwrap_or_default().to_string();
    let name = run["name"].as_str().unwrap_or_default().to_string();
    for room in rooms {
        let (push, previous) = {
            let pushes = github.pushes.lock().unwrap_or_else(|e| e.into_inner());
            let place = (room.clone(), commit.clone());
            let push = pushes.messages.get(&place).cloned();
            let previous = pushes
                .reactions
                .get(&(room.clone(), commit.clone(), name.clone()))
                .cloned();
            (push, previous)
        };
        match push {
            Some(push) => {
                if let Some(previous) = previous {
                    github.matrix.redact(room, &previous, BOT)?;
                }
                let reaction = github.matrix.react(room, BOT, &push, &key)?;
                let mut pushes = github.pushes.lock().unwrap_or_else(|e| e.into_inner());
                pushes
                    .reactions
                    .insert((room.clone(), commit.clone(), name.clone()), reaction);
            }
            None if done => {
                if let Some(message) = events::message("workflow_run", payload) {
                    github.matrix.send(room, BOT, &message.content())?;
                }
            }
            None => {}
        }
    }
    Ok(())
}

/// The rooms subscribed to the organization or the repository of the event, and to its kind.
fn rooms(subscriptions: &HashMap<String, Vec<Subscription>>, event: &str, payload: &Value) -> Vec<String> {
    let repository = payload["repository"]["full_name"].as_str().map(str::to_lowercase);
    let owner = payload["organization"]["login"]
        .as_str()
        .or(payload["repository"]["owner"]["login"].as_str())
        .map(str::to_lowercase);
    let mut rooms: Vec<String> = subscriptions
        .iter()
        .filter(|(_, subscriptions)| {
            subscriptions.iter().any(|subscription| {
                let target = Some(subscription.target.to_lowercase());
                let wanted = if subscription.events.is_empty() {
                    !NOISE.contains(&event)
                } else {
                    subscription.events.iter().any(|e| e == event)
                };
                wanted && (target == repository || target == owner)
            })
        })
        .map(|(room, _)| room.clone())
        .collect();
    rooms.sort();
    rooms
}

/// `sha256=<hex>` of the HMAC of the body with the secret of the webhook.
fn signed(key: &ring::hmac::Key, body: &[u8], signature: &str) -> bool {
    signature
        .strip_prefix("sha256=")
        .and_then(|tag| hex::decode(tag).ok())
        .is_some_and(|tag| ring::hmac::verify(key, body, &tag).is_ok())
}

#[cfg(test)]
mod tests {
    use super::Subscription;
    use ring::hmac::{HMAC_SHA256, Key};
    use serde_json::json;
    use std::collections::HashMap;

    /// The example of the GitHub documentation, "Validating webhook deliveries".
    #[test]
    fn accepts_the_signature_of_the_secret_and_nothing_else() {
        let key = Key::new(HMAC_SHA256, b"It's a Secret to Everybody");
        let signature = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert!(super::signed(&key, b"Hello, World!", signature));
        assert!(!super::signed(&key, b"Hello, World?", signature));
        assert!(!super::signed(
            &key,
            b"Hello, World!",
            signature.trim_start_matches("sha256=")
        ));
        assert!(!super::signed(&key, b"Hello, World!", ""));
    }

    #[test]
    fn events_reach_the_rooms_of_their_organization_or_repository() {
        let subscribe = |target: &str, events: &[&str]| Subscription {
            target: target.to_string(),
            events: events.iter().map(|e| e.to_string()).collect(),
        };
        let subscriptions = HashMap::from([
            ("!acme".to_string(), vec![subscribe("acme", &[])]),
            ("!widgets".to_string(), vec![subscribe("acme/widgets", &["release"])]),
            ("!other".to_string(), vec![subscribe("someone-else", &[])]),
        ]);
        let widgets = json!({
            "repository": {"full_name": "acme/widgets", "owner": {"login": "acme"}},
            "organization": {"login": "acme"},
        });
        assert_eq!(super::rooms(&subscriptions, "push", &widgets), ["!acme"]);
        assert_eq!(super::rooms(&subscriptions, "release", &widgets), ["!acme", "!widgets"]);
        assert!(super::rooms(&subscriptions, "check_run", &widgets).is_empty());
        let member = json!({"organization": {"login": "Acme"}});
        assert_eq!(super::rooms(&subscriptions, "organization", &member), ["!acme"]);
    }
}
