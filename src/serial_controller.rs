use std::{
    io::{BufReader, ErrorKind, Write},
    sync::Arc,
};

use anyhow::anyhow;
use mio::{Events, Interest, Poll, Registry, Token};
use mio_serial::SerialStream;
use ringbuf::{traits::*, HeapRb};
use serialport::SerialPort;
use tokio::{
    sync::{broadcast, broadcast::error::RecvError, mpsc, Mutex},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{
    config::{SerialPortConfig, SerriConfig},
    util::ReadUpTo,
};

const DEFAULT_SERIAL_HISTORY_SIZE: usize = 1024 * 1024;
const DEFAULT_SERIAL_READ_BUFFER_SIZE: usize = 64;

pub struct SerialController {
    inner: Arc<SerialControllerRef>,
}

struct SerialControllerRef {
    token: Token,
    registry: Registry,

    serial_reader: Mutex<BufReader<SerialStream>>,
    read_buffer_size: usize,
    history: Mutex<HeapRb<u8>>,

    serial_read_tx: broadcast::Sender<Vec<u8>>,
    serial_write_tx: mpsc::Sender<Vec<u8>>,
    serial_write_rx: Mutex<mpsc::Receiver<Vec<u8>>>,
}

impl SerialController {
    pub fn new(
        port_config: &SerialPortConfig,
        serri_config: &SerriConfig,
        token: Token,
        registry: Registry,
    ) -> anyhow::Result<Self> {
        let mut serial_port = port_config.serial_device.open()?;

        registry
            .register(&mut serial_port, token, Interest::READABLE)
            .expect("failed to register serial stream for polling");

        let serial_reader = BufReader::new(serial_port);
        let read_buffer_size = port_config
            .read_buffer_size
            .or(serri_config.default_read_buffer_size)
            .unwrap_or(DEFAULT_SERIAL_READ_BUFFER_SIZE);
        let history_size = port_config
            .history_size
            .or(serri_config.default_history_size)
            .unwrap_or(DEFAULT_SERIAL_HISTORY_SIZE);

        let (serial_read_tx, _) = broadcast::channel::<Vec<u8>>(32);
        let (serial_write_tx, serial_write_rx) = mpsc::channel::<Vec<u8>>(32);

        Ok(Self {
            inner: Arc::new(SerialControllerRef {
                token,
                registry,
                serial_reader: Mutex::new(serial_reader),
                read_buffer_size,
                history: Mutex::new(HeapRb::new(history_size)),
                serial_read_tx,
                serial_write_tx,
                serial_write_rx: Mutex::new(serial_write_rx),
            }),
        })
    }

    pub fn subscribe_serial_read(&self) -> broadcast::Receiver<Vec<u8>> {
        self.inner.serial_read_tx.subscribe()
    }

    pub fn get_serial_write_tx(&self) -> mpsc::Sender<Vec<u8>> {
        self.inner.serial_write_tx.clone()
    }

    pub async fn get_history(&self) -> Vec<u8> {
        let history = self.inner.history.lock().await;
        let (left, right) = history.as_slices();

        let mut history_vec = Vec::with_capacity(history.capacity().get());
        history_vec.extend_from_slice(left);
        history_vec.extend_from_slice(right);

        history_vec
    }

    pub fn run_reader_task(
        &self,
        event_rx: broadcast::Receiver<Token>,
        cancellation_token: CancellationToken,
    ) -> JoinHandle<()> {
        let mut this = Self {
            inner: Arc::clone(&self.inner),
        };

        tokio::spawn(async move {
            this.reader_task(event_rx, cancellation_token).await;
        })
    }

    async fn reader_task(
        &mut self,
        mut event_rx: broadcast::Receiver<Token>,
        cancellation_token: CancellationToken,
    ) {
        let mut serial_write_rx = self.inner.serial_write_rx.lock().await;

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => break,

                rx_event = event_rx.recv() => if let Err(e) = self.process_read_event(rx_event).await {
                    println!("{e:?}");
                    break;
                },

                incoming_write = serial_write_rx.recv() => if let Err(e) = self.process_write(incoming_write).await {
                    println!("{e:?}");
                    break;
                }
            }
        }

        let mut serial_reader = self.inner.serial_reader.lock().await;
        self.inner
            .registry
            .deregister(serial_reader.get_mut())
            .expect("failed to deregister serial stream from polling");
    }

    async fn process_read_event(&self, rx_event: Result<Token, RecvError>) -> anyhow::Result<()> {
        match rx_event {
            Ok(recv_token) if recv_token == self.inner.token => self.read_serial().await?,
            Ok(_) => (),

            Err(RecvError::Lagged(lag)) => println!("Event channel lagged by {lag} messages"),

            Err(e) => {
                println!("Event channel closed");
                return Err(e.into());
            }
        }

        Ok(())
    }

    async fn read_serial(&self) -> anyhow::Result<()> {
        let mut serial_reader = self.inner.serial_reader.lock().await;
        let mut read_buf = vec![0u8; self.inner.read_buffer_size];

        match serial_reader.read_up_to(&mut read_buf) {
            Ok(amt) if amt > 0 => {
                println!("Read {amt} bytes from {:?}", serial_reader.get_ref().name());

                let _ = self.inner.serial_read_tx.send(read_buf[..amt].to_vec());
                let mut history = self.inner.history.lock().await;
                history.push_slice_overwrite(&read_buf[..amt]);
            }

            Err(e) if e.kind() != ErrorKind::WouldBlock => {
                println!("Failed to read from serial port: {e:?}");
                return Err(e.into());
            }

            _ => (),
        }

        Ok(())
    }

    async fn process_write(&self, data: Option<Vec<u8>>) -> anyhow::Result<()> {
        let Some(data) = data else {
            return Err(anyhow!("Serial write channel closed"));
        };

        let mut serial_reader = self.inner.serial_reader.lock().await;
        serial_reader.get_mut().write_all(&data)?;
        Ok(())
    }
}

pub fn create_serial_reader() -> (
    std::thread::JoinHandle<()>,
    Registry,
    broadcast::Sender<Token>,
) {
    let poll = Poll::new().expect("failed to create poller");
    let registry = poll
        .registry()
        .try_clone()
        .expect("failed to clone poll registry");

    let (event_tx, _) = broadcast::channel(32);
    let event_tx_clone = event_tx.clone();
    let handle = std::thread::spawn(|| serial_reader_thread(poll, event_tx_clone));

    (handle, registry, event_tx)
}

fn serial_reader_thread(mut poll: Poll, event_tx: broadcast::Sender<Token>) {
    let mut events = Events::with_capacity(1024);
    loop {
        poll.poll(&mut events, None)
            .expect("failed to poll serial devices");

        for event in events.iter() {
            let _ = event_tx.send(event.token());
        }
    }
}
