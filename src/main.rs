#![feature(let_chains)]

mod build {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

use std::fs::read_to_string;

use futures::future::join_all;
use mio::Token;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{config::SerriConfig, serial_controller::SerialController};

mod config;
mod serial_controller;
mod util;
mod web;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                format!(
                    "{}=trace,tower_http=debug,axum=trace",
                    env!("CARGO_CRATE_NAME")
                )
                .into()
            }),
        )
        .with(tracing_subscriber::fmt::layer().without_time())
        .init();

    // TODO: better error message if config file is missing or fails to read etc.
    let serri_config: SerriConfig = toml::from_str(&read_to_string("serri.toml")?)?;
    debug!("{serri_config:#?}");

    let (_serial_reader_handle, registry, event_tx) = serial_controller::create_serial_reader();
    let cancellation_token = CancellationToken::new();

    let mut controllers = Vec::new();
    let mut controller_handles = Vec::new();

    for (index, port_config) in serri_config.serial_port.iter().enumerate() {
        let token = Token(index);
        let controller =
            SerialController::new(port_config, &serri_config, token, registry.try_clone()?);

        let controller_handle =
            controller.run_reader_task(event_tx.subscribe(), cancellation_token.clone());

        controllers.push(controller);
        controller_handles.insert(index, controller_handle);
    }

    web::run(serri_config, controllers).await?;

    // TODO: we're currently not waiting for the one serial reader thread to exit, should we?

    cancellation_token.cancel();
    tokio::select! {
        _ = join_all(controller_handles) => {},
        _ = shutdown_signal() => {
            warn!("Caught another termination, forcefully exiting")
        }
    }

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler")
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
