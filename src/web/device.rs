use std::sync::Arc;

use anyhow::anyhow;
use askama_axum::{IntoResponse, Response};
use axum::{
    extract::{
        ws::{close_code, CloseFrame, Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::Redirect,
    routing, Extension, Json, Router,
};
use dashmap::DashMap;
use mio::Token;
use serde::{Deserialize, Serialize};
use tinytemplate::TinyTemplate;
use tokio::sync::{broadcast, mpsc};

use crate::{
    config::{SerialPortConfig, SerriConfig},
    serial_controller::{SerialController, SerialReadData},
    web::template::{BaseTemplate, DevicePopoutTemplate, DeviceTemplate, IndexTemplate},
};

const BANNER_TEMPLATE_NAME: &str = "banner";

#[derive(Serialize)]
struct BannerContext<'a> {
    device: &'a str,
}

#[derive(Deserialize, Serialize)]
struct PreserveHistoryBody {
    preserve_history: bool,
}

#[derive(Serialize)]
struct ActiveConnections {
    active_connections: usize,
}

#[derive(Debug, Clone)]
struct ActiveConnectionsState {
    active_connections: Arc<DashMap<usize, usize>>,
}

pub fn router() -> Router {
    Router::new()
        .route("/", routing::get(|| async { Redirect::to("/") }))
        .route("/:device_index", routing::get(get_device))
        .route("/:device_index/ws", routing::get(get_device_ws))
        .route(
            "/:device_index/clear_history",
            routing::post(post_clear_history),
        )
        .route(
            "/:device_index/preserve_history",
            routing::get(get_preserve_history).post(post_preserve_history),
        )
        .route(
            "/:device_index/active_connections",
            routing::get(get_active_connections),
        )
        .with_state(ActiveConnectionsState {
            active_connections: Arc::new(DashMap::new()),
        })
}

#[derive(Deserialize)]
struct DeviceQuery {
    #[serde(default, rename = "p")]
    popout: bool,
}

async fn get_device(
    Path(device_index): Path<usize>,
    Query(query): Query<DeviceQuery>,
    Extension(serri_config): Extension<Arc<SerriConfig>>,
    Extension(controllers): Extension<Arc<Vec<SerialController>>>,
) -> Response {
    if device_index >= serri_config.serial_port.len() {
        return Redirect::to("/").into_response();
    }

    let controller = &controllers[device_index];
    let device_template = DeviceTemplate {
        index_template: IndexTemplate {
            base_template: BaseTemplate {
                serri_config,
                active_path: "/",
            },
            active_device_index: Some(device_index),
        },
        preserve_history: controller.get_preserve_history(),
    };

    if !query.popout {
        device_template.into_response()
    } else {
        DevicePopoutTemplate(device_template).into_response()
    }
}

async fn post_clear_history(
    Path(device_index): Path<usize>,
    Extension(controllers): Extension<Arc<Vec<SerialController>>>,
) -> StatusCode {
    let Some(controller) = controllers.get(device_index) else {
        return StatusCode::NOT_FOUND;
    };

    controller.clear_history().await;
    StatusCode::OK
}

async fn get_preserve_history(
    Path(device_index): Path<usize>,
    Extension(controllers): Extension<Arc<Vec<SerialController>>>,
) -> Response {
    let Some(controller) = controllers.get(device_index) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    Json(PreserveHistoryBody {
        preserve_history: controller.get_preserve_history(),
    })
    .into_response()
}

async fn post_preserve_history(
    Path(device_index): Path<usize>,
    Extension(controllers): Extension<Arc<Vec<SerialController>>>,
    Json(preserve_history_body): Json<PreserveHistoryBody>,
) -> StatusCode {
    let Some(controller) = controllers.get(device_index) else {
        return StatusCode::NOT_FOUND;
    };

    // TODO: if disabling preserve, clear history?
    controller.set_preserve_history(preserve_history_body.preserve_history);
    StatusCode::OK
}

async fn get_active_connections(
    Path(device_index): Path<usize>,
    State(state): State<ActiveConnectionsState>,
) -> Json<ActiveConnections> {
    Json(ActiveConnections {
        active_connections: *state.active_connections.entry(device_index).or_default(),
    })
}

async fn get_device_ws(
    Path(device_index): Path<usize>,
    ws: WebSocketUpgrade,
    Extension(serri_config): Extension<Arc<SerriConfig>>,
    Extension(controllers): Extension<Arc<Vec<SerialController>>>,
    State(state): State<ActiveConnectionsState>,
) -> Response {
    println!("New WS connection for device {device_index}");

    ws.on_upgrade(move |mut socket| async move {
        let Some(port_config) = serri_config.serial_port.get(device_index) else {
            return;
        };

        let serial_controller = &controllers[device_index];

        if !serial_controller.is_serial_device_open().await {
            println!("Serial device not open, trying to open...");

            if let Err(e) = serial_controller
                .reopen_serial_device(port_config, Token(device_index))
                .await
            {
                println!("Couldn't open serial device: {e:?}");

                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: close_code::ERROR,
                        reason: format!("Couldn't open serial device: {e:?}").into(),
                    })))
                    .await;

                return;
            };

            println!("Serial device reopened");
        }

        let serial_read_rx = serial_controller.get_serial_read_rx();
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

        *state.active_connections.entry(device_index).or_default() += 1;
        handle_device_ws(socket, serial_read_rx, serial_write_tx).await;

        // technically this could panic with an underflow if for whatever reason the entry
        // disappears before this is called
        *state.active_connections.entry(device_index).or_default() -= 1;
    })
}

