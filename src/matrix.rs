use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The homeserver, seen from the appservice: every call acts as one of the bots, with the appservice token. All the
/// calls block, for `spawn_blocking`.
pub struct Matrix {
    homeserver: String,
    server_name: String,
    as_token: String,
    /// Transaction IDs unique across restarts: the homeserver drops a repeated one as a retry.
    started: u64,
    sent: AtomicU64,
}

impl Matrix {
    pub fn new(homeserver: String, server_name: String, as_token: String) -> Self {
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        Self {
            homeserver: homeserver.trim_end_matches('/').to_string(),
            server_name,
            as_token,
            started,
            sent: AtomicU64::new(0),
        }
    }

    pub fn user_id(&self, bot: &str) -> String {
        format!("@{bot}:{}", self.server_name)
    }

    /// One of the users of the appservice: `@github`, `@clickup`, `@hooks` and `@hooks-*`.
    pub fn is_bot(&self, user: &str) -> bool {
        user.strip_prefix('@')
            .and_then(|user| user.strip_suffix(&format!(":{}", self.server_name)))
            .is_some_and(|bot| matches!(bot, "github" | "clickup" | "hooks") || bot.starts_with("hooks-"))
    }

    /// Posts an `m.room.message` as `bot`.
    pub fn send(&self, room: &str, bot: &str, content: &Value) -> Result<(), String> {
        let txn = format!("{}-{}", self.started, self.sent.fetch_add(1, Ordering::Relaxed));
        self.put(
            &format!("rooms/{}/send/m.room.message/{txn}", encoded(room)),
            bot,
            content,
        )
        .map_err(|e| format!("message to {room} as {bot}: {e}"))
    }

    pub fn join(&self, room: &str, bot: &str) -> Result<(), String> {
        self.post(&format!("rooms/{}/join", encoded(room)), bot, &json!({}))
            .map(|_| ())
            .map_err(|e| format!("join of {room} as {bot}: {e}"))
    }

    /// Also declines an invitation.
    pub fn leave(&self, room: &str, bot: &str) -> Result<(), String> {
        self.post(&format!("rooms/{}/leave", encoded(room)), bot, &json!({}))
            .map(|_| ())
            .map_err(|e| format!("leave of {room} as {bot}: {e}"))
    }

    pub fn invite(&self, room: &str, bot: &str, user: &str) -> Result<(), String> {
        self.post(
            &format!("rooms/{}/invite", encoded(room)),
            bot,
            &json!({"user_id": user}),
        )
        .map(|_| ())
        .map_err(|e| format!("invitation of {user} to {room}: {e}"))
    }

