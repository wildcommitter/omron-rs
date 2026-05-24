//! EEPROM record byte-layout parsers ported from `omron_ble/record_parsers.py`.
//!
//! Each device family stores blood-pressure measurement records in a slightly
//! different bit-packed layout. The four functions below cover the layouts the
//! original Python integration supports.

use crate::error::{OmronError, Result};
use chrono::NaiveDateTime;

/// Endianness selector for the legacy big-int bit-slicing helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Big,
    Little,
}

impl Endian {
    pub fn from_str(s: &str) -> Self {
        match s {
            "little" => Endian::Little,
            _ => Endian::Big,
        }
    }
}

/// A parsed measurement record. Matches the shape of the dict produced by the
/// Python parsers; optional fields default to `None` when a parser does not
/// populate them.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Record {
    pub sys: u16,
    pub dia: u16,
    pub bpm: u16,
    pub datetime: Option<NaiveDateTime>,
    pub mov: u8,
    pub ihb: u8,
    pub pos: u8,
    pub cuff: u8,
    pub battery: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_id: Option<u16>,
    /// EEPROM slot index for the record (assigned by the driver, not the parser).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot_index: Option<usize>,
    /// Byte offset within the per-user EEPROM region (assigned by the driver).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// 1-based user index (assigned by the driver).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub enum RecordParser {
    ClassicVital14,
    ClassicVital16Hem6401Family,
    ClassicVital14Hem7322Family,
    ClassicVital14Hem6232Family,
}

impl RecordParser {
    pub fn parse(&self, data: &[u8], endian: Endian) -> Result<Record> {
        match self {
            RecordParser::ClassicVital14 => parse_classic_vital_14(data),
            RecordParser::ClassicVital16Hem6401Family => parse_classic_vital_16_6401_family(data),
            RecordParser::ClassicVital14Hem7322Family => {
                parse_classic_vital_14_7322_family(data, endian)
            }
            RecordParser::ClassicVital14Hem6232Family => {
                parse_classic_vital_14_6232_family(data, endian)
            }
        }
    }
}

/// Extract an integer from a bit range within a byte array, mirroring
/// `_bytearray_bits_to_int` in the Python module.
fn bits_to_int(data: &[u8], endian: Endian, first_bit: u32, last_bit: u32) -> u64 {
    let mut big_int: u128 = 0;
    match endian {
        Endian::Big => {
            for b in data {
                big_int = (big_int << 8) | (*b as u128);
            }
        }
        Endian::Little => {
            for b in data.iter().rev() {
                big_int = (big_int << 8) | (*b as u128);
            }
        }
    }
    let total_bits = (data.len() as u32) * 8;
    let shifted = big_int >> (total_bits - (last_bit + 1));
    let num_valid_bits = (last_bit - first_bit) + 1;
    let bitmask: u128 = (1u128 << num_valid_bits) - 1;
    (shifted & bitmask) as u64
}

fn safe_datetime(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Option<NaiveDateTime> {
    chrono::NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|d| d.and_hms_opt(hour, minute, second))
}

pub fn parse_classic_vital_14_7322_family(data: &[u8], endian: Endian) -> Result<Record> {
    let mut r = Record::default();
    r.dia = bits_to_int(data, endian, 0, 7) as u16;
    r.sys = (bits_to_int(data, endian, 8, 15) as u16) + 25;
    let year = bits_to_int(data, endian, 16, 23) as i32 + 2000;
    r.bpm = bits_to_int(data, endian, 24, 31) as u16;
    r.mov = bits_to_int(data, endian, 32, 32) as u8;
    r.ihb = bits_to_int(data, endian, 33, 33) as u8;
    let month = bits_to_int(data, endian, 34, 37) as u32;
    let day = bits_to_int(data, endian, 38, 42) as u32;
    let hour = bits_to_int(data, endian, 43, 47) as u32;
    let minute = bits_to_int(data, endian, 52, 57) as u32;
    let second = (bits_to_int(data, endian, 58, 63) as u32).min(59);
    r.pos = bits_to_int(data, endian, 48, 49) as u8;
    r.battery = bits_to_int(data, endian, 50, 50) as u8;
    r.cuff = bits_to_int(data, endian, 51, 51) as u8;
    r.datetime = safe_datetime(year, month, day, hour, minute, second);
    Ok(r)
}

