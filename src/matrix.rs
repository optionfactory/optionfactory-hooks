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
        self.post(&format!("rooms/{}/join", encoded(room)), bot)
            .map_err(|e| format!("join of {room} as {bot}: {e}"))
    }

    /// Also declines an invitation.
    pub fn leave(&self, room: &str, bot: &str) -> Result<(), String> {
        self.post(&format!("rooms/{}/leave", encoded(room)), bot)
            .map_err(|e| format!("leave of {room} as {bot}: {e}"))
    }

    /// Creates the user of the bot on the homeserver, if it is not there yet.
    pub fn register(&self, bot: &str) -> Result<(), String> {
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
        if status.is_success() || answer.contains("M_USER_IN_USE") {
            Ok(())
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
        self.call(ureq::put(self.url(path, bot)), body)
    }

    fn post(&self, path: &str, bot: &str) -> Result<(), String> {
        self.call(ureq::post(self.url(path, bot)), &json!({}))
    }

    fn call(&self, request: ureq::RequestBuilder<ureq::typestate::WithBody>, body: &Value) -> Result<(), String> {
        request
            .header("Authorization", &format!("Bearer {}", self.as_token))
            .header("Content-Type", "application/json")
            .send(body.to_string().as_bytes())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
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
    #[test]
    fn room_ids_and_user_ids_are_encoded_for_the_path() {
        assert_eq!(
            super::encoded("!45y-wnfMabg:optionfactory.net"),
            "%2145y-wnfMabg%3Aoptionfactory.net"
        );
        assert_eq!(
            super::encoded("@hooks-deploy:optionfactory.net"),
            "%40hooks-deploy%3Aoptionfactory.net"
        );
    }
}
