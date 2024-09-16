use std::{sync::Arc, time::Duration};

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
use serialport::{DataBits, FlowControl, Parity, StopBits};
use tinytemplate::TinyTemplate;
use tokio::{
    sync::{broadcast, mpsc},
    time::Instant,
};
use tracing::{debug, error, error_span, info, warn, Instrument};

use crate::{
    config::SerriConfig,
    serial_controller::{SerialController, SerialReadData},
    web::template::{BaseTemplate, DevicePopoutTemplate, DeviceTemplate, IndexTemplate},
};

const BANNER_TEMPLATE_NAME: &str = "banner";
const PING_INTERVAL: Duration = Duration::from_secs(30);
const MAX_MISSED_PINGS: usize = 5;

#[derive(Debug, Copy, Clone)]
enum WsEvent {
    Ok,
    Ping,
    CloseNormal,
    CloseGoingAway,
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

    controller.clear_history();
    StatusCode::OK
}

#[derive(Deserialize, Serialize)]
struct PreserveHistoryBody {
    preserve_history: bool,
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

#[derive(Serialize)]
struct ActiveConnectionsBody {
    active_connections: usize,
}

async fn get_active_connections(
    Path(device_index): Path<usize>,
    State(state): State<ActiveConnectionsState>,
) -> Json<ActiveConnectionsBody> {
    Json(ActiveConnectionsBody {
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
    let span = error_span!("websocket", device_index);
    let _enter = span.enter();
    info!("New WebSocket connection");

    drop(_enter);
    ws.on_upgrade(move |mut socket| {
        async move {
            let Some(port_config) = serri_config.serial_port.get(device_index) else {
                return;
            };

            let serial_controller = &controllers[device_index];

            if !serial_controller.is_serial_device_open() {
                let span = error_span!(
                    "serial_device_reopen",
                    device = port_config.serial_device.device
                );
                let _enter = span.enter();

                warn!("Serial device not open, trying to open");

                if let Err(e) =
                    serial_controller.reopen_serial_device(port_config, Token(device_index))
                {
                    error!("Couldn't open serial device: {e:?}");

                    // drop the span early to not hold it over the await point
                    drop(_enter);
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: close_code::ERROR,
                            reason: format!("Couldn't open serial device: {e:?}").into(),
                        })))
                        .await;

                    return;
                };

                info!("Serial device reopened");
            }

            let serial_read_rx = serial_controller.get_serial_read_rx();
            let serial_write_tx = serial_controller.get_serial_write_tx();
            let history = serial_controller.get_history();

            if let Some(banner) = serri_config.banner.as_deref()
                && let Some(parameters) = serial_controller.get_serial_device_parameters()
                && send_banner(&mut socket, banner, parameters).await.is_err()
            {
                return;
            }

            if socket.send(history.into()).await.is_err() {
                return;
            }

            *state.active_connections.entry(device_index).or_default() += 1;
            handle_device_ws(socket, serial_read_rx, serial_write_tx).await;

            *state.active_connections.entry(device_index).or_insert(1) -= 1;
        }
        .instrument(span)
    })
}

async fn handle_device_ws(
    mut socket: WebSocket,
    mut serial_read_rx: broadcast::Receiver<SerialReadData>,
    mut serial_write_tx: mpsc::Sender<Vec<u8>>,
) {
    let mut ping_interval = tokio::time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
    let mut missed_pings: usize = 0;

    loop {
        // println!("Main loop");
        tokio::select! {
            ws_msg = socket.recv() => {
                match process_ws_message(ws_msg, &mut serial_write_tx).await {
                    Ok(WsEvent::Ping) => missed_pings = 0,

                    Ok(WsEvent::CloseNormal | WsEvent::CloseGoingAway) => {
                        let _ = socket.send(Message::Close(Some(CloseFrame {
                            code: close_code::NORMAL,
                            reason: "closing".into(),
                        })))
                        .await;

                        break;
                    },

                    Err(e) => {
                        warn!("WebSocket terminated: {e:?}");
                        break;
                    }

                    _ => (),
                }
            }

            _ = ping_interval.tick() => {
                debug!(missed_pings, "ping");
                missed_pings += 1;

                if missed_pings > MAX_MISSED_PINGS {
                    warn!(missed_pings, "Too many missed pings, closing connection");
                    break;
                }

                let _ = socket.send(Message::Ping("ping".as_bytes().to_vec())).await;
            }

            serial_read = serial_read_rx.recv() => {
                if let Err(e) = process_serial_read(serial_read, &mut socket).await {
                    error!("Failed to process serial read event: {e:?}");
                    break;
                }
            }
        }
    }

    debug!("WebSocket handler returning");
}

