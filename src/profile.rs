use crate::appservice::Command;
use crate::matrix::Matrix;
use serde_json::json;

/// `!<bot> avatar`, in reply to an image, and `!<bot> name <name>`: the icon and the name of a bot, the same in every
/// room, so for the admins only. Blocking.
pub fn command(matrix: &Matrix, admins: &[String], bot: &str, command: &Command) {
    let answer = answer(matrix, admins, bot, command).unwrap_or_else(|e| {
        eprintln!("{bot}: {e}");
        "That did not work: the log of the service has the reason.".to_string()
    });
    if let Err(e) = matrix.send(&command.room, bot, &json!({"msgtype": "m.notice", "body": answer})) {
        eprintln!("{bot}: {e}");
    }
}

fn answer(matrix: &Matrix, admins: &[String], bot: &str, command: &Command) -> Result<String, String> {
    if !admins.contains(&command.sender) {
        return Ok("Only the admins of the service can change my name and icon.".to_string());
    }
    let mut words = command.body.splitn(3, char::is_whitespace).skip(1);
    match (words.next(), words.next().map(str::trim)) {
        (Some("name"), Some(name)) if !name.is_empty() && name.chars().count() <= 64 => {
            let name = name.trim_matches('"');
            matrix.set_profile(bot, "displayname", name)?;
            Ok(format!("I am {name} now."))
        }
        (Some("avatar"), None) => {
            let image = match &command.reply_to {
                Some(id) => matrix.event(&command.room, id, bot)?,
                None => None,
            };
            match image.as_ref().and_then(|event| image_url(&event["content"])) {
                Some(url) => {
                    matrix.set_profile(bot, "avatar_url", url)?;
                    Ok("New icon set.".to_string())
                }
                None => Ok(format!("Reply to an image with `!{bot} avatar`.")),
            }
        }
        _ => Ok(format!(
            "`!{bot} avatar` in reply to an image, or `!{bot} name <name>` (at most 64 characters)."
        )),
    }
}

/// The `mxc://` URL of an image message; an encrypted one has none, and the bots are not in encrypted rooms anyway.
pub fn image_url(content: &serde_json::Value) -> Option<&str> {
    (content["msgtype"] == "m.image")
        .then(|| content["url"].as_str())
        .flatten()
        .filter(|url| url.starts_with("mxc://"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn only_an_image_message_gives_an_icon() {
        let image = json!({"msgtype": "m.image", "body": "github.png", "url": "mxc://example.org/abc"});
        assert_eq!(super::image_url(&image), Some("mxc://example.org/abc"));
        assert_eq!(
            super::image_url(&json!({"msgtype": "m.text", "body": "mxc://x/y"})),
            None
        );
        assert_eq!(
            super::image_url(&json!({"msgtype": "m.image", "url": "https://elsewhere/abc"})),
            None
        );
    }
}
