use crate::Client;
use crate::appservice::Command;
use crate::matrix::{Matrix, moderator};
use crate::profile::image_url;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::post;
use ring::digest::{SHA256, digest};
use ring::rand::SecureRandom;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Invited to a room, it creates there the bots of the scripts (`@hooks-<name>`) and hands out their tokens.
pub const BOT: &str = "hooks";
pub const USAGE: &str = "I give scripts a bot of their own in this room. Room moderators: `!hooks new <name> \
    [display name]` (the URL with the token comes to you in a direct room), `!hooks list`, `!hooks revoke <name>`. \
    Who created a hook, or an admin: `!hooks name <name> <display name>`, `!hooks avatar <name>` in reply to an image. \
    Sending: `curl -d 'text' <URL>`, or JSON with `text`, `html` and `level` (`error` notifies).";
/// Account data of `@hooks` in each room, with the hooks of the room.
const HOOKS: &str = "net.optionfactory.hooks.hooks";

pub struct Hooks {
    /// Room ID to its hooks, as in the account data of `@hooks` in that room.
    rooms: RwLock<HashMap<String, Vec<Hook>>>,
    matrix: Arc<Matrix>,
    admins: Vec<String>,
    public_url: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Hook {
    name: String,
    /// Hex SHA-256 of the token: the token itself is only with who uses it.
    token_sha256: String,
    creator: String,
}

impl Hooks {
    /// Registers `@hooks` and reads the hooks of its rooms; blocking.
    pub fn load(matrix: Arc<Matrix>, admins: Vec<String>, public_url: String) -> Result<Hooks, String> {
        matrix.register(BOT)?;
        let mut rooms = HashMap::new();
        for room in matrix.joined_rooms(BOT)? {
            let stored = matrix.account_data(BOT, Some(&room), HOOKS)?;
            let hooks: Vec<Hook> = stored
                .and_then(|data| serde_json::from_value(data["hooks"].clone()).ok())
                .unwrap_or_default();
            if !hooks.is_empty() {
                rooms.insert(room, hooks);
            }
        }
        Ok(Hooks {
            rooms: RwLock::new(rooms),
            matrix,
            admins,
            public_url: public_url.trim_end_matches('/').to_string(),
        })
    }

    /// The bots of the hooks of a room, to take them out of it.
    pub fn bots(&self, room: &str) -> Vec<String> {
        let rooms = self.rooms.read().unwrap_or_else(|e| e.into_inner());
        rooms
            .get(room)
            .map(|hooks| hooks.iter().map(|hook| bot(&hook.name)).collect())
            .unwrap_or_default()
    }

    fn of(&self, room: &str) -> Vec<Hook> {
        let rooms = self.rooms.read().unwrap_or_else(|e| e.into_inner());
        rooms.get(room).cloned().unwrap_or_default()
    }