pub fn parse_classic_vital_14_6232_family(data: &[u8], endian: Endian) -> Result<Record> {
    let mut r = Record::default();
    r.dia = bits_to_int(data, endian, 0, 7) as u16;
    r.sys = (bits_to_int(data, endian, 8, 15) as u16) + 25;
    let year = bits_to_int(data, endian, 18, 23) as i32 + 2000;
    r.bpm = bits_to_int(data, endian, 24, 31) as u16;
    r.mov = bits_to_int(data, endian, 32, 32) as u8;
    r.ihb = bits_to_int(data, endian, 33, 33) as u8;
    let month = bits_to_int(data, endian, 34, 37) as u32;
    let day = bits_to_int(data, endian, 38, 42) as u32;
    let hour = bits_to_int(data, endian, 43, 47) as u32;
    let minute = bits_to_int(data, endian, 52, 57) as u32;
    let second = (bits_to_int(data, endian, 58, 63) as u32).min(59);
    r.pos = bits_to_int(data, endian, 48, 49) as u8;
    r.battery = bits_to_int(data, endian, 50, 50) as u8;
    r.cuff = bits_to_int(data, endian, 51, 51) as u8;
    r.datetime = safe_datetime(year, month, day, hour, minute, second);
    Ok(r)
}

/// Classic 14-byte / 0x0E record layout used by most BP cuffs.
pub fn parse_classic_vital_14(data: &[u8]) -> Result<Record> {
    if data.len() < 8 {
        return Err(OmronError::InvalidRecord("record too short".into()));
    }
    let raw_sys = data[0];
    if raw_sys > 0xE1 {
        return Err(OmronError::InvalidRecord("record slot is empty".into()));
    }

    let mut r = Record::default();
    r.sys = raw_sys as u16 + 25;
    r.dia = data[1] as u16;
    r.bpm = data[2] as u16;

    let year = 2000 + (data[3] & 0x3F) as i32;
    let flags1: u16 = (data[4] as u16) | ((data[5] as u16) << 8);
    let flags2: u16 = (data[6] as u16) | ((data[7] as u16) << 8);

    let hour = (flags1 & 0x1F) as u32;
    let day = ((flags1 >> 5) & 0x1F) as u32;
    let month = ((flags1 >> 10) & 0x0F) as u32;
    r.ihb = ((flags1 >> 14) & 0x01) as u8;
    r.mov = ((flags1 >> 15) & 0x01) as u8;
    let second = ((flags2 & 0x3F) as u32).min(59);
    let minute = (((flags2 >> 6) & 0x3F) as u32).min(59);
    r.cuff = ((flags2 >> 12) & 0x01) as u8;
    r.battery = ((flags2 >> 13) & 0x01) as u8;
    r.pos = ((flags2 >> 14) & 0x03) as u8;

    if data[1] == 0 && data[2] == 0 && (data[3] & 0x3F) == 0 && flags1 == 0 && flags2 == 0 {
        return Err(OmronError::InvalidRecord("record slot is empty".into()));
    }

    if data.len() >= 2 {
        let trailing = &data[data.len() - 2..];
        r.record_id = Some(u16::from_le_bytes([trailing[0], trailing[1]]));
    }

    r.datetime = safe_datetime(year, month, day, hour, minute, second);
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, s).unwrap()
    }

    // Ground-truth bytes + expected values produced by running the original
    // Python parsers (see commit message / session log for the generator).

    #[test]
    fn classic_vital_14_matches_python() {
        let bytes = hex::decode("5f504819cd0d5e0b00000000abcd").unwrap();
        let r = parse_classic_vital_14(&bytes).unwrap();
        assert_eq!(r.sys, 120);
        assert_eq!(r.dia, 80);
        assert_eq!(r.bpm, 72);
        assert_eq!(r.ihb, 0);
        assert_eq!(r.mov, 0);
        assert_eq!(r.cuff, 0);
        assert_eq!(r.battery, 0);
        assert_eq!(r.pos, 0);
        assert_eq!(r.record_id, Some(52651));
        assert_eq!(r.datetime, Some(dt(2025, 3, 14, 13, 45, 30)));
    }

    #[test]
    fn classic_vital_14_empty_slot_is_rejected() {
        // all-zero record body should fail with the "empty slot" InvalidRecord
        let bytes = [0u8; 14];
        assert!(matches!(
            parse_classic_vital_14(&bytes),
            Err(OmronError::InvalidRecord(_))
        ));
        // raw_sys > 0xE1 also rejected
        let mut bytes = [0u8; 14];
        bytes[0] = 0xFF;
        assert!(matches!(
            parse_classic_vital_14(&bytes),
            Err(OmronError::InvalidRecord(_))
        ));
    }

    #[test]
    fn classic_vital_14_7322_family_matches_python() {
        let bytes = hex::decode("557319469289a3d6000000000000").unwrap();
        let r = parse_classic_vital_14_7322_family(&bytes, Endian::Big).unwrap();
        assert_eq!(r.dia, 85);
        assert_eq!(r.sys, 140);
        assert_eq!(r.bpm, 70);
        assert_eq!(r.mov, 1);
        assert_eq!(r.ihb, 0);
        assert_eq!(r.pos, 2);
        assert_eq!(r.battery, 1);
        assert_eq!(r.cuff, 0);
        assert_eq!(r.datetime, Some(dt(2025, 4, 20, 9, 15, 22)));
    }

    #[test]
    fn classic_vital_14_6232_family_matches_python() {
        // 6232 family uses bits 18..23 for year (only 6 bits) — Python parser
        // produced the same byte string as 7322 for these inputs because the
        // year value 25 fits in 6 bits and bits 16..17 happen to be the high
        // bits of the byte-2 (month) packing region.
        let bytes = hex::decode("557319469289a3d6000000000000").unwrap();
        let r = parse_classic_vital_14_6232_family(&bytes, Endian::Big).unwrap();
        assert_eq!(r.dia, 85);
        assert_eq!(r.sys, 140);
        assert_eq!(r.bpm, 70);
        assert_eq!(r.datetime, Some(dt(2025, 4, 20, 9, 15, 22)));
    }

    #[test]
    fn classic_vital_16_6401_family_matches_python() {
        let bytes = hex::decode("1906070e1e2d6e4b4400000900000000").unwrap();
        let r = parse_classic_vital_16_6401_family(&bytes).unwrap();
        assert_eq!(r.sys, 135);
        assert_eq!(r.dia, 75);
        assert_eq!(r.bpm, 68);
        assert_eq!(r.ihb, 1);
        assert_eq!(r.mov, 2);
        assert_eq!(r.datetime, Some(dt(2025, 6, 7, 14, 30, 45)));
    }

    #[test]
    fn bits_to_int_handles_big_endian_basic_cases() {
        // 0x80 in big endian, bits 0..7 should be 0x80
        assert_eq!(bits_to_int(&[0x80], Endian::Big, 0, 7), 0x80);
        // bits 0..0 (high bit) of 0x80 BE = 1
        assert_eq!(bits_to_int(&[0x80], Endian::Big, 0, 0), 1);
        // bits 1..7 of 0x80 BE = 0
        assert_eq!(bits_to_int(&[0x80], Endian::Big, 1, 7), 0);
    }
}

