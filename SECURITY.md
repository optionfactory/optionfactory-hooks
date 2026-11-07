# Security

## Reporting a vulnerability

Report it privately through GitHub: **Security → Report a vulnerability** on this repository. Do not open a public
issue. Only the latest release is supported; fixes go into a new release.

## Threat model

Some of what follows (the commands, the subscriptions, the script hooks) is still being built: the README says
what is done.

**The service forwards notifications; it holds no data of its own.** What it posts comes from GitHub, ClickUp or a
script, and goes to the rooms that asked for it. The worst a forged delivery can do is a false message in a room;
nothing it receives runs, changes a room's settings or reaches another system.

### What it accepts

- **GitHub and ClickUp:** a delivery counts only with a valid signature of the body (HMAC-SHA256 with the secret of
  the webhook, compared in constant time). Without a secret, before an admin enables the integration, the endpoint
  answers 503.
- **Scripts:** a message counts only with the token of a hook. The room keeps the SHA-256 of the token, never the
  token: room state is visible to every member. A leaked token lets whoever has it post as that hook, in its room
  only, until `!hooks revoke`.
- **The homeserver:** the appservice paths (`/_matrix/app/`) count only with the `hs_token` of the registration. The
  reverse proxy does not forward them; the homeserver reaches the service directly.
- **Commands:** the integrations are enabled and disabled only by the users in `admins`, in a direct room with the
  bot, created without encryption since the bot refuses encrypted rooms: the replies carry the secrets, so never in
  a room shared with others. Subscriptions and hooks are created and removed only by the moderators of the room
  (the power level needed to change `m.room.power_levels`, or above).

### Secrets

| Secret | Where | Who can read it |
| --- | --- | --- |
| `as_token`, `hs_token` | the settings file (mode `0640`) | root and the user of the service |
| secret of the GitHub webhook | account data of `@github` on the homeserver | the bot; the admins of the homeserver (`!admin query account-data`) and root on its machine |
| ClickUp API token and webhook secret | account data of `@clickup` | as above |
| tokens of the hooks | nowhere: only their SHA-256, in room state | members of the room see the hash |

The account data of the bots is kept and backed up by the homeserver, like the messages: old backups keep a rotated
secret until they expire, so a leaked ClickUp token is revoked in ClickUp, not only replaced in the bot. The message
with the ClickUp token is redacted as soon as it is read; until the homeserver prunes it, it is readable like any
other message of that direct room.

### Rooms

- **No encrypted rooms:** a bot declines invitations to them. This follows the cryptography policy of optionfactory's
  ISO 27001 documents: bots never sit in end-to-end encrypted rooms, and notifications about a client go only to that
  client's room.
- **Notices:** messages go out as `m.notice`, except failures.

### Logs

The service logs, to the journal, the rejected deliveries with the address of the sender (from `X-Forwarded-For` only
when the connection comes from `trusted_proxy`, otherwise the address of the connection) and the failures to reach
the homeserver. Accepted deliveries are logged with their kind only; their content goes to the rooms, not to the logs.

## Build and supply chain

A single static binary (musl), released on GitHub with `SHA256SUMS`, and packaged in the signed APT repository
[linux-packages](https://github.com/optionfactory/linux-packages). The dependencies are few: `axum` and `tokio` for
the service, `ureq` with rustls for the calls to the homeserver and to ClickUp, `ring` for the signatures and the
hashes, `hex`, `serde`, `serde_json`, `toml` and `clap`.
