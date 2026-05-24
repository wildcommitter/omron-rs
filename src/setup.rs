//! Top-level pair + time-sync orchestration, mirroring
//! `omron_ble.setup.async_pair_and_sync_device`.

use std::time::Duration;

use btleplug::api::Peripheral as _;
use btleplug::platform::Peripheral;
use tokio::time::sleep;
use tracing::debug;

use crate::consts::MODEL_NUMBER_UUID;
use crate::device_config::DeviceConfig;
use crate::devices::get_device_config;
use crate::error::{OmronError, Result};
use crate::time_sync::sync_device_time;
use crate::transport::GattTransport;

/// Bond-settle pause after `connect()` returns. Matches the
/// `_POST_CONNECT_BOND_SETTLE_SEC` constant in `omron_driver.py`.
const POST_CONNECT_BOND_SETTLE: Duration = Duration::from_millis(1500);

/// Connect to the peripheral, wait for the BLE link to settle, and refresh
/// the GATT cache. Use this everywhere instead of calling `peripheral.connect()`
/// directly so the bond-settle behaviour stays consistent.
pub async fn establish_connection(peripheral: &Peripheral) -> Result<()> {
    if !peripheral.is_connected().await? {
        peripheral.connect().await?;
    }
    debug!(
        "BLE link established; settling {:?} before first GATT op",
        POST_CONNECT_BOND_SETTLE
    );
    sleep(POST_CONNECT_BOND_SETTLE).await;
    peripheral.discover_services().await?;
    Ok(())
}

/// Connect and read the Model Number characteristic (0x2A24).
pub async fn fetch_device_model_number(peripheral: &Peripheral) -> Result<Option<String>> {
    establish_connection(peripheral).await?;
    for c in peripheral.characteristics() {
        if c.uuid == MODEL_NUMBER_UUID {
            let bytes = peripheral.read(&c).await?;
            let s = String::from_utf8_lossy(&bytes)
                .trim_matches(|c: char| c == ' ' || c == '\0')
                .to_string();
            debug!("fetched Model Number: {}", s);
            return Ok(Some(s));
        }
    }
    Ok(None)
}

/// Pair (program a new pairing key) and run initial time sync.  The device
/// must be in pairing mode (`-P-` blinking) before this is called on
/// classic-stack profiles.
pub async fn pair_and_sync_device(
    peripheral: Peripheral,
    model: &str,
    config: Option<DeviceConfig>,
) -> Result<()> {
    let config = config.unwrap_or_else(|| get_device_config(model));
    establish_connection(&peripheral).await?;

    let mut transport = GattTransport::new(peripheral.clone(), config.clone()).await?;

    // Wait briefly for the parent service to appear; some devices expose it
    // only after the post-bond service refresh.
    let mut service_found = false;
    let parent = config.parent_service_uuid;
    for attempt in 0..5 {
        let services = peripheral.services();
        if services.iter().any(|s| s.uuid == parent) {
            service_found = true;
            break;
        }
        if attempt < 4 {
            let _ = peripheral.discover_services().await;
            sleep(Duration::from_millis(350)).await;
        }
    }
    if !service_found {
        debug!(
            "parent service {} not found on {}; continuing anyway",
            parent, model
        );
    }

    transport.pair(None).await?;
    sync_device_time(&mut transport).await?;
    debug!("successfully paired and synced with {}", model);
    Ok(())
}

/// Convenience: connect to a peripheral and produce a ready-to-use transport.
/// Intended for the `read` command path where pairing has already happened.
pub async fn connect_with_transport(
    peripheral: Peripheral,
    model: &str,
) -> Result<GattTransport> {
    let config = get_device_config(model);
    establish_connection(&peripheral).await?;
    GattTransport::new(peripheral, config).await
}

#[allow(dead_code)]
pub(crate) fn _silence_unused() {
    let _ = OmronError::NotFound;
}
