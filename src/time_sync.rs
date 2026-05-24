//! EEPROM time-sync layout encoders/decoders + CTS time sync flow.
//!
//! Ported from `omron_ble.omron_driver._{decode,encode}_eeprom_time_payload`
//! and `omron_ble.setup_time_sync`.

use std::time::Duration;

use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeZone, Timelike};
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::consts::{CTS_CHARACTERISTIC_UUID, LOCAL_TIME_INFO_UUID};
use crate::device_config::{DeviceConfig, TimeSyncLayout};
use crate::error::Result;
use crate::transport::GattTransport;

/// Decoded representation of the Bluetooth CTS Current Time characteristic
/// (`0x2A2B`). The standard payload is 10 bytes; this type holds the parsed
/// fields without the day-of-week / fractions / adjust-reason metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CtsTime {
    pub datetime: chrono::NaiveDateTime,
    pub day_of_week: u8,
    pub fractions_256: u8,
    pub adjust_reason: u8,
}

/// Decode a CTS `0x2A2B` payload. Returns `None` if the payload is the wrong
/// length or carries an invalid date.
pub fn decode_cts_payload(data: &[u8]) -> Option<CtsTime> {
    if data.len() < 10 {
        return None;
    }
    let year = u16::from_le_bytes([data[0], data[1]]) as i32;
    let month = data[2] as u32;
    let day = data[3] as u32;
    let hour = data[4] as u32;
    let minute = data[5] as u32;
    let second = data[6] as u32;
    let datetime = chrono::NaiveDate::from_ymd_opt(year, month, day)?
        .and_hms_opt(hour, minute, second.min(59))?;
    Some(CtsTime {
        datetime,
        day_of_week: data[7],
        fractions_256: data[8],
        adjust_reason: data[9],
    })
}

/// Build the 10-byte Bluetooth CTS payload from a local timezone-aware datetime.
pub fn build_cts_payload(now: DateTime<Local>) -> Vec<u8> {
    let year = now.year() as u16;
    let mut payload = Vec::with_capacity(10);
    payload.extend_from_slice(&year.to_le_bytes());
    payload.push(now.month() as u8);
    payload.push(now.day() as u8);
    payload.push(now.hour() as u8);
    payload.push(now.minute() as u8);
    payload.push(now.second() as u8);
    // ISO weekday: Monday=1 ... Sunday=7 (CTS format).
    let wd = now.weekday().number_from_monday() as u8;
    payload.push(wd);
    payload.push(0x00); // Fractions256
    payload.push(0x00); // Adjust reason: Unknown
    payload
}

/// Decode the wall time stored in an EEPROM settings block (naive datetime).
pub fn decode_eeprom_time_payload(layout: TimeSyncLayout, cached: &[u8]) -> Option<NaiveDateTime> {
    let dt = |y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32| -> Option<NaiveDateTime> {
        chrono::NaiveDate::from_ymd_opt(y, mo, d).and_then(|nd| nd.and_hms_opt(h, mi, s.min(59)))
    };
    match layout {
        TimeSyncLayout::ModernOffset8 => {
            let b = cached.get(8..14)?;
            dt(b[0] as i32 + 2000, b[1] as u32, b[2] as u32, b[3] as u32, b[4] as u32, b[5] as u32)
        }
        TimeSyncLayout::ClassicOffset8 => {
            let b = cached.get(8..14)?;
            // [month, year-2000, hour, day, second, minute]
            dt(b[1] as i32 + 2000, b[0] as u32, b[3] as u32, b[2] as u32, b[5] as u32, b[4] as u32)
        }
        TimeSyncLayout::Hem6401Prefix => {
            let b = cached.get(0..6)?;
            dt(b[0] as i32 + 2000, b[1] as u32, b[2] as u32, b[3] as u32, b[4] as u32, b[5] as u32)
        }
        TimeSyncLayout::Linear10 => {
            let b = cached.get(2..8)?;
            dt(b[0] as i32 + 2000, b[1] as u32, b[2] as u32, b[3] as u32, b[4] as u32, b[5] as u32)
        }
        TimeSyncLayout::ClassicMixed => {
            let b = cached.get(2..8)?;
            // [month, year-2000, hour, day, second, minute]
            dt(b[1] as i32 + 2000, b[0] as u32, b[3] as u32, b[2] as u32, b[5] as u32, b[4] as u32)
        }
    }
}