async fn handle_device_ws(
    mut socket: WebSocket,
    mut serial_read_rx: broadcast::Receiver<SerialReadData>,
    mut serial_write_tx: mpsc::Sender<Vec<u8>>,
) {
    // TODO: websocket pings?

    loop {
        // println!("Main loop");
        tokio::select! {
            ws_msg = socket.recv() => if let Err(e) = process_ws_message(ws_msg, &mut serial_write_tx).await {
                println!("Failed to process WebSocket message: {e:?}");
                break;
            },

            serial_read = serial_read_rx.recv() => if let Err(e) = process_serial_read(serial_read, &mut socket).await {
                println!("Failed to process serial read event: {e:?}");
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
    template_renderer.add_template(BANNER_TEMPLATE_NAME, banner)?;

    let rendered = template_renderer.render(BANNER_TEMPLATE_NAME, &context)?;
    Ok(rendered)
}

async fn process_ws_message(
    ws_msg: Option<Result<Message, axum::Error>>,
    serial_write_tx: &mut mpsc::Sender<Vec<u8>>,
) -> anyhow::Result<()> {
    let Some(Ok(msg)) = ws_msg else {
        println!("WS client disconnected");
        return Err(anyhow!("WebSocket client disconnected"));
    };

    // println!("{msg:?}");

    match msg {
        Message::Text(text) => serial_write_tx.send(text.as_bytes().to_vec()).await?,
        Message::Close(close_frame) => {
            println!("WS client closed connection {close_frame:?}");
            return Err(anyhow!(
                "WebSocket client closed connection: {close_frame:?}"
            ));
        }

        _ => println!("Unhandled WS message: {msg:?}"),
    }

    Ok(())
}

async fn process_serial_read(
    serial_read: Result<SerialReadData, broadcast::error::RecvError>,
    socket: &mut WebSocket,
) -> anyhow::Result<()> {
    let serial_read = serial_read?;

    match serial_read {
        SerialReadData::Data(data) => socket.send(data.into()).await?,
        SerialReadData::Error => {
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code: close_code::ERROR,
                    reason: "Serial device returned error".into(),
                })))
                .await;

            return Err(anyhow!("Serial device returned error"));
        }
    }

    Ok(())
}
