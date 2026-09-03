//! Entry point for the `Anole` local `REST` API server (G-33).

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;

use formatwright_server::routes::{AppState, build_router};

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    if let Err(error) = runtime.block_on(serve()) {
        eprintln!("formatwright-server: {error}");
        std::process::exit(1);
    }
}

async fn serve() -> Result<(), String> {
    let bind = parse_bind_address(std::env::args().skip(1))?;
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .map_err(|error| format!("failed to bind {bind}: {error}"))?;
    let state = AppState::new(default_state_db());
    let app = build_router(state);
    println!("formatwright-server listening on http://{bind}");
    axum::serve(listener, app)
        .await
        .map_err(|error| format!("server error: {error}"))
}

/// Parses an optional `--bind <addr>` flag; defaults to loopback only.
fn parse_bind_address<I>(args: I) -> Result<SocketAddr, String>
where
    I: Iterator<Item = String>,
{
    const DEFAULT_BIND: &str = "127.0.0.1:8787";
    let mut bind = DEFAULT_BIND.to_owned();
    let mut iter = args.peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--bind" => {
                bind = iter
                    .next()
                    .ok_or_else(|| "--bind requires a socket address".to_owned())?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    bind.parse()
        .map_err(|error| format!("invalid bind address {bind}: {error}"))
}

fn default_state_db() -> PathBuf {
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(root)
            .join("FormatWright")
            .join("jobs.sqlite3");
    }

    #[cfg(target_os = "macos")]
    if let Some(root) = std::env::var_os("HOME") {
        return PathBuf::from(root)
            .join("Library")
            .join("Application Support")
            .join("FormatWright")
            .join("jobs.sqlite3");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(root) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(root)
            .join("formatwright")
            .join("jobs.sqlite3");
    } else if let Some(root) = std::env::var_os("HOME") {
        return PathBuf::from(root)
            .join(".local")
            .join("state")
            .join("formatwright")
            .join("jobs.sqlite3");
    }

    PathBuf::from("formatwright-jobs.sqlite3")
}