async fn send_banner(
    socket: &mut WebSocket,
    banner: &str,
    parameters: (String, u32, DataBits, Parity, StopBits, FlowControl),
) -> anyhow::Result<()> {
    let banner_context = create_banner_context(parameters);
    match render_banner_template(banner, banner_context) {
        Ok(rendered) => socket.send(rendered.into()).await?,
        Err(e) => {
            error!("Failed to render banner template: {e:?}");

            socket
                .send("Failed to render banner template".into())
                .await?
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct BannerContext {
    device: String,
    short_params: String,

    baud_rate: u32,
    data_bits: DataBits,
    parity: Parity,
    stop_bits: StopBits,
    flow_control: FlowControl,
}

fn create_banner_context(
    (device, baud_rate, data_bits, parity, stop_bits, flow_control): (
        String,
        u32,
        DataBits,
        Parity,
        StopBits,
        FlowControl,
    ),
) -> BannerContext {
    BannerContext {
        device,
        short_params: format_short_params(data_bits, parity, stop_bits),

        baud_rate,
        data_bits,
        parity,
        stop_bits,
        flow_control,
    }
}

fn format_short_params(data_bits: DataBits, parity: Parity, stop_bits: StopBits) -> String {
    let data_bits: u8 = data_bits.into();
    let stop_bits: u8 = stop_bits.into();

    let parity = match parity {
        Parity::None => "N",
        Parity::Odd => "O",
        Parity::Even => "E",
    };

    format!("{data_bits}{parity}{stop_bits}")
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
) -> anyhow::Result<WsEvent> {
    let Some(Ok(msg)) = ws_msg else {
        warn!("WebSocket client disconnected");
        return Err(anyhow!("WebSocket client disconnected"));
    };

    debug!(?msg);

    match msg {
        Message::Text(text) => serial_write_tx.send(text.as_bytes().to_vec()).await?,
        Message::Ping(_) | Message::Pong(_) => return Ok(WsEvent::Ping),

        Message::Close(Some(close_frame)) if close_frame.code == close_code::NORMAL => {
            info!(?close_frame, "WebSocket client closed connection");
            return Ok(WsEvent::CloseNormal);
        }

        Message::Close(Some(close_frame)) if close_frame.code == close_code::AWAY => {
            info!(?close_frame, "WebSocket client going away");
            return Ok(WsEvent::CloseGoingAway);
        }

        Message::Close(Some(close_frame)) => {
            debug!(
                ?close_frame,
                "WebSocket client closed connection unexpectedly"
            );

            return Err(anyhow!(
                "WebSocket client closed connection unexpectedly. code={} reason=\"{}\"",
                close_frame.code,
                close_frame.reason
            ));
        }

        Message::Close(None) => {
            debug!("WebSocket client closed connection without close frame");
            return Err(anyhow!("WebSocket connection closed unexpectedly"));
        }

        _ => debug!(?msg, "Unhandled WebSocket message"),
    }

    Ok(WsEvent::Ok)
}

async fn process_serial_read(
    serial_read: Result<SerialReadData, broadcast::error::RecvError>,
    socket: &mut WebSocket,
) -> anyhow::Result<()> {
    let serial_read = serial_read?;
    debug!(?serial_read);

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