    /// Stored first, then taken into use: the lock is never held during a call to the homeserver.
    fn save(&self, room: &str, hooks: Vec<Hook>) -> Result<(), String> {
        self.matrix
            .set_account_data(BOT, Some(room), HOOKS, &json!({"hooks": hooks}))?;
        let mut rooms = self.rooms.write().unwrap_or_else(|e| e.into_inner());
        if hooks.is_empty() {
            rooms.remove(room);
        } else {
            rooms.insert(room.to_string(), hooks);
        }
        Ok(())
    }
}

fn bot(name: &str) -> String {
    format!("hooks-{name}")
}

fn sha256(token: &str) -> String {
    hex::encode(digest(&SHA256, token.as_bytes()))
}

/// `POST /send/<token>`: text, or JSON with `text`, `html` and `level`.
pub fn sending(hooks: Arc<Hooks>) -> Router {
    Router::new().route("/send/{token}", post(send)).with_state(hooks)
}

async fn send(
    State(hooks): State<Arc<Hooks>>,
    Extension(Client(client)): Extension<Client>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let hashed = sha256(&token);
    let found = {
        let rooms = hooks.rooms.read().unwrap_or_else(|e| e.into_inner());
        rooms.iter().find_map(|(room, list)| {
            list.iter()
                .find(|hook| hook.token_sha256 == hashed)
                .map(|hook| (room.clone(), hook.name.clone()))
        })
    };
    let Some((room, name)) = found else {
        eprintln!("hooks: message from {client} with an unknown token");
        return StatusCode::UNAUTHORIZED;
    };
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    let Some(content) = content(json, &body) else {
        return StatusCode::BAD_REQUEST;
    };
    tokio::task::spawn_blocking(move || {
        if let Err(e) = hooks.matrix.send(&room, &bot(&name), &content) {
            eprintln!("hooks: {e}");
        }
    });
    StatusCode::ACCEPTED
}

/// The message of a script: a notice, or a normal message for `"level": "error"`.
fn content(json: bool, body: &[u8]) -> Option<Value> {
    let (text, html, level) = if json {
        let sent: Value = serde_json::from_slice(body).ok()?;
        (
            sent["text"].as_str()?.to_string(),
            sent["html"].as_str().map(str::to_string),
            sent["level"].as_str().unwrap_or_default().to_string(),
        )
    } else {
        (String::from_utf8(body.to_vec()).ok()?, None, String::new())
    };
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut content = json!({
        "msgtype": if level == "error" { "m.text" } else { "m.notice" },
        "body": text,
    });
    if let Some(html) = html {
        content["format"] = json!("org.matrix.custom.html");
        content["formatted_body"] = json!(html);
    }
    Some(content)
}

/// A message starting with `!hooks` in a room of `@hooks`; blocking. The answer goes to the same room.
pub fn command(hooks: &Hooks, command: &Command) {
    let words: Vec<&str> = command.body.split_whitespace().collect();
    let answer = match words.get(1).copied() {
        Some("new") => new(hooks, command, &words[2..]),
        Some("revoke") => revoke(hooks, command, &words[2..]),
        Some("list") => Ok(list(hooks, &command.room)),
        Some("name" | "avatar") => profile(hooks, command, &words[1..]),
        _ => Ok(USAGE.to_string()),
    };
    let answer = answer.unwrap_or_else(|e| {
        eprintln!("hooks: {e}");
        "That did not work: the log of the service has the reason.".to_string()
    });
    if let Err(e) = hooks
        .matrix
        .send(&command.room, BOT, &json!({"msgtype": "m.notice", "body": answer}))
    {
        eprintln!("hooks: {e}");
    }
}

fn new(hooks: &Hooks, command: &Command, args: &[&str]) -> Result<String, String> {
    if !moderator(&hooks.matrix.state(&command.room, BOT)?, &command.sender) {
        return Ok("Only the moderators of the room can create hooks.".to_string());
    }
    let Some((name, display)) = args.split_first() else {
        return Ok(USAGE.to_string());
    };
    if !valid_name(name) {
        return Ok("A hook name is 1 to 32 lowercase letters, digits and dashes.".to_string());
    }
    let mut list = hooks.of(&command.room);
    if list.iter().any(|hook| hook.name == *name) {
        return Ok(format!(
            "This room has a hook {name} already: `!hooks revoke {name}` first, for a new token."
        ));
    }
    let display = display.join(" ");
    let display = display.trim().trim_matches('"');
    let bot = bot(name);
    let created = hooks.matrix.register(&bot)?;
    let named = if created && !display.is_empty() {
        hooks.matrix.set_profile(&bot, "displayname", display)?;
        format!(" as {display}")
    } else if !created && !display.is_empty() {
        " (it existed already: it keeps its name, `!hooks name` changes it)".to_string()
    } else {
        String::new()
    };
    let user = hooks.matrix.user_id(&bot);
    hooks.matrix.invite(&command.room, BOT, &user)?;
    hooks.matrix.join(&command.room, &bot)?;
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "no randomness".to_string())?;
    let token = hex::encode(bytes);
    list.push(Hook {
        name: name.to_string(),
        token_sha256: sha256(&token),
        creator: command.sender.clone(),
    });
    hooks.save(&command.room, list)?;
    let direct = hooks.matrix.create_direct(BOT, &command.sender)?;
    let secret = format!(
        "The URL of the hook {name} (posting as {user} in {}): {}/send/{token}\nWhoever has it can post there: keep \
         it in Vaultwarden or in the vault of the project. `!hooks revoke {name}` in that room makes it useless.",
        command.room, hooks.public_url
    );
    hooks
        .matrix
        .send(&direct, BOT, &json!({"msgtype": "m.notice", "body": secret}))?;
    Ok(format!(
        "{user} is here{named}: the URL of the hook went to {} in a direct room.",
        command.sender
    ))
}

