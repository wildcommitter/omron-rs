//! Decoder for the BLE standard Blood Pressure Service (UUID `0x1810`).
//!
//! Devices that don't speak Omron's proprietary memory protocol (e.g. the
//! BP7900 / "Omron Complete") deliver measurements via this service. The
//! data flows as **indications** on the *Blood Pressure Measurement*
//! characteristic (`0x2A35`) — one indication per stored or new
//! measurement. The packet layout follows the Bluetooth SIG GATT
//! specification:
//!
//! ```text
//! byte 0       flags  (see BpFlags)
//! bytes 1..3   systolic   (SFLOAT)
//! bytes 3..5   diastolic  (SFLOAT)
//! bytes 5..7   MAP        (SFLOAT)
//! ── optional fields, in this order, gated by the flag bits ──
//! 7 bytes      timestamp  (year LE u16, month, day, hour, minute, second)
//! 2 bytes      pulse rate (SFLOAT)
//! 1 byte       user ID
//! 2 bytes      measurement status (LE u16, bit-flags)
//! ```
//!
//! The numeric fields use the IEEE-11073 "SFLOAT" 16-bit short floating
//! point format, also defined by the spec.

use chrono::NaiveDateTime;

use crate::error::{OmronError, Result};

/// Unit reported by the device for sys/dia/MAP. Pulse rate is always bpm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BpUnit {
    Mmhg,
    Kpa,
}

bitflags::bitflags! {
    /// Per-measurement status bit-flags from the optional Measurement Status
    /// field. Names from the BLE GATT spec for `0x2A35`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct BpMeasurementStatus: u16 {
        const BODY_MOVEMENT          = 1 << 0;
        const CUFF_FIT_LOOSE         = 1 << 1;
        const IRREGULAR_PULSE        = 1 << 2;
        const PULSE_RATE_RANGE_HIGH  = 1 << 3;
        const PULSE_RATE_RANGE_LOW   = 1 << 4;
        const IMPROPER_POSITION      = 1 << 5;
    }
}

impl serde::Serialize for BpMeasurementStatus {
    fn serialize<S>(&self, ser: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u16(self.bits())
    }
}

/// One decoded Blood Pressure Measurement indication.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BpsMeasurement {
    pub sys: f32,
    pub dia: f32,
    pub map: f32,
    pub unit: BpUnit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<NaiveDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<BpMeasurementStatus>,
}

/// IEEE-11073 SFLOAT: 12-bit signed mantissa + 4-bit signed exponent in a
/// 16-bit little-endian word. Returns NaN for the spec's "NaN / NRes /
/// Reserved" sentinels.
///
/// See "Personal Health Devices Transcoding" §2.2 (Health Device Profile).
pub fn decode_sfloat(raw: u16) -> f32 {
    // Spec-defined special values for the 12-bit mantissa.
    let mantissa_bits = raw & 0x0FFF;
    match mantissa_bits {
        0x07FF => return f32::NAN,         // NaN
        0x0800 => return f32::NAN,         // NRes (Not at this Resolution)
        0x07FE => return f32::INFINITY,    // +INFINITY
        0x0802 => return f32::NEG_INFINITY, // -INFINITY
        0x0801 => return f32::NAN,         // Reserved for future use
        _ => {}
    }

    // Sign-extend the 12-bit mantissa to i32.
    let mantissa: i32 = if mantissa_bits & 0x0800 != 0 {
        (mantissa_bits as i32) - 0x1000
    } else {
        mantissa_bits as i32
    };

    // Sign-extend the 4-bit exponent.
    let exp_bits = ((raw >> 12) & 0x0F) as i32;
    let exponent: i32 = if exp_bits & 0x08 != 0 { exp_bits - 0x10 } else { exp_bits };

    (mantissa as f32) * (10f32).powi(exponent)
}

const FLAG_KPA: u8 = 1 << 0;
const FLAG_TIMESTAMP: u8 = 1 << 1;
const FLAG_PULSE_RATE: u8 = 1 << 2;
const FLAG_USER_ID: u8 = 1 << 3;
const FLAG_STATUS: u8 = 1 << 4;

