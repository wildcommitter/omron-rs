//! BLE UUIDs and protocol constants ported from `omron_ble/const.py`.

use uuid::{uuid, Uuid};

pub const DEFAULT_DEVICE_MODEL: &str = "HEM-7142T2";

pub const CTS_CHARACTERISTIC_UUID: Uuid = uuid!("00002a2b-0000-1000-8000-00805f9b34fb");
pub const BATTERY_LEVEL_UUID: Uuid = uuid!("00002a19-0000-1000-8000-00805f9b34fb");
pub const FIRMWARE_REVISION_UUID: Uuid = uuid!("00002a26-0000-1000-8000-00805f9b34fb");
pub const HARDWARE_REVISION_UUID: Uuid = uuid!("00002a27-0000-1000-8000-00805f9b34fb");
pub const MANUFACTURER_NAME_UUID: Uuid = uuid!("00002a29-0000-1000-8000-00805f9b34fb");
pub const MODEL_NUMBER_UUID: Uuid = uuid!("00002a24-0000-1000-8000-00805f9b34fb");
pub const LOCAL_TIME_INFO_UUID: Uuid = uuid!("00002a0f-0000-1000-8000-00805f9b34fb");

pub const BP_MEASUREMENT_CHAR_UUID: Uuid = uuid!("00002a35-0000-1000-8000-00805f9b34fb");
pub const BP_RACP_CHAR_UUID: Uuid = uuid!("00002a52-0000-1000-8000-00805f9b34fb");

/// Bluetooth SIG company identifier for Omron Healthcare.
pub const OMRON_MANUFACTURER_ID: u16 = 526;

pub const CLASSIC_STACK_PARENT_SERVICE_UUID: Uuid =
    uuid!("ecbe3980-c9a2-11e1-b1bd-0002a5d5c51b");
pub const MODERN_STACK_PARENT_SERVICE_UUID: Uuid =
    uuid!("0000fe4a-0000-1000-8000-00805f9b34fb");
pub const STANDARD_BLOOD_PRESSURE_SERVICE_UUID: Uuid =
    uuid!("00001810-0000-1000-8000-00805f9b34fb");

pub const CLASSIC_STACK_RX_CHARACTERISTIC_UUIDS: [Uuid; 4] = [
    uuid!("49123040-aee8-11e1-a74d-0002a5d5c51b"),
    uuid!("4d0bf320-aee8-11e1-a0d9-0002a5d5c51b"),
    uuid!("5128ce60-aee8-11e1-b84b-0002a5d5c51b"),
    uuid!("560f1420-aee8-11e1-8184-0002a5d5c51b"),
];

pub const CLASSIC_STACK_TX_CHARACTERISTIC_UUIDS: [Uuid; 4] = [
    uuid!("db5b55e0-aee7-11e1-965e-0002a5d5c51b"),
    uuid!("e0b8a060-aee7-11e1-92f4-0002a5d5c51b"),
    uuid!("0ae12b00-aee8-11e1-a192-0002a5d5c51b"),
    uuid!("10e1ba60-aee8-11e1-89e5-0002a5d5c51b"),
];

pub const CLASSIC_STACK_UNLOCK_CHARACTERISTIC_UUID: Uuid =
    uuid!("b305b680-aee7-11e1-a730-0002a5d5c51b");

pub const MODERN_STACK_I2_CHARACTERISTIC_UUID: Uuid =
    uuid!("8858eb40-aee8-11e1-bb67-0002a5d5c51b");

pub const DISCOVERABLE_PARENT_SERVICE_UUIDS: [Uuid; 2] = [
    CLASSIC_STACK_PARENT_SERVICE_UUID,
    MODERN_STACK_PARENT_SERVICE_UUID,
];

/// The default application-level pairing key written into Omron classic-stack
/// devices.  Sixteen bytes; matches `omron_driver.PAIRING_KEY`.
pub const PAIRING_KEY: [u8; 16] = [
    0xde, 0xad, 0xbe, 0xaf, 0x12, 0x34, 0x12, 0x34, 0xde, 0xad, 0xbe, 0xaf, 0x12, 0x34, 0x12, 0x34,
];
