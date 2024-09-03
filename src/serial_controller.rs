use std::{
    io::{BufReader, ErrorKind, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use anyhow::anyhow;
use mio::{Events, Interest, Poll, Registry, Token};
use mio_serial::SerialStream;
use ringbuf::{traits::*, HeapRb};
use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};
use tokio::{
    sync::{broadcast, broadcast::error::RecvError, mpsc, Mutex},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{
    config::{SerialPortConfig, SerriConfig},
    util::ReadUpTo,
};

const SERIAL_READ_BUFFER_SIZE: usize = 256;
const DEFAULT_SERIAL_HISTORY_SIZE: usize = 100 * 1024;
const DEFAULT_PRESERVE_HISTORY: bool = true;

pub struct SerialController {
    inner: Arc<SerialControllerRef>,
}

struct SerialControllerRef {
    token: Token,
    registry: Registry,

    serial_reader: Mutex<Option<BufReader<SerialStream>>>,
    history: Mutex<HeapRb<u8>>,
    preserve_history: AtomicBool,

    serial_read_tx: broadcast::Sender<SerialReadData>,
    serial_write_tx: mpsc::Sender<Vec<u8>>,
    serial_write_rx: Mutex<mpsc::Receiver<Vec<u8>>>,
}

#[derive(Debug, Copy, Clone)]
pub struct SerialReadEvent {
    pub token: Token,
    pub is_error: bool,
}

#[derive(Debug, Clone)]
pub enum SerialReadData {
    Data(Vec<u8>),
    Error,
}

impl SerialController {
    pub fn new(
        port_config: &SerialPortConfig,
        serri_config: &SerriConfig,
        token: Token,
        registry: Registry,
    ) -> Self {
        let history_size = port_config
            .history_size
            .or(serri_config.default_history_size)
            .unwrap_or(DEFAULT_SERIAL_HISTORY_SIZE);

        // TODO: don't allocate history ringbuf if preserve is disabled?
        let preserve_history = port_config
            .preserve_history
            .or(serri_config.preserve_history)
            .unwrap_or(DEFAULT_PRESERVE_HISTORY);

        let (serial_read_tx, _) = broadcast::channel(32);
        let (serial_write_tx, serial_write_rx) = mpsc::channel(32);

        let serial_reader = match port_config.serial_device.open() {
            Ok(mut serial_port) => {
                registry
                    .register(&mut serial_port, token, Interest::READABLE)
                    .expect("failed to register serial stream for polling");

                Some(BufReader::new(serial_port))
            }

            Err(e) => {
                println!("Failed to open serial device: {e:?}");
                None
            }
        };

        Self {
            inner: Arc::new(SerialControllerRef {
                token,
                registry,
                serial_reader: Mutex::new(serial_reader),
                history: Mutex::new(HeapRb::new(history_size)),
                preserve_history: AtomicBool::new(preserve_history),
                serial_read_tx,
                serial_write_tx,
                serial_write_rx: Mutex::new(serial_write_rx),
            }),
        }
    }

    pub async fn reopen_serial_device(
        &self,
        port_config: &SerialPortConfig,
        token: Token,
    ) -> anyhow::Result<()> {
        let mut serial_port = port_config.serial_device.open()?;
        self.inner
            .registry
            .register(&mut serial_port, token, Interest::READABLE)
            .expect("failed to register serial stream for polling");

        let mut serial_reader = self.inner.serial_reader.lock().await;
        *serial_reader = Some(BufReader::new(serial_port));

        Ok(())
    }

    pub async fn is_serial_device_open(&self) -> bool {
        let serial_device = self.inner.serial_reader.lock().await;
        serial_device.is_some()
    }

    pub async fn get_serial_device_parameters(
        &self,
    ) -> Option<(String, u32, DataBits, Parity, StopBits, FlowControl)> {
        let serial_device = self.inner.serial_reader.lock().await;
        let serial_device = serial_device.as_ref()?;
        let serial_device = serial_device.get_ref();

        let device = serial_device.name().unwrap_or("unknown device".to_string());
        let baud_rate = serial_device.baud_rate().ok()?;
        let data_bits = serial_device.data_bits().ok()?;
        let parity = serial_device.parity().ok()?;
        let stop_bits = serial_device.stop_bits().ok()?;
        let flow_control = serial_device.flow_control().ok()?;

        Some((
            device,
            baud_rate,
            data_bits,
            parity,
            stop_bits,
            flow_control,
        ))
    }

    pub fn get_serial_read_rx(&self) -> broadcast::Receiver<SerialReadData> {
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

    pub async fn clear_history(&self) {
        let mut history = self.inner.history.lock().await;
        let amt = history.clear();
        println!("Cleared {amt} bytes from history");
    }

    pub fn get_preserve_history(&self) -> bool {
        self.inner.preserve_history.load(Ordering::Relaxed)
    }

    pub fn set_preserve_history(&self, preserve_history: bool) {
        self.inner
            .preserve_history
            .store(preserve_history, Ordering::Relaxed)
    }

    pub async fn run_reader_task(
        &self,
        event_rx: broadcast::Receiver<SerialReadEvent>,
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
        mut event_rx: broadcast::Receiver<SerialReadEvent>,
        cancellation_token: CancellationToken,
    ) {
        let mut serial_write_rx = self.inner.serial_write_rx.lock().await;

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => break,

                // if read/write fails, close the serial device *but* leave this task running, since
                // the device can be reopened later and this task will continue processing it
                rx_event = event_rx.recv() => if let Err(e) = self.process_read_event(rx_event).await {
                    println!("Read event error: {e:?}");
                    self.close_serial_device().await;
                },

                incoming_write = serial_write_rx.recv() => if let Err(e) = self.process_write(incoming_write).await {
                    println!("Incoming write error: {e:?}");
                    self.close_serial_device().await;
                }
            }
        }

        println!("Serial reader task closing");
        self.close_serial_device().await;
    }

    async fn process_read_event(
        &self,
        rx_event: Result<SerialReadEvent, RecvError>,
    ) -> anyhow::Result<()> {
        match rx_event {
            Ok(SerialReadEvent { token, is_error }) if token == self.inner.token => {
                if is_error {
                    let _ = self.inner.serial_read_tx.send(SerialReadData::Error);
                    return Err(anyhow!("Serial device returned error"));
                }

                self.read_serial().await?
            }

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
        let Some(serial_reader) = serial_reader.as_mut() else {
            return Err(anyhow!("Serial device not open"));
        };

        // TODO: its technically possible that the serial device produces data faster than we can
        // read it, so this message will keep growing indefinitely. probably a good idea to set a
        // maximum size?
        let mut message = Vec::new();
        let mut read_buf = [0u8; SERIAL_READ_BUFFER_SIZE];

        loop {
            match serial_reader.read_up_to(&mut read_buf) {
                Ok(amt) if amt > 0 => {
                    message.extend_from_slice(&read_buf[..amt]);

                    if serial_reader.buffer().is_empty() {
                        break;
                    }
                }

                Err(e) if e.kind() != ErrorKind::WouldBlock => {
                    println!("Failed to read from serial port: {e:?}");
                    return Err(e.into());
                }

                // having read 0 bytes means the device is exhausted of data
                _ => return Ok(()),
            }
        }

        println!(
            "Read {} bytes from {:?}",
            message.len(),
            serial_reader.get_ref().name()
        );

        if self.get_preserve_history() {
            let mut history = self.inner.history.lock().await;
            history.push_slice_overwrite(&message);
        }

        let _ = self
            .inner
            .serial_read_tx
            .send(SerialReadData::Data(message));

        Ok(())
    }

    async fn process_write(&self, data: Option<Vec<u8>>) -> anyhow::Result<()> {
        let data = data.ok_or(anyhow!("Serial write channel closed"))?;

        let mut serial_reader = self.inner.serial_reader.lock().await;
        let serial_reader = serial_reader
            .as_mut()
            .ok_or(anyhow!("Serial device not open"))?;

        serial_reader.get_mut().write_all(&data)?;
        Ok(())
    }

    async fn close_serial_device(&self) {
        let mut serial_reader = self.inner.serial_reader.lock().await;
        let Some(mut serial_reader) = serial_reader.take() else {
            println!("Serial device not open");
            return;
        };

        self.inner
            .registry
            .deregister(serial_reader.get_mut())
            .expect("failed to deregister serial stream from polling");
    }
}

pub fn create_serial_reader() -> (
    std::thread::JoinHandle<()>,
    Registry,
    broadcast::Sender<SerialReadEvent>,
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

fn serial_reader_thread(mut poll: Poll, event_tx: broadcast::Sender<SerialReadEvent>) {
    let mut events = Events::with_capacity(1024);
    loop {
        poll.poll(&mut events, None)
            .expect("failed to poll serial devices");

        for event in events.iter() {
            // println!("{event:?}");
            let _ = event_tx.send(SerialReadEvent {
                token: event.token(),
                is_error: event.is_error(),
            });
        }
    }
}
