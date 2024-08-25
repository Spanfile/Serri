use std::sync::Arc;

use askama_axum::{IntoResponse, Response};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, WebSocketUpgrade,
    },
    response::Redirect,
    routing, Extension, Router,
};
use serde::Serialize;
use tinytemplate::TinyTemplate;
use tokio::sync::{broadcast, mpsc};

use crate::{
    config::{SerialPortConfig, SerriConfig},
    serial_controller::SerialController,
    web::template::{BaseTemplate, DeviceTemplate, IndexTemplate},
};

#[derive(Serialize)]
struct BannerContext<'a> {
    device: &'a str,
}

pub fn router() -> Router {
    Router::new()
        .route("/", routing::get(|| async { Redirect::to("/") }))
        .route("/:device_index", routing::get(device))
        .route("/:device_index/ws", routing::get(device_ws))
}

async fn device(
    Path(device_index): Path<usize>,
    Extension(serri_config): Extension<Arc<SerriConfig>>,
) -> Response {
    if device_index >= serri_config.serial_port.len() {
        return Redirect::to("/").into_response();
    }

    DeviceTemplate {
        index_template: IndexTemplate {
            base_template: BaseTemplate {
                serri_config,
                active_path: "/",
            },
            active_device_index: Some(device_index),
        },
    }
    .into_response()
}

async fn device_ws(
    Path(device_index): Path<usize>,
    ws: WebSocketUpgrade,
    Extension(serri_config): Extension<Arc<SerriConfig>>,
    Extension(controllers): Extension<Arc<Vec<SerialController>>>,
) -> Response {
    println!("New WS connection for device {device_index}");

    ws.on_upgrade(move |mut socket| async move {
        let Some(port_config) = serri_config.serial_port.get(device_index) else {
            return;
        };

        let serial_controller = &controllers[device_index];
        let serial_read_rx = serial_controller.subscribe_serial_read();
        let serial_write_tx = serial_controller.get_serial_write_tx();
        let history = serial_controller.get_history().await;

        if let Some(banner) = serri_config.banner.as_deref()
            && send_banner(&mut socket, banner, port_config).await.is_err()
        {
            return;
        };

        if socket.send(history.into()).await.is_err() {
            return;
        }

        handle_device_ws(socket, serial_read_rx, serial_write_tx).await
    })
}

async fn handle_device_ws(
    mut socket: WebSocket,
    mut serial_read_rx: broadcast::Receiver<Vec<u8>>,
    mut serial_write_tx: mpsc::Sender<Vec<u8>>,
) {
    // TODO: websocket pings?

    loop {
        // println!("Main loop");
        tokio::select! {
            ws_msg = socket.recv() => if !process_ws_message(ws_msg, &mut serial_write_tx).await {
                break;
            },

            serial_read = serial_read_rx.recv() => if let Err(e) = process_serial_read(serial_read, &mut socket).await {
                println!("Failed to process serial read: {e:?}");
                break;
            }
        }
    }

    println!("Closing WS");
    let _ = socket.close().await;
}

async fn send_banner(
    socket: &mut WebSocket,
    banner: &str,
    port_config: &SerialPortConfig,
) -> anyhow::Result<()> {
    let banner_context = create_banner_context(port_config);
    match render_banner_template(banner, banner_context) {
        Ok(rendered) => socket.send(rendered.into()).await?,
        Err(e) => {
            println!("Failed to render banner template: {e:?}");

            socket
                .send("Failed to render banner template".into())
                .await?
        }
    }

    Ok(())
}

fn create_banner_context(port_config: &SerialPortConfig) -> BannerContext {
    BannerContext {
        device: &port_config.serial_device.device,
    }
}

fn render_banner_template(banner: &str, context: BannerContext) -> anyhow::Result<String> {
    let mut template_renderer = TinyTemplate::new();
    template_renderer.add_template("banner", banner)?;

    let rendered = template_renderer.render("banner", &context)?;
    Ok(rendered)
}

async fn process_ws_message(
    ws_msg: Option<Result<Message, axum::Error>>,
    serial_write_tx: &mut mpsc::Sender<Vec<u8>>,
) -> bool {
    let Some(Ok(msg)) = ws_msg else {
        println!("WS client disconnected");
        return false;
    };

    // println!("{msg:?}");

    match msg {
        Message::Text(text) => {
            if serial_write_tx
                .send(text.as_bytes().to_vec())
                .await
                .is_err()
            {
                println!("Serial TX channel closed",);
                return false;
            }
        }

        Message::Close(close_frame) => {
            println!("WS client closed connection {close_frame:?}");
            return false;
        }

        _ => println!("Unhandled WS message: {msg:?}"),
    }

    true
}

async fn process_serial_read(
    serial_read: Result<Vec<u8>, broadcast::error::RecvError>,
    socket: &mut WebSocket,
) -> anyhow::Result<()> {
    let serial_read = serial_read?;
    socket.send(serial_read.into()).await?;

    Ok(())
}