/// Build EEPROM settings bytes that encode `now` for a given layout (including
/// the checksum/padding that the Python implementation writes).
pub fn encode_eeprom_time_payload(
    layout: TimeSyncLayout,
    cached: &[u8],
    now: DateTime<Local>,
) -> Vec<u8> {
    let (yr, mo, d, h, mi, s) = (
        (now.year() - 2000) as u8,
        now.month() as u8,
        now.day() as u8,
        now.hour() as u8,
        now.minute() as u8,
        now.second() as u8,
    );

    match layout {
        TimeSyncLayout::ModernOffset8 => {
            let mut result = Vec::with_capacity(16);
            result.extend_from_slice(&cached[0..8.min(cached.len())]);
            while result.len() < 8 {
                result.push(0x00);
            }
            result.extend_from_slice(&[yr, mo, d, h, mi, s]);
            let chk = result.iter().fold(0u8, |a, b| a.wrapping_add(*b));
            result.push(chk);
            result.push(0x00);
            result
        }
        TimeSyncLayout::ClassicOffset8 => {
            let mut result = Vec::with_capacity(16);
            result.extend_from_slice(&cached[0..8.min(cached.len())]);
            while result.len() < 8 {
                result.push(0x00);
            }
            // [month, year-2000, hour, day, second, minute]
            result.extend_from_slice(&[mo, yr, h, d, s, mi]);
            let chk = result.iter().fold(0u8, |a, b| a.wrapping_add(*b));
            result.push(chk);
            result.push(0x00);
            result
        }
        TimeSyncLayout::Hem6401Prefix => {
            let mut result = cached.to_vec();
            if result.len() < 16 {
                result.resize(16, 0x00);
            }
            result[0..6].copy_from_slice(&[yr, mo, d, h, mi, s]);
            result
        }
        TimeSyncLayout::Linear10 => {
            let mut result = Vec::with_capacity(10);
            result.extend_from_slice(&cached[0..2.min(cached.len())]);
            while result.len() < 2 {
                result.push(0x00);
            }
            result.extend_from_slice(&[yr, mo, d, h, mi, s]);
            result.push(0x00);
            let chk = result.iter().fold(0u8, |a, b| a.wrapping_add(*b));
            result.push(chk);
            result
        }
        TimeSyncLayout::ClassicMixed => {
            let mut result = Vec::with_capacity(10);
            result.extend_from_slice(&cached[0..2.min(cached.len())]);
            while result.len() < 2 {
                result.push(0x00);
            }
            // [month, year-2000, hour, day, second, minute]
            result.extend_from_slice(&[mo, yr, h, d, s, mi]);
            result.push(0x00);
            let chk = result.iter().fold(0u8, |a, b| a.wrapping_add(*b));
            result.push(chk);
            result
        }
    }
}

/// Write current local time via Bluetooth CTS (and Local Time Info if present).
/// Returns true if the CTS write actually ran.
pub async fn sync_time_via_cts(transport: &mut GattTransport) -> Result<bool> {
    if !transport.has_characteristic(&CTS_CHARACTERISTIC_UUID) {
        return Ok(false);
    }

    let now = Local::now();
    let payload = build_cts_payload(now);

    transport.subscribe_cts().await?;
    sleep(Duration::from_millis(500)).await;

    let snapshot_ok = transport
        .read_char(&CTS_CHARACTERISTIC_UUID)
        .await
        .map(|b| !b.is_empty())
        .unwrap_or(false);
    let _ = transport.wait_cts_notify(Duration::from_secs(1)).await;

    let mut wrote = false;
    if snapshot_ok {
        transport
            .write_char(&CTS_CHARACTERISTIC_UUID, &payload, true)
            .await?;
        debug!("synced current time via CTS: {}", now);
        wrote = true;
    } else {
        debug!("skipping CTS write: snapshot read failed");
    }

    if transport.has_characteristic(&LOCAL_TIME_INFO_UUID) {
        let utc_off_mins = now.offset().local_minus_utc() / 60;
        let tz_offset_15m = (utc_off_mins / 15) as i8;
        let tz_byte = tz_offset_15m as u8;
        let dst_byte: u8 = 0x00; // chrono doesn't surface DST cleanly; leave 0.
        let _ = transport
            .write_char(&LOCAL_TIME_INFO_UUID, &[tz_byte, dst_byte], true)
            .await;
    }

    transport.unsubscribe_cts().await?;
    Ok(wrote)
}

