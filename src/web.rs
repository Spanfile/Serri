mod template;

use std::{
    io::{ErrorKind, Read, Write},
    sync::Arc,
};

use askama_axum::IntoResponse;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, WebSocketUpgrade,
    },
    response::Redirect,
    routing, Extension, Router,
};
use futures_util::{SinkExt, StreamExt};
use serialport::TTYPort;
use tower_http::services::ServeDir;

use crate::{
    config::SerriConfig,
    web::template::{DeviceTemplate, IndexTemplate},
};

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
    }
}

async fn device(
    Path(device_index): Path<usize>,
    Extension(config): Extension<Arc<SerriConfig>>,
) -> DeviceTemplate {
    DeviceTemplate {
        index_template: IndexTemplate {
            config: Arc::clone(&config),
        },
        device_index,
    }
}

async fn device_ws(
    Path(device_index): Path<usize>,
    ws: WebSocketUpgrade,
    Extension(config): Extension<Arc<SerriConfig>>,
) -> impl IntoResponse {
    println!("New WS connection for device {device_index}");
    if let Some(port_config) = config.serial_port.get(device_index) {
        let port = port_config
            .serial_device
            .open()
            .expect("failed to open serial port");

        return ws.on_upgrade(move |socket| handle_device_ws(socket, port));
    }

    panic!("pls fix")
}

async fn handle_device_ws(socket: WebSocket, mut serial_port: TTYPort) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut reader = serial_port
        .try_clone_native()
        .expect("failed to clone serial port");

    let (read_tx, mut read_rx) = tokio::sync::mpsc::channel(32);

    let reader_task = tokio::task::spawn_blocking(move || loop {
        let mut buf = [0u8; 1024];

        match reader.read(&mut buf) {
            Ok(amt) => read_tx
                .blocking_send(buf[..amt].to_vec())
                .expect("failed to send read buf"),
            Err(e) => match e.kind() {
                ErrorKind::TimedOut => (),
                _ => println!("failed to read from serial port: {e:?}"),
            },
        }
    });

    loop {
        tokio::select! {
            ws_msg = ws_rx.next() => {
                if let Some(Ok(msg)) = ws_msg {
                    // println!("{msg:?}");

                    if let Message::Text(text) = msg {
                        let buf = text.as_bytes();
                        if let Err(e) = serial_port.write_all(buf) {
                            println!("Writing to serial port failed: {e:?}");
                            break;
                        }
                    }
                } else {
                    break
                }
            }

            read_buf = read_rx.recv() => {
                if let Some(read_buf) = read_buf {
                    if let Err(e) = ws_tx.send(Message::Binary(read_buf)).await {
                        println!("Writing to WS failed: {e:?}");
                        break;
                    }
                } else {
                    println!("Read task channel closed");
                    break;
                }
            }
        }
    }

    println!("WS closing");
}
