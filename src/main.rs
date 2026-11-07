mod appservice;
mod clickup;
mod github;
mod matrix;

use axum::extract::{ConnectInfo, DefaultBodyLimit, Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};

/// Events of GitHub, ClickUp and scripts posted to Matrix rooms, each source as its own bot.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Receives the webhooks through nginx and talks to the homeserver as its appservice.
    Serve {
        /// TOML: the address, the homeserver, the tokens of the appservice and the admins (README.md).
        #[arg(long, default_value = "/etc/optionfactory-hooks/hooks.conf")]
        config: PathBuf,
    },
}

/// The file given to `serve --config`. The secrets of the webhooks are not here: an admin sets them through the
/// bots, which keep them in their account data.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    /// For the reverse proxy, which forwards the webhooks, and the homeserver, which delivers the room events of the
    /// bots.
    #[serde(default = "Settings::listen")]
    listen: SocketAddr,
    /// The reverse proxy: only from it is the client address in `X-Forwarded-For` believed.
    trusted_proxy: Option<IpAddr>,
    /// Client-server API of the homeserver.
    #[serde(default = "Settings::homeserver")]
    homeserver: String,
    /// The part after the colon in the user IDs of the bots.
    server_name: String,
    /// The users who may enable and disable the integrations.
    admins: Vec<String>,
    appservice: Appservice,
}

/// The tokens of the registration of the appservice in the homeserver.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Appservice {
    as_token: String,
    hs_token: String,
}

impl Settings {
    fn listen() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 8009))
    }
    fn homeserver() -> String {
        "http://127.0.0.1:8008".to_string()
    }

    fn read(path: &Path) -> Result<Settings, String> {
        std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| Settings::parse(&text))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    fn parse(text: &str) -> Result<Settings, String> {
        let settings: Settings = toml::from_str(text).map_err(|e| e.to_string())?;
        let suffix = format!(":{}", settings.server_name);
        match settings
            .admins
            .iter()
            .find(|a| !a.starts_with('@') || !a.ends_with(&suffix))
        {
            Some(admin) => Err(format!("admin {admin} is not a user of {}", settings.server_name)),
            None => Ok(settings),
        }
    }
}

fn main() -> ExitCode {
    let outcome = match Cli::parse().command {
        Command::Serve { config } => Settings::read(&config).and_then(serve),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn serve(settings: Settings) -> Result<(), String> {
    let Settings {
        listen,
        trusted_proxy: proxy,
        homeserver,
        server_name,
        admins: _, // for the admin commands, in the next step
        appservice: Appservice { as_token, hs_token },
    } = settings;
    let matrix = Arc::new(matrix::Matrix::new(homeserver, server_name, as_token));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(listen)
            .await
            .map_err(|e| format!("{listen}: {e}"))?;
        let secrets = {
            let matrix = matrix.clone();
            tokio::task::spawn_blocking(move || {
                [(github::BOT, github::SETTINGS), (clickup::BOT, clickup::SETTINGS)].map(|(bot, kind)| {
                    matrix.register(bot)?;
                    let secret = matrix.account_data(bot, kind)?;
                    Ok::<_, String>(secret.and_then(|data| data["secret"].as_str().map(str::to_string)))
                })
            })
            .await
            .map_err(|e| e.to_string())?
        };
        let [github_secret, clickup_secret] = secrets;
        // the reverse proxy forwards /github and /clickup only: the appservice paths are for the homeserver
        let app = appservice::transactions(hs_token, matrix.clone())
            .merge(github::webhook(github_secret?, matrix.clone()))
            .merge(clickup::webhook(clickup_secret?, matrix.clone()));
        // the payloads of big pushes reach a few megabytes
        let app = app
            .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
            .layer(middleware::from_fn_with_state(proxy, client));
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(terminated())
            .await
            .map_err(|e| e.to_string())
    })
}

/// Who made the request: the address the reverse proxy forwards (`X-Forwarded-For`, set by its `forwarded.conf`) when
/// the request comes from it, the address of the connection otherwise.
#[derive(Clone, Copy)]
pub struct Client(pub IpAddr);

async fn client(
    State(proxy): State<Option<IpAddr>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    let forwarded = request.headers().get("X-Forwarded-For").and_then(|v| v.to_str().ok());
    let address = forwarded_for(peer.ip(), forwarded, proxy);
    request.extensions_mut().insert(Client(address));
    next.run(request).await
}

/// The proxy appends the address it saw: the last one in the list, the others came with the request.
fn forwarded_for(peer: IpAddr, forwarded: Option<&str>, proxy: Option<IpAddr>) -> IpAddr {
    forwarded
        .filter(|_| proxy == Some(peer))
        .and_then(|list| list.rsplit(',').next())
        .and_then(|last| last.trim().parse().ok())
        .unwrap_or(peer)
}

async fn terminated() {
    match signal(SignalKind::terminate()) {
        Ok(mut terminate) => terminate.recv().await,
        Err(_) => std::future::pending().await,
    };
}

#[cfg(test)]
mod tests {
    use super::Settings;
    use std::net::IpAddr;

    #[test]
    fn the_settings_of_the_readme_are_read_and_mistakes_are_refused() {
        let readme = include_str!("../README.md");
        let example = readme
            .split("```toml\n")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .unwrap();
        let settings = Settings::parse(example).unwrap();
        assert_eq!(settings.admins, ["@roberto:optionfactory.net"]);
        assert!(Settings::parse(&example.replace("trusted_proxy", "trusted_proxi")).is_err());
        assert!(Settings::parse(&example.replace("@roberto:optionfactory.net", "roberto@optionfactory.net")).is_err());
    }

    #[test]
    fn the_forwarded_address_counts_only_when_the_proxy_sends_it() {
        let proxy: IpAddr = "172.18.0.10".parse().unwrap();
        let elsewhere: IpAddr = "172.18.0.13".parse().unwrap();
        let client: IpAddr = "140.82.115.1".parse().unwrap();
        assert_eq!(super::forwarded_for(proxy, Some("140.82.115.1"), Some(proxy)), client);
        assert_eq!(
            super::forwarded_for(proxy, Some("10.0.0.1, 140.82.115.1"), Some(proxy)),
            client
        );
        assert_eq!(
            super::forwarded_for(elsewhere, Some("140.82.115.1"), Some(proxy)),
            elsewhere
        );
        assert_eq!(super::forwarded_for(proxy, Some("140.82.115.1"), None), proxy);
    }
}
