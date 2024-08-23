use std::{
    io::{BufReader, ErrorKind, Write},
    sync::Arc,
};

use askama_axum::{IntoResponse, Response};
use axum::{
    extract::{
        ws::{close_code, CloseFrame, Message, WebSocket},
        Path, WebSocketUpgrade,
    },
    response::Redirect,
    routing, Extension, Router,
};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use mio::{Events, Interest, Poll, Registry, Token};
use mio_serial::SerialStream;
use serde::Serialize;
use serialport::SerialPort;
use tinytemplate::TinyTemplate;
use tokio::sync::{broadcast, broadcast::error::RecvError};

use crate::{
    config::{SerialPortConfig, SerriConfig},
    util::ReadUpTo,
    web::template::{BaseTemplate, DeviceTemplate, IndexTemplate},
};

const DEFAULT_SERIAL_READ_BUFFER_SIZE: usize = 64;

#[derive(Serialize)]
struct BannerContext<'a> {
    device: &'a str,
}

pub fn router() -> anyhow::Result<Router> {
    let poll = Poll::new()?;
    let events = Events::with_capacity(32);
    let registry = poll
        .registry()
        .try_clone()
        .expect("failed to clone poll registry");

    let (event_tx, _) = broadcast::channel::<Token>(32);
    let event_tx_clone = event_tx.clone();
    let _serial_reader_thread =
        std::thread::spawn(|| serial_reader_thread(poll, events, event_tx_clone));

    Ok(Router::new()
        .route("/", routing::get(|| async { Redirect::to("/") }))
        .route("/:device_index", routing::get(device))
        .route("/:device_index/ws", routing::get(device_ws))
        .layer(Extension(Arc::new(registry)))
        .layer(Extension(event_tx.clone())))
}

fn serial_reader_thread(mut poll: Poll, mut events: Events, event_tx: broadcast::Sender<Token>) {
    loop {
        // println!("polling...");
        poll.poll(&mut events, None)
            .expect("failed to poll serial devices");

        for event in events.iter() {
            // println!("Event: {event:?}");
            let _ = event_tx.send(event.token());
        }
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

    ws.on_upgrade(move |socket| async move {
        let Some(port_config) = serri_config.serial_port.get(device_index) else {
            return;
        };

        let token = Token(device_index);
        let event_rx = event_tx.subscribe();

        handle_device_ws(
            socket,
            &serri_config,
            port_config,
            token,
            event_rx,
            registry,
        )
        .await
    })
}

async fn handle_device_ws(
    mut socket: WebSocket,
    serri_config: &SerriConfig,
    port_config: &SerialPortConfig,
    token: Token,
    mut event_rx: broadcast::Receiver<Token>,
    registry: Arc<Registry>,
) {
    let mut serial_port = match port_config.serial_device.open() {
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

    if let Some(banner) = serri_config.banner.as_deref()
        && send_banner(&mut socket, banner, port_config).await.is_err()
    {
        return;
    };

    registry
        .register(&mut serial_port, token, Interest::READABLE)
        .expect("failed to register serial stream for polling");

    let mut serial_reader = BufReader::new(serial_port);
    let read_buffer_size = port_config
        .read_buffer_size
        .or(serri_config.default_read_buffer_size)
        .unwrap_or(DEFAULT_SERIAL_READ_BUFFER_SIZE);

    // TODO: websocket pings?
    let (mut ws_tx, mut ws_rx) = socket.split();

    loop {
        // println!("Main loop");
        tokio::select! {
            ws_msg = ws_rx.next() => if !process_ws_message(ws_msg, serial_reader.get_mut()) {
                break;
            },

            rx_event = event_rx.recv() => if !process_mio_event(token, rx_event, &mut serial_reader, &mut ws_tx, read_buffer_size).await {
                break;
            }
        }
    }

    println!("Closing WS for {:?}", serial_reader.get_ref().name());
    let _ = ws_tx.close().await;

    registry
        .deregister(&mut serial_reader.into_inner())
        .expect("failed to deregister serial stream from polling");
}

async fn send_banner(
    socket: &mut WebSocket,
    banner: &str,
    port_config: &SerialPortConfig,
) -> anyhow::Result<()> {
    let banner_context = create_banner_context(port_config);
    match render_banner_template(banner, banner_context) {
        Ok(rendered) => socket.send(Message::Text(rendered)).await?,
        Err(e) => {
            println!("Failed to render banner template: {e:?}");

            socket
                .send(Message::Text(
                    "Failed to render banner template".to_string(),
                ))
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

fn process_ws_message(
    ws_msg: Option<Result<Message, axum::Error>>,
    serial_port: &mut SerialStream,
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

async fn process_mio_event(
    token: Token,
    rx_event: Result<Token, RecvError>,
    serial_reader: &mut BufReader<SerialStream>,
    ws_tx: &mut SplitSink<WebSocket, Message>,
    read_buffer_size: usize,
) -> bool {
    match rx_event {
        Ok(recv_token) if recv_token == token => {
            let mut read_buf = vec![0u8; read_buffer_size];
            match serial_reader.read_up_to(&mut read_buf) {
                Ok(amt) if amt > 0 => {
                    println!("Read {amt} bytes from {:?}", serial_reader.get_ref().name());
                    if let Err(e) = ws_tx.send(Message::Binary(read_buf[..amt].to_vec())).await {
                        println!("Writing to websocket failed: {e:?}");
                        return false;
                    }

                    true
                }

                Err(e) if e.kind() != ErrorKind::WouldBlock => {
                    println!("Failed to read from serial port: {e:?}");
                    false
                }

                _ => true,
            }
        }

        Ok(_) => true,

        Err(RecvError::Closed) => {
            println!("Event channel closed");
            false
        }

        Err(RecvError::Lagged(lag)) => {
            println!("Event channel lagged by {lag} messages");
            true
        }
    }
}
