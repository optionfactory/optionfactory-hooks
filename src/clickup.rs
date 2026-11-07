use crate::Client;
use crate::matrix::Matrix;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use std::sync::Arc;

pub const BOT: &str = "clickup";
/// Type of the account data of the bot holding the secret of the webhook.
pub const SETTINGS: &str = "net.optionfactory.hooks.clickup";
pub const USAGE: &str = "I post the task changes of ClickUp. Room moderators: `!clickup subscribe <space or list>`, \
    `!clickup list`, `!clickup unsubscribe <space or list>`.";

struct Clickup {
    /// None until an admin enables the integration.
    key: Option<ring::hmac::Key>,
    #[allow(dead_code)] // the events reach the subscribed rooms in the next step
    matrix: Arc<Matrix>,
}

/// The webhook of the workspace, with the secret ClickUp returned when the webhook was created, kept in the account data of the bot.
pub fn webhook(secret: Option<String>, matrix: Arc<Matrix>) -> Router {
    let key = secret.map(|secret| ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes()));
    Router::new()
        .route("/clickup", post(receive))
        .with_state(Arc::new(Clickup { key, matrix }))
}

async fn receive(
    State(clickup): State<Arc<Clickup>>,
    Extension(Client(client)): Extension<Client>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let signature = headers
        .get("X-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let Some(key) = &clickup.key else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    if !signed(key, &body, signature) {
        eprintln!("clickup: delivery from {client} with a wrong signature");
        return StatusCode::UNAUTHORIZED;
    }
    let event = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|payload| payload["event"].as_str().map(str::to_string))
        .unwrap_or_default();
    eprintln!("clickup: {event}");
    StatusCode::ACCEPTED
}

/// The hex HMAC of the body with the secret of the webhook, without prefix.
fn signed(key: &ring::hmac::Key, body: &[u8], signature: &str) -> bool {
    hex::decode(signature).is_ok_and(|tag| ring::hmac::verify(key, body, &tag).is_ok())
}

#[cfg(test)]
mod tests {
    use ring::hmac::{HMAC_SHA256, Key, sign};

    #[test]
    fn accepts_the_signature_of_the_secret_and_nothing_else() {
        let body = br#"{"event":"taskCreated","task_id":"1vj37mc"}"#;
        let signature = hex::encode(sign(&Key::new(HMAC_SHA256, b"secret"), body));
        assert!(super::signed(&Key::new(HMAC_SHA256, b"secret"), body, &signature));
        assert!(!super::signed(&Key::new(HMAC_SHA256, b"another"), body, &signature));
        assert!(!super::signed(&Key::new(HMAC_SHA256, b"secret"), b"{}", &signature));
    }
}
