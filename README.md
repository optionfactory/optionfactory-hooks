# optionfactory-hooks

Events of GitHub, ClickUp and our own scripts posted to Matrix rooms, each source as its own bot with its own name and
icon. One static binary, an appservice of the homeserver.

This README describes the tool. How optionfactory runs it (the instance, nginx, the registration, the settings) is in
the `internal-infra` repository.

Threat model, secrets and how to report a vulnerability: [SECURITY.md](SECURITY.md).

## How it works

- **Bots.** The appservice owns `@github`, `@clickup`, `@hooks` and `@hooks-*`: it acts as any of them with its
  token, without passwords or logins, and registers `@github`, `@clickup` and `@hooks` on the homeserver at start.
  Invite a bot to a room and it joins and says how to use it. It refuses encrypted rooms, and leaves a room that turns
  encrypted.
- **Enabling an integration** is up to the admins of the settings, in a direct room with the bot. Element and Cinny
  encrypt direct messages by default and the bot refuses encrypted rooms: create the room with encryption off. The
  bot keeps the secrets in its own account data on the homeserver: nothing to configure or deploy, backed up with the
  homeserver.

  ```
  !github enable                  a new secret for the webhook of the organization, with the URL to give GitHub
  !github rotate                  a new secret, the old one refused at once
  !clickup connect <API token> [workspace id]
                                  creates the webhook of the workspace; the message with the token is redacted
                                  when the bot may, otherwise it asks to delete it
  !clickup disconnect             deletes the webhook
  !github name GitHub             the name of the bot, in every room
  !github avatar                  in reply to an image: the icon of the bot, in every room
  ```
- **Routing lives in the rooms.** Room moderators subscribe a room with a command, and the bot keeps the
  subscriptions in its account data for that room: no database, no configuration to change, backed up with the
  homeserver.

  ```
  !github subscribe acme                               every repository of the organization
  !github subscribe acme/widgets pull_request,workflow_run
  !clickup subscribe Widgets Sprint                    a list or a folder by id or name, or a space by id
  !github list / unsubscribe ...
  ```

  A subscription without events takes them all except the noise of continuous integration and deployments
  (`check_run`, `check_suite`, `workflow_job`, `status`, `deployment`, `deployment_status`, `page_build`). Events
  without their own format get a line with who, what and where.
- **The look**, taken from maubot's GitHub plugin: one sentence per event, `[owner/repo] alice merged pull request #12:
  Title` or `[List] alice moved task Checkout from [to do] to [in progress]`, the state in the verb, no emoji of our
  own. Colour only where GitHub and ClickUp have it: labels, statuses and priorities as pills in their colours. Pushes
  as a list of commits, comments and descriptions quoted and rendered from Markdown (their HTML shown as text, their
  images as links), GitHub's `:shortcodes:` as their emoji (`:sparkles:` as ✨, gitmoji included), no link previews.
  A workflow run is a reaction on the message of its push, `🔵 build` then `🟢 build`, replaced as it goes on; when
  that push is not known (a restart, a room without pushes), a finished run is a message, and a failure notifies.
- **GitHub:** one webhook for the organization, its signature (`X-Hub-Signature-256`) checked on every delivery.
- **ClickUp:** one webhook for the workspace, its signature (`X-Signature`) checked on every delivery. A delivery
  does not say where the task is, so the bot reads the task from the API with the token of the connection, and
  routes it by its list, folder and space.
- **Scripts:** invite `@hooks` to the room, then a moderator creates a hook: `@hooks-<name>` joins, and the URL
  with the token goes to who asked in a new direct room with `@hooks`, so that the other members of the room, guests
  included, never see it. The bot keeps only the hash of the token.

  ```
  !hooks new deploy Deploy          creates @hooks-deploy, named Deploy
  !hooks list / revoke deploy
  !hooks name deploy Deploy bot     who created the hook, or an admin: the name, in every room
  !hooks avatar deploy              in reply to an image: the icon of @hooks-deploy, in every room
  ```

  ```bash
  curl -fsS -d 'widgets 1.4.2 deployed' "$HOOK_URL"
  curl -fsS -H 'Content-Type: application/json' -d '{"text": "backup failed", "level": "error"}' "$HOOK_URL"
  ```

  JSON takes `text`, an optional `html`, `level`: `error` makes a normal message, which notifies, instead of a notice,
  and `mention`: `room` mentions the whole room (`@room`), which needs the bot at the power level the room asks for it,
  50 unless it says otherwise. `:shortcodes:` become their emoji, in `text` and `html`. A hook bot is one identity for
  the whole server: a second room gets the same bot with its own token.
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
server_name = "example.org"
public_url = "https://hooks.example.org"     # where the reverse proxy serves the webhooks
admins = ["@alice:example.org"]    # who may enable and disable the integrations

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
    - {regex: "@github:example\\.org", exclusive: true}
    - {regex: "@clickup:example\\.org", exclusive: true}
    - {regex: "@hooks(-.*)?:example\\.org", exclusive: true}
```

## Status

Done:

- the settings, the listener, the appservice side (token of the homeserver; invitations: join and usage; encrypted
  rooms refused, and left when a room turns encrypted);
- GitHub: `!github enable` and `rotate` for the admins, `!github subscribe`, `list` and `unsubscribe` for the
  moderators, the signature of the deliveries, the events formatted and routed to the organization or the
  repository;
- ClickUp: `!clickup connect` and `disconnect` for the admins, `!clickup subscribe`, `list` and `unsubscribe` for the
  moderators, the signature of the deliveries, the task changes formatted and routed to their list, folder or space;
- script hooks: `!hooks new`, `list`, `revoke`, `name` and `avatar`, `/send/<token>`;
- `!<bot> name` and `!<bot> avatar` for the admins, for `@github` and `@clickup`.

## Building

`make build-release`: `target/x86_64-unknown-linux-musl/release/optionfactory-hooks`, statically linked (musl,
rustls with built-in root certificates). `make publish-github` releases it on GitHub, tagged on the local commit: it
refuses to run with uncommitted changes or with `master` not pushed.
