# optionfactory-hooks

Events of GitHub, ClickUp and our own scripts posted to Matrix rooms, each source as its own bot with its own name and
icon. One static binary, an appservice of the homeserver.

This README describes the tool. How optionfactory runs it (the instance, nginx, the registration, the settings) is in
the `internal-infra` repository.

Threat model, secrets and how to report a vulnerability: [SECURITY.md](SECURITY.md).

## How it works

- **Bots.** The appservice owns `@github`, `@clickup` and `@hooks-*`: it acts as any of them with its token, without
  passwords or logins, and registers `@github` and `@clickup` on the homeserver at start. Invite a bot to a room and
  it joins and says how to use it. It refuses encrypted rooms.
- **Enabling an integration** is up to the admins of the settings, in a direct room with the bot. Element and Cinny
  encrypt direct messages by default and the bot refuses encrypted rooms: create the room with encryption off. The
  bot keeps the secrets in its own account data on the homeserver: nothing to configure or deploy, backed up with the
  homeserver.

  ```
  !github enable                  a new secret for the webhook of the organization, with the URL to give GitHub
  !github rotate                  a new secret, the old one refused at once
  !clickup connect <API token>    creates the webhook of the workspace; the message with the token is redacted
  !clickup disconnect             deletes it
  ```
- **Routing lives in the rooms.** Room moderators subscribe a room with a command, and the subscription is stored as
  state of the room: no database, no configuration to change.

  ```
  !github subscribe optionfactory                       every repository of the organization
  !github subscribe optionfactory/ior-portal pull_request,workflow_run
  !clickup subscribe <space or list>
  !github list / unsubscribe ...
  ```

  A subscription without events takes them all except the noise of continuous integration (`check_run`,
  `check_suite`, `workflow_job`, `status`); a finished workflow run is reported once. Events without their own format
  get a line with who, what and where.
- **GitHub:** one webhook for the organization, its signature (`X-Hub-Signature-256`) checked on every delivery.
- **ClickUp:** one webhook for the workspace, its signature (`X-Signature`) checked on every delivery.
- **Scripts:** `curl -d 'deployed' https://hooks.opfa.net/send/<token>`, one token per hook, kept in the vault of who
  uses it. The room stores only the hash of the token.

  ```
  !hooks new deploy "Deploy"        creates @hooks-deploy; the URL with the token, sent privately to who asked
  !hooks avatar deploy              in reply to an image: the icon of @hooks-deploy
  !hooks list / revoke deploy
  ```

  A hook bot is one identity for the whole server: a second room gets the same bot with its own token.
- **Notices.** Messages go out as `m.notice`, which most clients do not push; failures as normal messages.

## Running

```
optionfactory-hooks serve [--config /etc/optionfactory-hooks/hooks.conf]
```

The settings are in a TOML file (mode `0640`, readable by the user of the service: it holds the tokens of the
appservice); an unknown key stops the service at start:

```toml
listen = "127.0.0.1:8009"                # for the reverse proxy and the homeserver
trusted_proxy = "172.18.0.10"            # the proxy: only from it is X-Forwarded-For believed
homeserver = "http://127.0.0.1:8008"     # client-server API
server_name = "optionfactory.net"
admins = ["@roberto:optionfactory.net"]  # who may enable and disable the integrations

[appservice]
as_token = "<as_token of the registration>"
hs_token = "<hs_token of the registration>"
```

The reverse proxy forwards only `/github`, `/clickup` and `/send/`, including its `forwarded.conf`: the client
address in `X-Forwarded-For` names the sender of rejected deliveries in the logs. The paths under `/_matrix/app/` are
for the homeserver, which reaches the same address. Until an admin enables it, `/github` and `/clickup` answer 503.

The registration of the appservice in the homeserver:

```yaml
id: optionfactory-hooks
url: http://<listen, as the homeserver reaches it>
as_token: <as_token>
hs_token: <hs_token>
sender_localpart: hooks
rate_limited: false
namespaces:
  users:
    - {regex: "@github:optionfactory\\.net", exclusive: true}
    - {regex: "@clickup:optionfactory\\.net", exclusive: true}
    - {regex: "@hooks-.*:optionfactory\\.net", exclusive: true}
```

## Status

Done: the settings, the listener, the bots registered at start with the secrets of their webhooks read from their
account data, the signatures of GitHub and ClickUp, the appservice side (token of the homeserver, invitations: join
and usage, encrypted rooms refused), the Matrix calls as a bot.

Next:

1. Commands: the admin ones (`enable`, `rotate`, `connect`, `disconnect`) and the room ones, subscriptions as room
   state, moderators only.
2. GitHub events formatted and routed, the organization or one repository.
3. Script hooks: `!hooks new/list/revoke/avatar`, `/send/<token>` (text or JSON with `text`, `html`, `level`).
4. ClickUp: task events formatted and routed.
5. Bot names and avatars set at start, icons built into the binary.

## Building

`make build-release`: `target/x86_64-unknown-linux-musl/release/optionfactory-hooks`, statically linked (musl,
rustls with built-in root certificates). `make publish-github` releases it on GitHub.
