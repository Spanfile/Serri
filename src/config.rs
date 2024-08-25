use std::{fmt, fmt::Formatter, net::SocketAddr, num::ParseIntError, str::FromStr, time::Duration};

use mio_serial::{SerialPortBuilderExt, SerialStream};
use serde::{
    de::{Error, Unexpected, Visitor},
    Deserialize, Deserializer,
};
use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};
use thiserror::Error;

use crate::util::MaybeSplitOnce;

#[derive(Debug, Deserialize)]
pub struct SerriConfig {
    pub listen: SocketAddr,
    pub banner: Option<String>,
    pub default_history_size: Option<usize>,
    pub default_read_buffer_size: Option<usize>,
    #[serde(default)]
    pub serial_port: Vec<SerialPortConfig>,
}

#[derive(Debug, Deserialize)]
pub struct SerialPortConfig {
    pub title: Option<String>,
    pub description: Option<String>,
    pub history_size: Option<usize>,
    pub read_buffer_size: Option<usize>,
    #[serde(flatten)]
    pub serial_device: SerialDevice,
}

#[derive(Debug)]
pub struct SerialDevice {
    pub device: String,
    pub baud_rate: u32,
    pub serial_params: SerialParams,
}

#[derive(Debug, Deserialize)]
pub struct SerialParams {
    #[serde(
        deserialize_with = "deserialize_data_bits",
        default = "default_data_bits"
    )]
    pub data_bits: DataBits,

    #[serde(deserialize_with = "deserialize_parity", default = "default_parity")]
    pub parity: Parity,

    #[serde(
        deserialize_with = "deserialize_stop_bits",
        default = "default_stop_bits"
    )]
    pub stop_bits: StopBits,

    #[serde(
        deserialize_with = "deserialize_flow_control",
        default = "default_flow_control"
    )]
    pub flow_control: FlowControl,
}

#[derive(Debug, Error)]
pub enum SerialDeviceParseError {
    #[error("malformed device string: {0}")]
    MalformedConnectionString(String),
    #[error("invalid baud rate: {0}")]
    InvalidBaudRate(String),
    #[error("invalid serial parameters: {0}")]
    InvalidParameters(String),

    #[error("invalid integer value: {0}")]
    InvalidInteger(#[from] ParseIntError),
}

impl SerialDevice {
    pub fn open(&self) -> anyhow::Result<SerialStream> {
        let mut port = serialport::new(&self.device, self.baud_rate)
            .data_bits(self.serial_params.data_bits)
            .parity(self.serial_params.parity)
            .stop_bits(self.serial_params.stop_bits)
            .flow_control(self.serial_params.flow_control)
            .open_native_async()?;

        port.set_timeout(Duration::from_millis(10))
            .expect("failed to set serial port timeout");

        port.set_exclusive(true)
            .expect("failed to set serial port exclusive");

        Ok(port)
    }
}

impl Default for SerialParams {
    fn default() -> Self {
        Self {
            data_bits: DataBits::Eight,
            parity: Parity::None,
            stop_bits: StopBits::One,
            flow_control: FlowControl::None,
        }
    }
}

impl FromStr for SerialDevice {
    type Err = SerialDeviceParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((device, rest)) = s.split_once(':') else {
            return Err(SerialDeviceParseError::MalformedConnectionString(
                s.to_string(),
            ));
        };

        let (baud_str, params_str) = rest.maybe_split_once(':');
        let baud_rate: u32 = baud_str
            .parse()
            .map_err(|_| SerialDeviceParseError::InvalidBaudRate(baud_str.to_string()))?;

        let serial_params = if let Some(params_str) = params_str {
            let Some((data_char, parity_and_stop)) = params_str.split_at_checked(1) else {
                return Err(SerialDeviceParseError::InvalidParameters(
                    params_str.to_string(),
                ));
            };

            let Some((parity_char, stop_char)) = parity_and_stop.split_at_checked(1) else {
                return Err(SerialDeviceParseError::InvalidParameters(
                    params_str.to_string(),
                ));
            };

            // jesus fucking god dammit people, stop using () as an error type in your public APIs
            // (looking at you, serialport-rs)
            let data_bits = data_char
                .parse::<u8>()?
                .try_into()
                .map_err(|_| SerialDeviceParseError::MalformedConnectionString(s.to_string()))?;

            let parity = parse_parity_str(parity_char)
                .map_err(|_| SerialDeviceParseError::MalformedConnectionString(s.to_string()))?;

            let stop_bits = stop_char
                .parse::<u8>()?
                .try_into()
                .map_err(|_| SerialDeviceParseError::MalformedConnectionString(s.to_string()))?;

            SerialParams {
                data_bits,
                parity,
                stop_bits,
                flow_control: default_flow_control(),
            }
        } else {
            SerialParams::default()
        };

