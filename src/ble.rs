//! BLE adapter/scanner helpers — wraps btleplug so the CLI doesn't have to
//! deal with adapter selection and peripheral filtering directly.

use std::time::Duration;

use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral};
use tokio::time::sleep;
use tracing::debug;

use crate::consts::{DISCOVERABLE_PARENT_SERVICE_UUIDS, OMRON_MANUFACTURER_ID};
use crate::error::{OmronError, Result};

/// Scan-time summary of an Omron peripheral discovered via advertisement.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoveredDevice {
    pub address: String,
    pub local_name: Option<String>,
    pub rssi: Option<i16>,
    pub inferred_model: Option<String>,
    pub is_omron: bool,
    pub in_pairing_mode: bool,
}

pub async fn default_adapter() -> Result<Adapter> {
    let manager = Manager::new().await?;
    let mut adapters = manager.adapters().await?;
    adapters.pop().ok_or_else(|| OmronError::Other("no BLE adapter found".into()))
}

/// Scan for `duration` and return everything that looks like an Omron device.
pub async fn scan_for_omron(adapter: &Adapter, duration: Duration) -> Result<Vec<(Peripheral, DiscoveredDevice)>> {
    adapter.start_scan(ScanFilter::default()).await?;
    sleep(duration).await;
    adapter.stop_scan().await?;

    let mut out = Vec::new();
    for p in adapter.peripherals().await? {
        let props = match p.properties().await? {
            Some(p) => p,
            None => continue,
        };
        let local_name = props.local_name.clone();
        let manuf = props.manufacturer_data.keys().copied().collect::<Vec<_>>();
        let services = props.services.clone();

        let mut is_omron = false;
        if manuf.contains(&OMRON_MANUFACTURER_ID) {
            is_omron = true;
        }
        if !is_omron {
            for sid in &services {
                if DISCOVERABLE_PARENT_SERVICE_UUIDS.contains(sid) {
                    is_omron = true;
                    break;
                }
            }
        }
        if !is_omron {
            if let Some(name) = &local_name {
                let lower = name.to_ascii_lowercase();
                if lower.contains("omron") || name.to_ascii_uppercase().starts_with("HEM-") {
                    is_omron = true;
                }
            }
        }
        if !is_omron {
            continue;
        }

        let inferred_model = local_name
            .as_deref()
            .and_then(crate::devices::infer_model_id_from_local_name);

        let in_pairing_mode = props
            .manufacturer_data
            .get(&OMRON_MANUFACTURER_ID)
            .map(|payload| in_pairing_mode_from_msd(payload))
            .unwrap_or(false);

        let info = DiscoveredDevice {
            address: p.address().to_string(),
            local_name,
            rssi: props.rssi,
            inferred_model,
            is_omron: true,
            in_pairing_mode,
        };
        debug!(?info, "discovered Omron device");
        out.push((p, info));
    }
    Ok(out)
}

/// Best-effort decode of the "pairing mode" advertisement status bit from the
/// manufacturer-specific data block. The bit lives in byte 0x07 of the
/// Omron-specific payload (cf. `parser.py:_start_update`).
fn in_pairing_mode_from_msd(payload: &[u8]) -> bool {
    if payload.len() <= 7 {
        return false;
    }
    let status = payload[7];
    (status & 0b0000_0100) != 0
}

/// Find a peripheral by address (case-insensitive). Triggers a fresh scan.
pub async fn find_peripheral_by_address(adapter: &Adapter, address: &str) -> Result<Peripheral> {
    adapter.start_scan(ScanFilter::default()).await?;
    let target = address.to_ascii_lowercase();
    let mut found: Option<Peripheral> = None;
    for _ in 0..15 {
        sleep(Duration::from_millis(500)).await;
        for p in adapter.peripherals().await? {
            if p.address().to_string().to_ascii_lowercase() == target {
                found = Some(p);
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    adapter.stop_scan().await?;
    found.ok_or(OmronError::NotFound)
}