pub fn parse_classic_vital_16_6401_family(data: &[u8]) -> Result<Record> {
    if data.len() < 16 {
        return Err(OmronError::InvalidRecord("record too short".into()));
    }
    let year_off = data[0] as i32;
    let month = data[1] as u32;
    let day = data[2] as u32;
    let hour = data[3] as u32;
    let minute = data[4] as u32;
    let second = (data[5] as u32).min(59);

    let raw_sys = data[6];
    let dia = data[7];
    let bpm = data[8];

    if year_off == 0
        && month == 0
        && day == 0
        && hour == 0
        && minute == 0
        && data[5] == 0
        && raw_sys == 0
        && dia == 0
        && bpm == 0
    {
        return Err(OmronError::InvalidRecord("record slot is empty".into()));
    }
    if raw_sys > 0xE1 {
        return Err(OmronError::InvalidRecord("record slot is empty".into()));
    }

    let flags = data[11];
    let mut r = Record::default();
    r.sys = raw_sys as u16 + 25;
    r.dia = dia as u16;
    r.bpm = bpm as u16;
    r.ihb = flags & 0x03;
    r.mov = (flags >> 2) & 0x03;
    r.datetime = safe_datetime(year_off + 2000, month, day, hour, minute, second);
    Ok(r)
}
