//! Rust port of the [hass-omron](https://github.com/eigger/hass-omron)
//! Home Assistant integration: BLE pair, connect, and read measurement data
//! from Omron blood-pressure monitors.

pub mod ble;
#[cfg(target_os = "linux")]
pub mod bluez_agent;
pub mod bps;
pub mod consts;
pub mod device_catalog;
pub mod device_config;
pub mod devices;
pub mod driver;
pub mod error;
pub mod pairing;
pub mod racp;
pub mod record_parsers;
pub mod setup;
pub mod time_sync;
pub mod transport;

pub use device_config::DeviceConfig;
pub use devices::{get_device_config, supported_models};
pub use driver::OmronDeviceDriver;
pub use error::{OmronError, Result};
pub use record_parsers::Record;
pub use transport::GattTransport;
