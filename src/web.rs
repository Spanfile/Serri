mod template;

use std::{
    io::{BufReader, ErrorKind, Write},
    sync::Arc,
};

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket},
        Path, WebSocketUpgrade,
    },
    http::StatusCode,
    response::{Redirect, Response},
    routing, Extension, Router,
};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use serialport::TTYPort;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;

use crate::{
    config::SerriConfig,
    util::ReadUpTo,
    web::template::{DeviceTemplate, IndexTemplate},
};

const SERIAL_READ_BUFFER_SIZE: usize = 256;

pub async fn run(config: SerriConfig) -> anyhow::Result<()> {
    let config = Arc::new(config);
    let app = Router::new()
        .route("/", routing::get(root))
        .route("/device", routing::get(|| async { Redirect::to("/") }))
        .route("/device/:device_index", routing::get(device))
        .route("/device/:device_index/ws", routing::get(device_ws))
        .nest_service("/dist", ServeDir::new("dist"))
        .layer(Extension(Arc::clone(&config)));

    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn root(Extension(config): Extension<Arc<SerriConfig>>) -> IndexTemplate {
    IndexTemplate {
        config: Arc::clone(&config),
        active_device_index: None,
        active_path: String::from("/"),
    }
}

async fn device(
    Path(device_index): Path<usize>,
    Extension(config): Extension<Arc<SerriConfig>>,
) -> DeviceTemplate {
    DeviceTemplate {
        index_template: IndexTemplate {
            config: Arc::clone(&config),
            active_device_index: Some(device_index),
            active_path: String::from("/"),
        },
    }
}

async fn device_ws(
    Path(device_index): Path<usize>,
    ws: WebSocketUpgrade,
    Extension(config): Extension<Arc<SerriConfig>>,
) -> Response {
    println!("New WS connection for device {device_index}");
    if let Some(port_config) = config.serial_port.get(device_index) {
        let port = match port_config.serial_device.open() {
            Ok(port) => port,
            Err(e) => {
                println!(
                    "Failed to open serial device {}: {e}",
                    port_config.serial_device.device
                );

                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::new(format!("Failed to open serial device: {e}")))
                    .unwrap();
            }
        };

        let banner = config.banner.clone();

        return ws.on_upgrade(move |mut socket| async {
            if let Some(banner) = banner
                && let Err(e) = socket.send(Message::Text(banner)).await
            {
                println!("Failed to send banner to WS: {e}");
                return;
            }

            handle_device_ws(socket, port).await
        });
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::new(String::from("Device not found")))
        .unwrap()
}

async fn handle_device_ws(socket: WebSocket, mut serial_port: TTYPort) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let reader = serial_port
        .try_clone_native()
        .expect("failed to clone serial port");

    let cancel_token = CancellationToken::new();
    let read_cancel_token = cancel_token.clone();
    let (read_tx, mut read_rx) = tokio::sync::mpsc::channel(32);

    let reader_task = std::thread::spawn(move || {
        let mut buf_reader = BufReader::with_capacity(SERIAL_READ_BUFFER_SIZE, reader);
        loop {
            // TODO: polled read

            let mut buf = [0u8; SERIAL_READ_BUFFER_SIZE];
            match buf_reader.read_up_to(&mut buf) {
                Ok(amt) if amt > 0 => {
                    println!("Read {amt} bytes from serial");
                    read_tx
                        .blocking_send(buf[..amt].to_vec())
                        .expect("failed to notify read buf");
                }

                // my read_up_to should never return TimedOut but hey
                Err(e) if e.kind() != ErrorKind::TimedOut => {
                    println!("Failed to read from serial port: {e:?}");
                    break;
                }

                _ => {
                    // println!("Timed out")
                }
            }

            if read_cancel_token.is_cancelled() {
                println!("Reader closing");
                break;
            }
        }
    });

    // TODO: websocket pings

    loop {
        tokio::select! {
            ws_msg = ws_rx.next() => if !process_ws_message(ws_msg, &mut serial_port) {
                break;
            },

            read_buf = read_rx.recv() => if !process_serial_read(read_buf, &mut ws_tx).await {
                break;
            }
        }
    }

    println!("WS closing");

    cancel_token.cancel();
    if let Err(e) = reader_task.join() {
        println!("Reader task failed: {e:?}");
    }
}

fn process_ws_message(
    ws_msg: Option<Result<Message, axum::Error>>,
    serial_port: &mut TTYPort,
) -> bool {
    if let Some(Ok(msg)) = ws_msg {
        println!("{msg:?}");

        if let Message::Text(text) = msg
            && let Err(e) = serial_port.write_all(text.as_bytes())
        {
            println!("Writing to serial port failed: {e:?}");
            return false;
        }

        true
    } else {
        false
    }
}

async fn process_serial_read(
    read_buf: Option<Vec<u8>>,
    ws_tx: &mut SplitSink<WebSocket, Message>,
) -> bool {
    if let Some(read_buf) = read_buf {
        if let Err(e) = ws_tx.send(Message::Binary(read_buf)).await {
            println!("Writing to WS failed: {e:?}");
            return false;
        }

        true
    } else {
        println!("Reader task channel closed");
        false
    }
}