/// EEPROM-based time sync — preferred for legacy classic-stack profiles that
/// don't use CTS. Returns `Ok(true)` on success, `Ok(false)` if unsupported.
pub async fn sync_time_via_eeprom(
    transport: &mut GattTransport,
    config: &DeviceConfig,
) -> Result<bool> {
    if !config.supports_eeprom_time_sync() {
        return Ok(false);
    }
    let Some([section_start, section_end]) = config.settings_time_sync_bytes else {
        return Ok(false);
    };
    let Some(read_addr) = config.settings_read_address else {
        return Ok(false);
    };
    let Some(write_addr) = config.settings_write_address else {
        return Ok(false);
    };
    let section_size = section_end - section_start;

    let now = Local::now();
    transport.unlock(None).await?;
    transport.open_memory_session().await?;

    let result: Result<bool> = async {
        let cached = transport
            .read_memory_range(
                read_addr + section_start as u16,
                section_size,
                section_size.min(config.transmission_block_size),
            )
            .await?;
        debug!(
            model = %config.model,
            "EEPROM time raw ({} bytes): {}",
            cached.len(),
            hex::encode(&cached)
        );

        let layout = config.resolved_time_sync_layout();
        if let Some(device_dt) = decode_eeprom_time_payload(layout, &cached) {
            let device_local: DateTime<Local> = Local
                .from_local_datetime(&device_dt)
                .single()
                .unwrap_or_else(|| Local.from_utc_datetime(&device_dt));
            let diff = (device_local - now).num_seconds().abs();
            if diff <= 60 {
                debug!(model = %config.model, "device time already in sync");
                return Ok(true);
            }
        }

        let new_payload = encode_eeprom_time_payload(layout, &cached, now);
        let block_size = new_payload.len();
        transport
            .write_memory_range(write_addr + section_start as u16, &new_payload, block_size)
            .await?;
        // Allow the device to commit the EEPROM write internally before the
        // next memory-protocol command.
        sleep(Duration::from_secs(1)).await;
        debug!(model = %config.model, "synced time via EEPROM: {}", now);
        Ok(true)
    }
    .await;

    let _ = transport.close_memory_session().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now_2026_05_24_16_57_12() -> DateTime<Local> {
        // Construct with a fixed offset and convert; the encoders only read
        // year/month/day/hour/minute/second so the offset is irrelevant.
        let naive = chrono::NaiveDate::from_ymd_opt(2026, 5, 24)
            .unwrap()
            .and_hms_opt(16, 57, 12)
            .unwrap();
        Local.from_local_datetime(&naive).unwrap()
    }

    #[test]
    fn encode_modern_offset8_matches_python() {
        let cached: Vec<u8> = (0u8..16).collect();
        let out = encode_eeprom_time_payload(
            TimeSyncLayout::ModernOffset8,
            &cached,
            now_2026_05_24_16_57_12(),
        );
        assert_eq!(hex::encode(&out), "00010203040506071a051810390ca800");
    }

    #[test]
    fn encode_classic_offset8_matches_python() {
        let cached: Vec<u8> = (0u8..16).collect();
        let out = encode_eeprom_time_payload(
            TimeSyncLayout::ClassicOffset8,
            &cached,
            now_2026_05_24_16_57_12(),
        );
        assert_eq!(hex::encode(&out), "0001020304050607051a10180c39a800");
    }

    #[test]
    fn encode_hem6401_prefix_matches_python() {
        let cached: Vec<u8> = (0u8..16).collect();
        let out = encode_eeprom_time_payload(
            TimeSyncLayout::Hem6401Prefix,
            &cached,
            now_2026_05_24_16_57_12(),
        );
        assert_eq!(hex::encode(&out), "1a051810390c060708090a0b0c0d0e0f");
    }

    #[test]
    fn encode_linear_10_matches_python() {
        let mut cached = vec![0u8; 10];
        cached[0] = 0xAA;
        cached[1] = 0xBB;
        let out = encode_eeprom_time_payload(
            TimeSyncLayout::Linear10,
            &cached,
            now_2026_05_24_16_57_12(),
        );
        assert_eq!(hex::encode(&out), "aabb1a051810390c00f1");
    }

    #[test]
    fn encode_classic_mixed_matches_python() {
        let mut cached = vec![0u8; 10];
        cached[0] = 0xAA;
        cached[1] = 0xBB;
        let out = encode_eeprom_time_payload(
            TimeSyncLayout::ClassicMixed,
            &cached,
            now_2026_05_24_16_57_12(),
        );
        assert_eq!(hex::encode(&out), "aabb051a10180c3900f1");
    }

    #[test]
    fn decode_modern_offset8_round_trip() {
        let cached: Vec<u8> = (0u8..16).collect();
        let now = now_2026_05_24_16_57_12();
        let out = encode_eeprom_time_payload(TimeSyncLayout::ModernOffset8, &cached, now);
        let parsed = decode_eeprom_time_payload(TimeSyncLayout::ModernOffset8, &out).unwrap();
        assert_eq!(parsed.format("%Y-%m-%d %H:%M:%S").to_string(), "2026-05-24 16:57:12");
    }

    #[test]
    fn cts_round_trips_through_decode() {
        let now = now_2026_05_24_16_57_12();
        let bytes = build_cts_payload(now);
        let decoded = decode_cts_payload(&bytes).unwrap();
        assert_eq!(decoded.datetime.format("%Y-%m-%d %H:%M:%S").to_string(), "2026-05-24 16:57:12");
        // 2026-05-24 is a Sunday → ISO weekday 7
        assert_eq!(decoded.day_of_week, 7);
        assert_eq!(decoded.fractions_256, 0);
        assert_eq!(decoded.adjust_reason, 0);
    }

    #[test]
    fn cts_decode_rejects_truncated_or_invalid() {
        assert!(decode_cts_payload(&[]).is_none());
        assert!(decode_cts_payload(&[0xEA, 0x07, 5, 24, 16, 57, 12]).is_none()); // 7 < 10
        // Month 13 invalid
        let bad = [0xEA, 0x07, 13, 1, 0, 0, 0, 1, 0, 0];
        assert!(decode_cts_payload(&bad).is_none());
    }

    #[test]
    fn build_cts_payload_length_and_year() {
        let payload = build_cts_payload(now_2026_05_24_16_57_12());
        assert_eq!(payload.len(), 10);
        // little-endian year
        assert_eq!(u16::from_le_bytes([payload[0], payload[1]]), 2026);
        assert_eq!(payload[2], 5);   // month
        assert_eq!(payload[3], 24);  // day
        assert_eq!(payload[4], 16);  // hour
        assert_eq!(payload[5], 57);  // minute
        assert_eq!(payload[6], 12);  // second
        assert_eq!(payload[7], 7);   // 2026-05-24 is a Sunday → ISO weekday 7
    }
}

/// Top-level time sync entry point. Mirrors
/// `setup_time_sync.async_sync_device_time` — EEPROM first when supported, then
/// CTS, then EEPROM again as a retry. Returns true if any path succeeded.
pub async fn sync_device_time(transport: &mut GattTransport) -> Result<bool> {
    let config = transport.config().clone();

    let mut eeprom_ok = false;
    if config.supports_eeprom_time_sync() {
        eeprom_ok = sync_time_via_eeprom(transport, &config).await.unwrap_or_else(|e| {
            warn!(%e, "EEPROM time sync failed");
            false
        });
        if eeprom_ok {
            return Ok(true);
        }
    }

    let cts_ok = sync_time_via_cts(transport).await.unwrap_or_else(|e| {
        warn!(%e, "CTS time sync failed");
        false
    });

    if config.supports_eeprom_time_sync() && !eeprom_ok {
        eeprom_ok = sync_time_via_eeprom(transport, &config).await.unwrap_or_else(|e| {
            warn!(%e, "EEPROM time sync retry failed");
            false
        });
    }

    Ok(cts_ok || eeprom_ok)
}