        Ok(SerialDevice {
            device: device.to_string(),
            baud_rate,
            serial_params,
        })
    }
}

impl<'de> Deserialize<'de> for SerialDevice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum SerialDeviceEnum {
            #[serde(with = "ThisSerialDevice")]
            DeviceObject(SerialDevice),
            DeviceString {
                device: String,
            },
        }

        #[derive(Deserialize)]
        #[serde(remote = "SerialDevice")]
        struct ThisSerialDevice {
            device: String,
            baud_rate: u32,
            #[serde(flatten, default)]
            serial_params: SerialParams,
        }

        match SerialDeviceEnum::deserialize(deserializer)? {
            SerialDeviceEnum::DeviceObject(device) => Ok(device),
            SerialDeviceEnum::DeviceString { device: device_str } => {
                let device = SerialDevice::from_str(&device_str).map_err(Error::custom)?;
                Ok(device)
            }
        }
    }
}

// yeah yeah i know i yelled about using () as an error like a 100 lines up but it doesn't matter
// for internal private functions
fn parse_parity_str(parity: &str) -> Result<Parity, ()> {
    match parity {
        "None" | "none" | "N" | "n" => Ok(Parity::None),
        "Even" | "even" | "E" | "e" => Ok(Parity::Even),
        "Odd" | "odd" | "O" | "o" => Ok(Parity::Odd),
        _ => Err(()),
    }
}

fn default_data_bits() -> DataBits {
    SerialParams::default().data_bits
}
fn default_parity() -> Parity {
    SerialParams::default().parity
}
fn default_stop_bits() -> StopBits {
    SerialParams::default().stop_bits
}
fn default_flow_control() -> FlowControl {
    SerialParams::default().flow_control
}

fn deserialize_data_bits<'de, D>(deserializer: D) -> Result<DataBits, D::Error>
where
    D: Deserializer<'de>,
{
    struct DataBitsVisitor;

    impl<'de> Visitor<'de> for DataBitsVisitor {
        type Value = DataBits;

        fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
            formatter.write_str("data bits integer (5, 6, 7 or 8)")
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: Error,
        {
            let vu8 =
                u8::try_from(v).map_err(|_| E::invalid_value(Unexpected::Signed(v), &self))?;
            DataBits::try_from(vu8)
                .map_err(|_| E::invalid_value(Unexpected::Unsigned(v as u64), &self))
        }
    }

    deserializer.deserialize_i64(DataBitsVisitor)
}

fn deserialize_parity<'de, D>(deserializer: D) -> Result<Parity, D::Error>
where
    D: Deserializer<'de>,
{
    struct ParityVisitor;

    impl<'de> Visitor<'de> for ParityVisitor {
        type Value = Parity;

        fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
            formatter.write_str("parity string ([Nn]one, [Ee]ven, [Oo]dd)")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: Error,
        {
            parse_parity_str(v).map_err(|_| Error::invalid_value(Unexpected::Str(v), &self))
        }
    }

    deserializer.deserialize_str(ParityVisitor)
}

fn deserialize_stop_bits<'de, D>(deserializer: D) -> Result<StopBits, D::Error>
where
    D: Deserializer<'de>,
{
    struct StopBitsVisitor;

    impl<'de> Visitor<'de> for StopBitsVisitor {
        type Value = StopBits;

        fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
            formatter.write_str("stop bits integer (1 or 2)")
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: Error,
        {
            let vu8 =
                u8::try_from(v).map_err(|_| E::invalid_value(Unexpected::Signed(v), &self))?;
            StopBits::try_from(vu8)
                .map_err(|_| E::invalid_value(Unexpected::Unsigned(v as u64), &self))
        }
    }

    deserializer.deserialize_i64(StopBitsVisitor)
}

fn deserialize_flow_control<'de, D>(deserializer: D) -> Result<FlowControl, D::Error>
where
    D: Deserializer<'de>,
{
    struct FlowControlVisitor;

    impl<'de> Visitor<'de> for FlowControlVisitor {
        type Value = FlowControl;

        fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
            formatter.write_str("flow control string")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: Error,
        {
            FlowControl::from_str(v).map_err(|_| E::invalid_value(Unexpected::Str(v), &self))
        }
    }

    deserializer.deserialize_str(FlowControlVisitor)
}
