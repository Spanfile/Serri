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
    response::{IntoResponse, Redirect, Response},
    routing, Extension, Router,
};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use mio::{Events, Interest, Poll, Registry, Token};
use mio_serial::SerialStream;
use tokio::sync::{broadcast, broadcast::error::RecvError};
use tower_http::services::ServeDir;

use crate::{
    config::SerriConfig,
    util::ReadUpTo,
    web::template::{
        BaseTemplate, ConfigTemplate, DeviceTemplate, IndexTemplate, NotFoundTemplate,
    },
};

const SERIAL_READ_BUFFER_SIZE: usize = 256;

pub async fn run(serri_config: SerriConfig) -> anyhow::Result<()> {
    let serri_config = Arc::new(serri_config);

    let poll = Poll::new()?;
    let events = Events::with_capacity(32);
    let registry = poll
        .registry()
        .try_clone()
        .expect("failed to clone poll registry");

    let (event_tx, _) = broadcast::channel::<Token>(32);

    let app = Router::new()
        .route("/", routing::get(root))
        .route("/device", routing::get(|| async { Redirect::to("/") }))
        .route("/device/:device_index", routing::get(device))
        .route("/device/:device_index/ws", routing::get(device_ws))
        .route("/config", routing::get(config))
        .nest_service("/dist", ServeDir::new("dist"))
        .fallback(not_found)
        .layer(Extension(Arc::clone(&serri_config)))
        .layer(Extension(Arc::new(registry)))
        .layer(Extension(event_tx.clone()));

    let serial_reader_thread = std::thread::spawn(|| serial_reader_thread(poll, events, event_tx));

    let listener = tokio::net::TcpListener::bind(serri_config.listen).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

fn serial_reader_thread(mut poll: Poll, mut events: Events, event_tx: broadcast::Sender<Token>) {
    loop {
        println!("polling...");
        poll.poll(&mut events, None).expect("failed to poll");

        for event in events.iter() {
            // println!("Event: {event:?}");
            let _ = event_tx.send(event.token());
        }
    }
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
    Extension(registry): Extension<Arc<Registry>>,
    Extension(event_tx): Extension<broadcast::Sender<Token>>,
) -> Response {
    println!("New WS connection for device {device_index}");

    if let Some(port_config) = serri_config.serial_port.get(device_index) {
        let serial_stream = match port_config.serial_device.open() {
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

        let event_rx = event_tx.subscribe();
        let banner = serri_config.banner.clone();

        return ws.on_upgrade(move |mut socket| async move {
            if let Some(banner) = banner
                && let Err(e) = socket.send(Message::Text(banner)).await
            {
                println!("Failed to send banner to WS: {e}");
                return;
            }

            handle_device_ws(socket, device_index, serial_stream, registry, event_rx).await
        });
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::new(String::from("Device not found")))
        .unwrap()
}

async fn handle_device_ws(
    socket: WebSocket,
    device_index: usize,
    mut serial_stream: SerialStream,
    registry: Arc<Registry>,
    mut event_rx: broadcast::Receiver<Token>,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let token = Token(device_index);
    registry
        .register(&mut serial_stream, token, Interest::READABLE)
        .expect("failed to register serial stream for polling");

    let mut serial_reader = BufReader::new(
        serial_stream
            .try_clone_native()
            .expect("failed to clone serial port"),
    );

    // let cancel_token = CancellationToken::new();
    // let read_cancel_token = cancel_token.clone();
    // let (read_tx, mut read_rx) = tokio::sync::mpsc::channel(32);
    //
    // let reader_task = std::thread::spawn(move || {
    //     let mut buf_reader = BufReader::new(reader);
    //     loop {
    //         // TODO: polled read
    //
    //         let mut buf = [0u8; SERIAL_READ_BUFFER_SIZE];
    //         match buf_reader.read_up_to(&mut buf) {
    //             Ok(amt) if amt > 0 => {
    //                 println!("Read {amt} bytes from serial");
    //                 read_tx
    //                     .blocking_send(buf[..amt].to_vec())
    //                     .expect("failed to notify read buf");
    //             }
    //
    //             // my read_up_to should never return TimedOut but hey
    //             Err(e) if e.kind() != ErrorKind::TimedOut => {
    //                 println!("Failed to read from serial port: {e:?}");
    //                 break;
    //             }
    //
    //             _ => {
    //                 // println!("Timed out")
    //             }
    //         }
    //
    //         if read_cancel_token.is_cancelled() {
    //             println!("Reader closing");
    //             break;
    //         }
    //     }
    // });

    // TODO: websocket pings?

    loop {
        // println!("Main loop");
        tokio::select! {
            ws_msg = ws_rx.next() => if !process_ws_message(ws_msg, &mut serial_stream) {
                break;
            },

            rx_event = event_rx.recv() => if !process_mio_event(token, rx_event, &mut serial_reader, &mut ws_tx).await {
                break;
            },

            // read_buf = read_rx.recv() => if !process_serial_read(read_buf, &mut ws_tx).await {
            //     break;
            // }
        }
    }

    println!("WS closing");

    registry
        .deregister(&mut serial_stream)
        .expect("failed to deregister serial stream from polling");

    // cancel_token.cancel();
    // if let Err(e) = reader_task.join() {
    //     println!("Reader task failed: {e:?}");
    // }
}

fn process_ws_message(
    ws_msg: Option<Result<Message, axum::Error>>,
    serial_stream: &mut SerialStream,
) -> bool {
    if let Some(Ok(msg)) = ws_msg {
        println!("{msg:?}");

        if let Message::Text(text) = msg
            && let Err(e) = serial_stream.write_all(text.as_bytes())
        {
            println!("Writing to serial port failed: {e:?}");
            return false;
        }

        true
    } else {
        false
    }
}

async fn process_mio_event(
    token: Token,
    rx_event: Result<Token, RecvError>,
    serial_reader: &mut BufReader<SerialStream>,
    ws_tx: &mut SplitSink<WebSocket, Message>,
) -> bool {
    match rx_event {
        Ok(recv_token) if recv_token == token => {
            let mut buf = [0u8; SERIAL_READ_BUFFER_SIZE];

            match serial_reader.read_up_to(&mut buf) {
                Ok(amt) if amt > 0 => {
                    println!("Read {amt} bytes from serial");
                    if let Err(e) = ws_tx.send(Message::Binary(buf[..amt].to_vec())).await {
                        println!("Writing to websocket failed: {e:?}");
                        return false;
                    }

                    true
                }

                // my read_up_to should never return WouldBlock but hey
                Err(e) if e.kind() != ErrorKind::WouldBlock => {
                    println!("Failed to read from serial port: {e:?}");
                    false
                }

                _ => true,
            }
        }

        Ok(_) => true,

        Err(RecvError::Closed) => {
            println!("Event reader channel closed");
            false
        }

        Err(RecvError::Lagged(lag)) => {
            println!("Event reader channel lagged by {lag} messages");
            true
        }
    }
}

// async fn process_serial_read(
//     read_buf: Option<Vec<u8>>,
//     ws_tx: &mut SplitSink<WebSocket, Message>,
// ) -> bool {
//     if let Some(read_buf) = read_buf {
//         if let Err(e) = ws_tx.send(Message::Binary(read_buf)).await {
//             println!("Writing to WS failed: {e:?}");
//             return false;
//         }
//
//         true
//     } else {
//         println!("Reader task channel closed");
//         false
//     }
// }
