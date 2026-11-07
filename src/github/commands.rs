use super::{BOT, Github, NOISE, SECRET, SUBSCRIPTIONS, Subscription, USAGE};
use crate::matrix::moderator;
use ring::rand::SecureRandom;
use serde_json::json;

/// A message starting with `!github` in a room of the bot; blocking. The answer goes to the same room.
pub fn command(github: &Github, room: &str, sender: &str, body: &str) {
    let words: Vec<&str> = body.split_whitespace().collect();
    let answer = match words.get(1).copied() {
        Some("enable") => secret(github, room, sender, false),
        Some("rotate") => secret(github, room, sender, true),
        Some("subscribe") => subscribe(github, room, sender, &words[2..]),
        Some("unsubscribe") => unsubscribe(github, room, sender, &words[2..]),
        Some("list") => Ok(list(github, room)),
        _ => Ok(USAGE.to_string()),
    };
    let answer = answer.unwrap_or_else(|e| {
        eprintln!("github: {e}");
        "That did not work: the log of the service has the reason.".to_string()
    });
    if let Err(e) = github
        .matrix
        .send(room, BOT, &json!({"msgtype": "m.notice", "body": answer}))
    {
        eprintln!("github: {e}");
    }
}

/// A new secret for the webhook, sent only to an admin in a room with the bot alone: the answer carries it.
fn secret(github: &Github, room: &str, sender: &str, rotate: bool) -> Result<String, String> {
    if !github.admins.iter().any(|admin| admin == sender) {
        return Ok("Only the admins of the service can do that.".to_string());
    }
    if !github.matrix.alone_with(room, BOT, sender)? {
        return Ok(
            "I send the secret only in a room with you and me alone: create one without encryption and \
            invite me there."
                .to_string(),
        );
    }
    let enabled = github.key.read().unwrap_or_else(|e| e.into_inner()).is_some();
    match (enabled, rotate) {
        (true, false) => {
            return Ok(
                "The webhook is enabled already: `!github rotate` makes a new secret and refuses the old \
                one at once."
                    .to_string(),
            );
        }
        (false, true) => return Ok("The webhook is not enabled yet: `!github enable`.".to_string()),
        _ => {}
    }
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "no randomness".to_string())?;
    let secret = hex::encode(bytes);
    github
        .matrix
        .set_account_data(BOT, None, SECRET, &json!({"secret": secret}))?;
    *github.key.write().unwrap_or_else(|e| e.into_inner()) =
        Some(ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes()));
    let then = if rotate {
        " The old secret is refused from now on: paste this one into the webhook straight away, then redeliver from \
         Recent Deliveries what failed in between."
    } else {
        ""
    };
    Ok(format!(
        "On GitHub, organization Settings, Webhooks: payload URL {}/github, content type application/json, secret \
         {secret}, \"Send me everything\", active.{then}",
        github.public_url
    ))
}

fn subscribe(github: &Github, room: &str, sender: &str, args: &[&str]) -> Result<String, String> {
    if !moderator(&github.matrix.state(room, BOT)?, sender) {
        return Ok("Only the moderators of the room can change its subscriptions.".to_string());
    }
    let (target, events) = match args {
        [target] => (*target, vec![]),
        [target, events] => (*target, events.split(',').map(str::to_string).collect()),
        _ => return Ok(USAGE.to_string()),
    };
    if !valid_target(target) || !events.iter().all(|e| valid_event(e)) {
        return Ok(format!("Not a subscription: {}. {USAGE}", args.join(" ")));
    }
    let mut subscriptions = current(github, room);
    subscriptions.retain(|s| !s.target.eq_ignore_ascii_case(target));
    subscriptions.push(Subscription {
        target: target.to_string(),
        events: events.clone(),
    });
    save(github, room, subscriptions)?;
    let what = if events.is_empty() {
        format!("every event but {}", NOISE.join(", "))
    } else {
        events.join(", ")
    };
    let enabled = github.key.read().unwrap_or_else(|e| e.into_inner()).is_some();
    let pending = if enabled {
        ""
    } else {
        " The webhook of the organization is not enabled yet: an admin runs `!github enable`."
    };
    Ok(format!("This room gets {what} of {target}.{pending}"))
}

fn unsubscribe(github: &Github, room: &str, sender: &str, args: &[&str]) -> Result<String, String> {
    if !moderator(&github.matrix.state(room, BOT)?, sender) {
        return Ok("Only the moderators of the room can change its subscriptions.".to_string());
    }
    let [target] = args else {
        return Ok(USAGE.to_string());
    };
    let mut subscriptions = current(github, room);
    let before = subscriptions.len();
    subscriptions.retain(|s| !s.target.eq_ignore_ascii_case(target));
    if subscriptions.len() == before {
        return Ok(format!("This room is not subscribed to {target}."));
    }
    save(github, room, subscriptions)?;
    Ok(format!("This room no longer gets the events of {target}."))
}

fn list(github: &Github, room: &str) -> String {
    let subscriptions = current(github, room);
    if subscriptions.is_empty() {
        return "This room has no subscriptions.".to_string();
    }
    let lines: Vec<String> = subscriptions
        .iter()
        .map(|s| match s.events.as_slice() {
            [] => format!("{}: every event but the noise of continuous integration", s.target),
            events => format!("{}: {}", s.target, events.join(", ")),
        })
        .collect();
    format!("This room gets: {}.", lines.join("; "))
}

fn current(github: &Github, room: &str) -> Vec<Subscription> {
    let subscriptions = github.subscriptions.read().unwrap_or_else(|e| e.into_inner());
    subscriptions.get(room).cloned().unwrap_or_default()
}

/// Stored first, then taken into use: the lock is never held during a call to the homeserver.
fn save(github: &Github, room: &str, subscriptions: Vec<Subscription>) -> Result<(), String> {
    github
        .matrix
        .set_account_data(BOT, Some(room), SUBSCRIPTIONS, &json!({"subscriptions": subscriptions}))?;
    let mut all = github.subscriptions.write().unwrap_or_else(|e| e.into_inner());
    if subscriptions.is_empty() {
        all.remove(room);
    } else {
        all.insert(room.to_string(), subscriptions);
    }
    Ok(())
}

/// `owner` or `owner/repository`, with the characters GitHub allows in them.
fn valid_target(target: &str) -> bool {
    let parts: Vec<&str> = target.split('/').collect();
    parts.len() <= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
}

fn valid_event(event: &str) -> bool {
    !event.is_empty() && event.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

#[cfg(test)]
mod tests {

    #[test]
    fn targets_are_an_owner_or_a_repository() {
        assert!(super::valid_target("acme"));
        assert!(super::valid_target("acme/widgets"));
        assert!(!super::valid_target("acme/widgets/extra"));
        assert!(!super::valid_target("acme/"));
        assert!(!super::valid_target("<script>"));
        assert!(super::valid_event("pull_request"));
        assert!(!super::valid_event("Pull-Request"));
    }
}
