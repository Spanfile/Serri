mod template;

use std::{
    io::{BufReader, ErrorKind, Write},
    sync::Arc,
};

use axum::{
    extract::{
        ws::{close_code, CloseFrame, Message, WebSocket},
        Path, WebSocketUpgrade,
    },
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing, Extension, Router,
};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use serde::Serialize;
use serialport::{SerialPort, TTYPort};
use tinytemplate::TinyTemplate;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;

use crate::{
    config::SerriConfig,
    util::ReadUpTo,
    web::template::{
        BaseTemplate, ConfigTemplate, DeviceTemplate, IndexTemplate, NotFoundTemplate,
    },
};

const SERIAL_READ_BUFFER_SIZE: usize = 256;

#[derive(Serialize)]
struct BannerContext {
    device: String,
    device_index: usize,
}

pub async fn run(serri_config: SerriConfig) -> anyhow::Result<()> {
    let serri_config = Arc::new(serri_config);
    let app = Router::new()
        .route("/", routing::get(root))
        .route("/device", routing::get(|| async { Redirect::to("/") }))
        .route("/device/:device_index", routing::get(device))
        .route("/device/:device_index/ws", routing::get(device_ws))
        .route("/config", routing::get(config))
        .nest_service("/dist", ServeDir::new("dist"))
        .fallback(not_found)
        .layer(Extension(Arc::clone(&serri_config)));

    let listener = tokio::net::TcpListener::bind(serri_config.listen).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn not_found(Extension(serri_config): Extension<Arc<SerriConfig>>) -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        NotFoundTemplate {
            base_template: BaseTemplate {
                serri_config,
                active_path: "",
            },
        },
    )
}

async fn root(Extension(serri_config): Extension<Arc<SerriConfig>>) -> IndexTemplate {
    IndexTemplate {
        base_template: BaseTemplate {
            serri_config,
            active_path: "/",
        },
        active_device_index: None,
    }
}

async fn config(Extension(serri_config): Extension<Arc<SerriConfig>>) -> ConfigTemplate {
    ConfigTemplate {
        base_template: BaseTemplate {
            serri_config,
            active_path: "/config",
        },
    }
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
) -> Response {
    println!("New WS connection for device {device_index}");

    ws.on_upgrade(move |mut socket| async move {
        if let Some(port_config) = serri_config.serial_port.get(device_index) {
            let serial_port = match port_config.serial_device.open() {
                Ok(port) => port,
                Err(e) => {
                    println!(
                        "Failed to open serial device {}: {e}",
                        port_config.serial_device.device
                    );

                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: close_code::ERROR,
                            reason: format!("Failed to open serial device: {e}",).into(),
                        })))
                        .await;

                    return;
                }
            };

            let banner = serri_config.banner.clone();

            if let Some(banner) = banner {
                let banner_context = create_banner_context(&serial_port, device_index);
                match render_banner_template(banner, banner_context) {
                    Ok(rendered) => {
                        let Ok(_) = socket.send(Message::Text(rendered)).await else {
                            return;
                        };
                    }

                    Err(e) => {
                        println!("Failed to render banner template: {e:?}");

                        let Ok(_) = socket
                            .send(Message::Text(
                                "Failed to render banner template".to_string(),
                            ))
                            .await
                        else {
                            return;
                        };
                    }
                }
            }

            handle_device_ws(socket, serial_port).await
        }
    })
}

fn render_banner_template(banner: String, context: BannerContext) -> anyhow::Result<String> {
    let mut template_renderer = TinyTemplate::new();
    template_renderer.add_template("banner", &banner)?;

    let rendered = template_renderer.render("banner", &context)?;
    Ok(rendered)
}

fn create_banner_context(serial_port: &TTYPort, device_index: usize) -> BannerContext {
    BannerContext {
        device: serial_port
            .name()
            .unwrap_or_else(|| "unnamed device".to_string()),
        device_index,
    }
}

async fn handle_device_ws(mut socket: WebSocket, mut serial_port: TTYPort) {
    let Ok(cloned_port) = serial_port.try_clone_native() else {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: close_code::ERROR,
                reason: "Failed to clone serial device".into(),
            })))
            .await;

        return;
    };

    let serial_reader = BufReader::new(cloned_port);

    // TODO: fork and update tokio-serial if it starts working again?
    // i tried to be a good boy and use polling to read from the serial ports but both of my USB
    // serial adapters misbehaved when polled so blocking reader thread per connection it is

    let cancel_token = CancellationToken::new();
    let read_cancel_token = cancel_token.clone();
    let (read_tx, mut read_rx) = mpsc::channel(32);

    let reader_task =
        std::thread::spawn(move || serial_reader_task(serial_reader, read_tx, read_cancel_token));

    // TODO: websocket pings?
    let (mut ws_tx, mut ws_rx) = socket.split();

    loop {
        // println!("Main loop");
        tokio::select! {
            ws_msg = ws_rx.next() => if !process_ws_message(ws_msg, &mut serial_port) {
                break;
            },

            read_buf = read_rx.recv() => if !process_serial_read(read_buf, &mut ws_tx).await {
                break;
            }
        }
    }

    println!("Closing WS for {:?}", serial_port.name());

    cancel_token.cancel();
    if let Err(e) = reader_task.join() {
        let reason = if let Ok(error) = e.downcast::<String>() {
            format!("Failed to read from serial device: {error}").into()
        } else {
            "Failed to read from serial device".into()
        };

        println!("Serial reader task failed: {reason}");

        let _ = ws_tx
            .send(Message::Close(Some(CloseFrame {
                code: close_code::ERROR,
                reason,
            })))
            .await;

        return;
    }

    let _ = ws_tx.close().await;
}

fn serial_reader_task(
    mut serial_reader: BufReader<TTYPort>,
    read_tx: mpsc::Sender<Vec<u8>>,
    read_cancel_token: CancellationToken,
) {
    let mut buf = [0u8; SERIAL_READ_BUFFER_SIZE];
    loop {
        match serial_reader.read_up_to(&mut buf) {
            Ok(amt) if amt > 0 => {
                println!("Read {amt} bytes from {:?}", serial_reader.get_ref().name());
                read_tx
                    .blocking_send(buf[..amt].to_vec())
                    .expect("Read channel closed");
            }

            // my read_up_to should never return TimedOut but hey
            Err(e) if e.kind() != ErrorKind::TimedOut => {
                panic!("{e}");
            }

            _ => (),
        }

        if read_cancel_token.is_cancelled() {
            println!(
                "Reader closing for {:?} (cancelled)",
                serial_reader.get_ref().name()
            );

            break;
        }
    }
}

fn process_ws_message(
    ws_msg: Option<Result<Message, axum::Error>>,
    serial_port: &mut TTYPort,
) -> bool {
    let Some(Ok(msg)) = ws_msg else {
        println!("WS client disconnected for {:?}", serial_port.name());
        return false;
    };

    // println!("{msg:?}");

    match msg {
        Message::Text(text) => {
            if let Err(e) = serial_port.write_all(text.as_bytes()) {
                println!(
                    "Writing to serial port ({:?}) failed: {e:?}",
                    serial_port.name()
                );

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
    read_buf: Option<Vec<u8>>,
    ws_tx: &mut SplitSink<WebSocket, Message>,
) -> bool {
    let Some(read_buf) = read_buf else {
        println!("Reader task channel closed");
        return false;
    };

    if let Err(e) = ws_tx.send(Message::Binary(read_buf)).await {
        println!("Writing to WS failed: {e:?}");
        return false;
    }

    true
}