    /// A new direct room of the bot with `user`, without encryption: the bots stay out of encrypted rooms.
    pub fn create_direct(&self, bot: &str, user: &str) -> Result<String, String> {
        let body = json!({"preset": "trusted_private_chat", "is_direct": true, "invite": [user]});
        let created = self
            .post("createRoom", bot, &body)
            .map_err(|e| format!("direct room of {bot} with {user}: {e}"))?;
        created["room_id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("direct room of {bot} with {user}: no room_id"))
    }

    /// Needs the power to redact the events of others, unless the event is of the bot.
    pub fn redact(&self, room: &str, event: &str, bot: &str) -> Result<(), String> {
        let txn = format!("{}-{}", self.started, self.sent.fetch_add(1, Ordering::Relaxed));
        let path = format!("rooms/{}/redact/{}/{txn}", encoded(room), encoded(event));
        self.put(&path, bot, &json!({"reason": "it carried a secret"}))
            .map_err(|e| format!("redaction of {event} in {room}: {e}"))
    }

    /// Whether the room has only `user` and the bot in it: answers that carry a secret go only there.
    pub fn alone_with(&self, room: &str, bot: &str, user: &str) -> Result<bool, String> {
        let members = self.joined_members(room, bot)?;
        let me = self.user_id(bot);
        Ok(members.len() == 2 && members.iter().any(|m| m == user) && members.contains(&me))
    }

    /// Creates the user of the bot on the homeserver, if it is not there yet; true when it was not.
    pub fn register(&self, bot: &str) -> Result<bool, String> {
        let url = format!("{}/_matrix/client/v3/register", self.homeserver);
        let body = json!({"type": "m.login.application_service", "username": bot});
        let response = ureq::post(url)
            .config()
            .http_status_as_error(false)
            .build()
            .header("Authorization", &format!("Bearer {}", self.as_token))
            .header("Content-Type", "application/json")
            .send(body.to_string().as_bytes());
        let mut response = response.map_err(|e| format!("registration of {bot}: {e}"))?;
        let status = response.status();
        let answer = response.body_mut().read_to_string().unwrap_or_default();
        if status.is_success() {
            Ok(true)
        } else if answer.contains("M_USER_IN_USE") {
            Ok(false)
        } else {
            Err(format!("registration of {bot}: {status} {answer}"))
        }
    }

    /// The account data of `kind` of the bot, private to it, kept and backed up by the homeserver; of one room when
    /// `room` is given.
    pub fn account_data(&self, bot: &str, room: Option<&str>, kind: &str) -> Result<Option<Value>, String> {
        self.get(&self.account_data_path(bot, room, kind), bot)
            .map_err(|e| format!("{kind} of {bot}: {e}"))
    }

    pub fn set_account_data(&self, bot: &str, room: Option<&str>, kind: &str, data: &Value) -> Result<(), String> {
        self.put(&self.account_data_path(bot, room, kind), bot, data)
            .map_err(|e| format!("{kind} of {bot}: {e}"))
    }

    pub fn joined_rooms(&self, bot: &str) -> Result<Vec<String>, String> {
        let rooms = self
            .get("joined_rooms", bot)
            .map_err(|e| format!("rooms of {bot}: {e}"))?;
        Ok(rooms
            .and_then(|rooms| serde_json::from_value(rooms["joined_rooms"].clone()).ok())
            .unwrap_or_default())
    }

    pub fn joined_members(&self, room: &str, bot: &str) -> Result<Vec<String>, String> {
        let members = self
            .get(&format!("rooms/{}/joined_members", encoded(room)), bot)
            .map_err(|e| format!("members of {room}: {e}"))?;
        Ok(members
            .and_then(|members| {
                members["joined"]
                    .as_object()
                    .map(|joined| joined.keys().cloned().collect())
            })
            .unwrap_or_default())
    }

    /// An event of the room, whole.
    pub fn event(&self, room: &str, id: &str, bot: &str) -> Result<Option<Value>, String> {
        self.get(&format!("rooms/{}/event/{}", encoded(room), encoded(id)), bot)
            .map_err(|e| format!("event {id} of {room}: {e}"))
    }

    /// `displayname` or `avatar_url` of the bot, the same in every room.
    pub fn set_profile(&self, bot: &str, field: &str, value: &str) -> Result<(), String> {
        let path = format!("profile/{}/{field}", encoded(&self.user_id(bot)));
        self.put(&path, bot, &json!({ field: value }))
            .map_err(|e| format!("{field} of {bot}: {e}"))
    }

    /// The state events of the room, whole: who sent `m.room.create` is not in its content.
    pub fn state(&self, room: &str, bot: &str) -> Result<Vec<Value>, String> {
        let state = self
            .get(&format!("rooms/{}/state", encoded(room)), bot)
            .map_err(|e| format!("state of {room}: {e}"))?;
        Ok(state.and_then(|state| state.as_array().cloned()).unwrap_or_default())
    }

    fn account_data_path(&self, bot: &str, room: Option<&str>, kind: &str) -> String {
        let user = encoded(&self.user_id(bot));
        match room {
            Some(room) => format!("user/{user}/rooms/{}/account_data/{kind}", encoded(room)),
            None => format!("user/{user}/account_data/{kind}"),
        }
    }

    fn url(&self, path: &str, bot: &str) -> String {
        let user_id = encoded(&self.user_id(bot));
        format!("{}/_matrix/client/v3/{path}?user_id={user_id}", self.homeserver)
    }

    /// None when the homeserver answers 404: nothing there yet.
    fn get(&self, path: &str, bot: &str) -> Result<Option<Value>, String> {
        let response = ureq::get(self.url(path, bot))
            .header("Authorization", &format!("Bearer {}", self.as_token))
            .call();
        match response {
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

    fn put(&self, path: &str, bot: &str, body: &Value) -> Result<(), String> {
        self.call(ureq::put(self.url(path, bot)), body).map(|_| ())
    }

    fn post(&self, path: &str, bot: &str, body: &Value) -> Result<Value, String> {
        self.call(ureq::post(self.url(path, bot)), body)
    }

    /// The answer of the homeserver, `{}` when it has none.
    fn call(&self, request: ureq::RequestBuilder<ureq::typestate::WithBody>, body: &Value) -> Result<Value, String> {
        let mut response = request
            .header("Authorization", &format!("Bearer {}", self.as_token))
            .header("Content-Type", "application/json")
            .send(body.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        let answer = response.body_mut().read_to_string().unwrap_or_default();
        Ok(serde_json::from_str(&answer).unwrap_or_else(|_| json!({})))
    }
}

/// A moderator can change the power levels of the room: the level the room asks for that, 50 if it does not say. From
/// room version 12 the creators (who sent `m.room.create`, and its `additional_creators`) are above every level and
/// are not among the users of the power levels.
pub fn moderator(state: &[Value], sender: &str) -> bool {
    let event = |kind: &str| state.iter().find(|e| e["type"] == kind && e["state_key"] == "");
    if let Some(create) = event("m.room.create") {
        let version = create["content"]["room_version"].as_str().unwrap_or("1");
        let creators_above = version.parse::<u32>().map_or(version.contains("hydra"), |v| v >= 12);
        let creator = create["sender"] == sender
            || create["content"]["additional_creators"]
                .as_array()
                .is_some_and(|more| more.iter().any(|c| c == sender));
        if creators_above && creator {
            return true;
        }
    }
    let levels = event("m.room.power_levels")
        .map(|e| &e["content"])
        .cloned()
        .unwrap_or_default();
    let needed = levels["events"]["m.room.power_levels"]
        .as_i64()
        .or(levels["state_default"].as_i64())
        .unwrap_or(50);
    let level = levels["users"][sender]
        .as_i64()
        .or(levels["users_default"].as_i64())
        .unwrap_or(0);
    level >= needed
}

/// Room and user IDs carry `!`, `@` and `:`, which a path segment or a query value must not.
fn encoded(part: &str) -> String {
    part.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn moderators_are_who_can_change_the_power_levels_and_the_creators_of_newer_rooms() {
        let state = |version: &str, levels: serde_json::Value| {
            vec![
                json!({"type": "m.room.create", "state_key": "", "sender": "@alice:example.org",
                       "content": {"room_version": version, "additional_creators": ["@bob:example.org"]}}),
                json!({"type": "m.room.power_levels", "state_key": "", "content": levels}),
            ]
        };
        let levels = json!({
            "users": {"@carol:example.org": 50, "@dave:example.org": 100},
            "events": {"m.room.power_levels": 100},
        });
        let v12 = state("12", levels.clone());
        assert!(super::moderator(&v12, "@alice:example.org"));
        assert!(super::moderator(&v12, "@bob:example.org"));
        assert!(super::moderator(&v12, "@dave:example.org"));
        assert!(!super::moderator(&v12, "@carol:example.org"));
        assert!(!super::moderator(&v12, "@eve:example.org"));
        let v11 = state("11", levels);
        assert!(!super::moderator(&v11, "@alice:example.org"));
        let default = state("10", json!({"users": {"@carol:example.org": 50}}));
        assert!(super::moderator(&default, "@carol:example.org"));
    }

    #[test]
    fn room_ids_and_user_ids_are_encoded_for_the_path() {
        assert_eq!(
            super::encoded("!45y-wnfMabg:example.org"),
            "%2145y-wnfMabg%3Aexample.org"
        );
        assert_eq!(
            super::encoded("@hooks-deploy:example.org"),
            "%40hooks-deploy%3Aexample.org"
        );
    }
}
