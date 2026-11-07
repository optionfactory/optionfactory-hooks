use crate::Client;
use crate::matrix::Matrix;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use std::sync::Arc;

pub const BOT: &str = "github";
/// Type of the account data of the bot holding the secret of the webhook.
pub const SETTINGS: &str = "net.optionfactory.hooks.github";
pub const USAGE: &str = "I post the events of GitHub. Room moderators: `!github subscribe <owner or owner/repository> \
    [event,...]`, `!github list`, `!github unsubscribe <owner or owner/repository>`.";

struct Github {
    /// None until an admin enables the integration.
    key: Option<ring::hmac::Key>,
    #[allow(dead_code)] // the events reach the subscribed rooms in the next step
    matrix: Arc<Matrix>,
}

/// The webhook of the organization, one secret for every repository, kept in the account data of the bot.
pub fn webhook(secret: Option<String>, matrix: Arc<Matrix>) -> Router {
    let key = secret.map(|secret| ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes()));
    Router::new()
        .route("/github", post(receive))
        .with_state(Arc::new(Github { key, matrix }))
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
    let Some(key) = &github.key else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    if !signed(key, &body, signature) {
        eprintln!("github: delivery from {client} with a wrong signature");
        return StatusCode::UNAUTHORIZED;
    }
    let event = headers
        .get("X-GitHub-Event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    eprintln!("github: {event}");
    StatusCode::ACCEPTED
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
    use ring::hmac::{HMAC_SHA256, Key};

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
}