fn revoke(hooks: &Hooks, command: &Command, args: &[&str]) -> Result<String, String> {
    if !moderator(&hooks.matrix.state(&command.room, BOT)?, &command.sender) {
        return Ok("Only the moderators of the room can revoke hooks.".to_string());
    }
    let [name] = args else {
        return Ok(USAGE.to_string());
    };
    let mut list = hooks.of(&command.room);
    let before = list.len();
    list.retain(|hook| hook.name != *name);
    if list.len() == before {
        return Ok(format!("This room has no hook {name}."));
    }
    hooks.save(&command.room, list)?;
    hooks.matrix.leave(&command.room, &bot(name))?;
    Ok(format!("The hook {name} is revoked: its token is refused from now on."))
}

fn list(hooks: &Hooks, room: &str) -> String {
    let list = hooks.of(room);
    if list.is_empty() {
        return "This room has no hooks.".to_string();
    }
    let lines: Vec<String> = list
        .iter()
        .map(|hook| format!("{} (by {})", hook.name, hook.creator))
        .collect();
    format!("Hooks of this room: {}.", lines.join(", "))
}

/// `name <hook> <display name>` and `avatar <hook>`, in reply to an image: for who created the hook and the admins,
/// since the bot looks the same in every room.
fn profile(hooks: &Hooks, command: &Command, args: &[&str]) -> Result<String, String> {
    let (field, name, rest) = match args {
        [field, name, rest @ ..] => (*field, *name, rest.join(" ")),
        _ => return Ok(USAGE.to_string()),
    };
    let Some(hook) = hooks.of(&command.room).into_iter().find(|hook| hook.name == name) else {
        return Ok(format!("This room has no hook {name}."));
    };
    if hook.creator != command.sender && !hooks.admins.contains(&command.sender) {
        return Ok(format!(
            "Only {} or an admin can change the name and icon of {name}.",
            hook.creator
        ));
    }
    let bot = bot(name);
    match field {
        "name" => {
            let display = rest.trim().trim_matches('"');
            if display.is_empty() || display.chars().count() > 64 {
                return Ok("A name is 1 to 64 characters.".to_string());
            }
            hooks.matrix.set_profile(&bot, "displayname", display)?;
            Ok(format!("{name} is {display} now."))
        }
        _ => {
            let image = match &command.reply_to {
                Some(id) => hooks.matrix.event(&command.room, id, BOT)?,
                None => None,
            };
            match image.as_ref().and_then(|event| image_url(&event["content"])) {
                Some(url) => {
                    hooks.matrix.set_profile(&bot, "avatar_url", url)?;
                    Ok(format!("New icon for {name}."))
                }
                None => Ok(format!("Reply to an image with `!hooks avatar {name}`.")),
            }
        }
    }
}

fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn a_script_sends_text_or_json_and_an_error_notifies() {
        assert_eq!(
            super::content(false, b"deployed\n").unwrap(),
            json!({"msgtype": "m.notice", "body": "deployed"})
        );
        let sent = br#"{"text": "backup failed", "html": "backup <b>failed</b>", "level": "error"}"#;
        assert_eq!(
            super::content(true, sent).unwrap(),
            json!({"msgtype": "m.text", "body": "backup failed",
                   "format": "org.matrix.custom.html", "formatted_body": "backup <b>failed</b>"})
        );
        assert_eq!(super::content(false, b"  "), None);
        assert_eq!(super::content(true, br#"{"html": "no text"}"#), None);
    }

    #[test]
    fn hook_names_fit_a_user_id() {
        assert!(super::valid_name("deploy"));
        assert!(super::valid_name("shop-production-2"));
        assert!(!super::valid_name("Deploy"));
        assert!(!super::valid_name("de ploy"));
        assert!(!super::valid_name(""));
    }
}