/// Decode a single Blood Pressure Measurement indication payload.
///
/// Returns [`OmronError::InvalidRecord`] if the buffer is shorter than the
/// flags say it should be.
pub fn decode_bp_measurement(data: &[u8]) -> Result<BpsMeasurement> {
    if data.len() < 7 {
        return Err(OmronError::InvalidRecord(format!(
            "BP measurement indication too short ({} bytes)",
            data.len()
        )));
    }
    let flags = data[0];
    let unit = if flags & FLAG_KPA == 0 { BpUnit::Mmhg } else { BpUnit::Kpa };

    let sys = decode_sfloat(u16::from_le_bytes([data[1], data[2]]));
    let dia = decode_sfloat(u16::from_le_bytes([data[3], data[4]]));
    let map = decode_sfloat(u16::from_le_bytes([data[5], data[6]]));
    let mut off = 7;

    let datetime = if flags & FLAG_TIMESTAMP != 0 {
        if data.len() < off + 7 {
            return Err(OmronError::InvalidRecord("BP indication: missing timestamp".into()));
        }
        let year = u16::from_le_bytes([data[off], data[off + 1]]);
        let month = data[off + 2] as u32;
        let day = data[off + 3] as u32;
        let hour = data[off + 4] as u32;
        let min = data[off + 5] as u32;
        let sec = data[off + 6] as u32;
        off += 7;
        chrono::NaiveDate::from_ymd_opt(year as i32, month, day)
            .and_then(|d| d.and_hms_opt(hour, min, sec))
    } else {
        None
    };

    let bpm = if flags & FLAG_PULSE_RATE != 0 {
        if data.len() < off + 2 {
            return Err(OmronError::InvalidRecord("BP indication: missing pulse rate".into()));
        }
        let v = decode_sfloat(u16::from_le_bytes([data[off], data[off + 1]]));
        off += 2;
        Some(v)
    } else {
        None
    };

    let user_id = if flags & FLAG_USER_ID != 0 {
        if data.len() < off + 1 {
            return Err(OmronError::InvalidRecord("BP indication: missing user id".into()));
        }
        let v = data[off];
        off += 1;
        Some(v)
    } else {
        None
    };

    let status = if flags & FLAG_STATUS != 0 {
        if data.len() < off + 2 {
            return Err(OmronError::InvalidRecord("BP indication: missing status".into()));
        }
        Some(BpMeasurementStatus::from_bits_truncate(u16::from_le_bytes([
            data[off],
            data[off + 1],
        ])))
    } else {
        None
    };

    Ok(BpsMeasurement { sys, dia, map, unit, bpm, datetime, user_id, status })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4 || (a.is_nan() && b.is_nan())
    }

    // SFLOAT spec examples — see "ISO/IEEE Std 11073-20601" Annex F.7.

    #[test]
    fn sfloat_integers_no_exponent() {
        // 120 = mantissa 120, exponent 0 → raw 0x0078
        assert!(approx_eq(decode_sfloat(0x0078), 120.0));
        // 80 = 0x0050
        assert!(approx_eq(decode_sfloat(0x0050), 80.0));
        // 0
        assert_eq!(decode_sfloat(0x0000), 0.0);
    }

    #[test]
    fn sfloat_decimal_via_negative_exponent() {
        // 80.5 with exponent -1 → mantissa 805 = 0x325, exp = 0xF
        // raw = (0xF << 12) | 0x325 = 0xF325
        assert!(approx_eq(decode_sfloat(0xF325), 80.5));
        // 36.5 (typical body temp in °C) with exponent -1 → mantissa 365 = 0x16D
        assert!(approx_eq(decode_sfloat(0xF16D), 36.5));
    }

    #[test]
    fn sfloat_negative_mantissa() {
        // -50: mantissa bits = 0xFCE (sign-extends to -50), exp = 0 → raw 0x0FCE
        assert!(approx_eq(decode_sfloat(0x0FCE), -50.0));
    }

    #[test]
    fn sfloat_special_values() {
        assert!(decode_sfloat(0x07FF).is_nan()); // NaN
        assert!(decode_sfloat(0x0800).is_nan()); // NRes
        assert!(decode_sfloat(0x0801).is_nan()); // Reserved
        assert!(decode_sfloat(0x07FE).is_infinite() && decode_sfloat(0x07FE).is_sign_positive());
        assert!(decode_sfloat(0x0802).is_infinite() && decode_sfloat(0x0802).is_sign_negative());
    }

    fn make_bp_indication(
        kpa: bool,
        sys: u16,
        dia: u16,
        map: u16,
        timestamp: Option<(u16, u8, u8, u8, u8, u8)>,
        pulse: Option<u16>,
        user_id: Option<u8>,
        status: Option<u16>,
    ) -> Vec<u8> {
        let mut flags = 0u8;
        if kpa { flags |= FLAG_KPA; }
        if timestamp.is_some() { flags |= FLAG_TIMESTAMP; }
        if pulse.is_some() { flags |= FLAG_PULSE_RATE; }
        if user_id.is_some() { flags |= FLAG_USER_ID; }
        if status.is_some() { flags |= FLAG_STATUS; }

        let mut buf = vec![flags];
        buf.extend_from_slice(&sys.to_le_bytes());
        buf.extend_from_slice(&dia.to_le_bytes());
        buf.extend_from_slice(&map.to_le_bytes());
        if let Some((y, mo, d, h, mi, s)) = timestamp {
            buf.extend_from_slice(&y.to_le_bytes());
            buf.extend_from_slice(&[mo, d, h, mi, s]);
        }
        if let Some(p) = pulse {
            buf.extend_from_slice(&p.to_le_bytes());
        }
        if let Some(u) = user_id {
            buf.push(u);
        }
        if let Some(s) = status {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf
    }

    #[test]
    fn minimal_indication_only_required_fields() {
        let bytes = make_bp_indication(false, 0x0078, 0x0050, 0x005D, None, None, None, None);
        // 7 bytes: flags + 3× SFLOAT
        assert_eq!(bytes.len(), 7);
        let m = decode_bp_measurement(&bytes).unwrap();
        assert_eq!(m.unit, BpUnit::Mmhg);
        assert!(approx_eq(m.sys, 120.0));
        assert!(approx_eq(m.dia, 80.0));
        assert!(approx_eq(m.map, 93.0));
        assert!(m.bpm.is_none());
        assert!(m.datetime.is_none());
        assert!(m.user_id.is_none());
        assert!(m.status.is_none());
    }

    #[test]
    fn full_indication_all_optional_fields() {
        let ts = Some((2026, 1, 15, 10, 30, 0));
        let bytes = make_bp_indication(
            false, 0x0078, 0x0050, 0x005D, ts, Some(0x0048), Some(2), Some(0b0000_0101),
        );
        // 7 (req) + 7 (ts) + 2 (pulse) + 1 (user) + 2 (status) = 19
        assert_eq!(bytes.len(), 19);
        let m = decode_bp_measurement(&bytes).unwrap();
        assert!(approx_eq(m.sys, 120.0));
        assert!(approx_eq(m.dia, 80.0));
        assert!(approx_eq(m.map, 93.0));
        assert!(approx_eq(m.bpm.unwrap(), 72.0));
        assert_eq!(
            m.datetime,
            Some(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap().and_hms_opt(10, 30, 0).unwrap())
        );
        assert_eq!(m.user_id, Some(2));
        let s = m.status.unwrap();
        assert!(s.contains(BpMeasurementStatus::BODY_MOVEMENT));
        assert!(s.contains(BpMeasurementStatus::IRREGULAR_PULSE));
        assert!(!s.contains(BpMeasurementStatus::CUFF_FIT_LOOSE));
    }

    #[test]
    fn kpa_flag_round_trips() {
        let bytes = make_bp_indication(true, 0x0078, 0x0050, 0x005D, None, None, None, None);
        let m = decode_bp_measurement(&bytes).unwrap();
        assert_eq!(m.unit, BpUnit::Kpa);
    }

    #[test]
    fn rejects_truncated_indication() {
        // Too short for required fields.
        assert!(matches!(
            decode_bp_measurement(&[0x00, 0x01, 0x02]),
            Err(OmronError::InvalidRecord(_))
        ));
        // Flags promise timestamp but buffer ends at the required fields.
        let mut b = make_bp_indication(false, 0x0078, 0x0050, 0x005D, None, None, None, None);
        b[0] |= FLAG_TIMESTAMP;
        assert!(matches!(
            decode_bp_measurement(&b),
            Err(OmronError::InvalidRecord(_))
        ));
    }

    #[test]
    fn fractional_sys_decodes() {
        // Sys = 120.5 (exp -1) → mantissa 1205 = 0x4B5 → raw 0xF4B5
        let bytes = make_bp_indication(false, 0xF4B5, 0x0050, 0x005D, None, None, None, None);
        let m = decode_bp_measurement(&bytes).unwrap();
        assert!(approx_eq(m.sys, 120.5));
    }
}
