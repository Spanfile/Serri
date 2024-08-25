#![feature(let_chains)]

use std::fs::read_to_string;

use mio::Token;
use tokio_util::sync::CancellationToken;

use crate::{config::SerriConfig, serial_controller::SerialController};

mod config;
mod serial_controller;
mod util;
mod web;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let serri_config: SerriConfig = toml::from_str(&read_to_string("serri.toml")?)?;
    println!("{serri_config:#?}");

    let (serial_reader_handle, registry, event_tx) = serial_controller::create_serial_reader();
    let cancellation_token = CancellationToken::new();

    let mut controller_handles = Vec::new();
    let mut controllers = Vec::new();

    for (index, port_config) in serri_config.serial_port.iter().enumerate() {
        let token = Token(index);
        let controller =
            SerialController::new(port_config, &serri_config, token, registry.try_clone()?)?;

        let controller_handle =
            controller.run_reader_task(event_tx.subscribe(), cancellation_token.clone());

        controller_handles.push(controller_handle);
        controllers.push(controller);
    }

    web::run(serri_config, controllers).await?;

    cancellation_token.cancel();
    // TODO: wait for tasks and thread to stop

    Ok(())
}
