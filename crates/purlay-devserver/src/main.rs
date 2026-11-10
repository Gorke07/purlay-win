//! `cargo run -p purlay-devserver -- --port 4390 [--live]`

use purlay_devserver::{Config, spawn};
use std::net::IpAddr;
use std::time::Duration;

const USAGE: &str = "\
purlay-devserver: an in-memory fake Purlay server for development

USAGE:
    purlay-devserver [--port N] [--host IP] [--live] [--live-every SECONDS] [--origin URL] [--empty] [--auto-sign-in USER]

OPTIONS:
    --port N              Port to listen on (default 4390; 0 picks one)
    --host IP             Address to listen on (default 127.0.0.1)
    --live                A bot posts, replies, reacts and types in #random now and then
    --live-every SECONDS  About how often the bot acts (default 20)
    --origin URL          The appUrl to advertise (default http://<host>:<port>)
    --empty               People and channels only, no messages
    --auto-sign-in USER   The sign-in page signs straight in as USER
";

fn parse() -> Result<Config, String> {
    let mut config = Config { port: 4390, ..Config::default() };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--port" | "-p" => config.port = value("--port")?.parse().map_err(|_| "--port is not a number")?,
            "--host" => config.host = value("--host")?.parse::<IpAddr>().map_err(|_| "--host is not an IP address")?,
            "--live" => config.live = true,
            "--live-every" => {
                let s: f64 = value("--live-every")?.parse().map_err(|_| "--live-every is not a number")?;
                config.live_every = Duration::from_secs_f64(s.max(1.0));
            }
            "--origin" => config.public_origin = Some(value("--origin")?),
            "--empty" => config.seed_messages = false,
            "--auto-sign-in" => config.auto_sign_in = Some(value("--auto-sign-in")?),
            "--help" | "-h" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(config)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,purlay_devserver=info")),
        )
        .init();
    let config = match parse() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let live = config.live;
    let (addr, server) = match spawn(config).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not listen: {e}");
            std::process::exit(1);
        }
    };
    println!("Purlay dev server (contract {}) listening on {addr}", purlay_devserver::SERVER_VERSION);
    println!("  server address for the app: {}", server.origin());
    println!("  workspace: Acme (acme), live mode: {}", if live { "on" } else { "off" });
    println!("  sign in: the app opens {}/sign-in/native?..., pick a person there:", server.origin());
    for p in server.people() {
        println!("    @{:<6} {}", p.username, p.display_name);
    }
    println!("  Ctrl+C to stop.");
    let _ = tokio::signal::ctrl_c().await;
    println!("stopping...");
    server.shutdown().await;
}
