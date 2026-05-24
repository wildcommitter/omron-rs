//! Bluetooth GATT **Record Access Control Point** (RACP), characteristic
//! `0x2A52`. The standard way to enumerate, count, or delete stored
//! measurements on a BLE-SIG-compliant medical device. Records themselves
//! still arrive on the data characteristic (`0x2A35` for BP); RACP only
//! carries the control conversation.
//!
//! Wire format (Bluetooth Core Spec, vol 3 part G §4.0):
//!
//! ```text
//! Request:  <op_code:1> <operator:1> [operand…]
//! Response: 0x06 0x00 <request_op_code:1> <result_code:1>
//! Count:    0x05 0x00 <count_le:2>
//! ```
//!
//! For "Report All Stored Records" the request is exactly two bytes:
//! `0x01 0x01`. The device acks each record on the data char, then sends
//! the Response (`0x06 0x00 0x01 0x01` on success) on RACP itself.

use crate::error::{OmronError, Result};

/// RACP request op codes. Field bytes are exact wire values.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpCode {
    ReportStoredRecords = 0x01,
    DeleteStoredRecords = 0x02,
    Abort = 0x03,
    ReportNumberOfStoredRecords = 0x04,
    NumberOfStoredRecordsResponse = 0x05,
    Response = 0x06,
}

impl OpCode {
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0x01 => Self::ReportStoredRecords,
            0x02 => Self::DeleteStoredRecords,
            0x03 => Self::Abort,
            0x04 => Self::ReportNumberOfStoredRecords,
            0x05 => Self::NumberOfStoredRecordsResponse,
            0x06 => Self::Response,
            _ => return None,
        })
    }
}

/// RACP operators. NULL is what the spec uses for op codes that take no
/// filter (e.g. responses); AllRecords is the universal "every stored
/// record" selector.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Null = 0x00,
    AllRecords = 0x01,
    LessThanOrEqualTo = 0x02,
    GreaterThanOrEqualTo = 0x03,
    WithinRange = 0x04,
    FirstRecord = 0x05,
    LastRecord = 0x06,
}

/// Result codes returned in the second byte of a Response (op `0x06`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ResultCode {
    Success = 0x01,
    OpCodeNotSupported = 0x02,
    InvalidOperator = 0x03,
    OperatorNotSupported = 0x04,
    InvalidOperand = 0x05,
    NoRecordsFound = 0x06,
    AbortUnsuccessful = 0x07,
    ProcedureNotCompleted = 0x08,
    OperandNotSupported = 0x09,
    /// Catch-all for vendor-specific or unknown codes.
    Other = 0xFF,
}

impl ResultCode {
    pub fn from_u8(b: u8) -> Self {
        match b {
            0x01 => Self::Success,
            0x02 => Self::OpCodeNotSupported,
            0x03 => Self::InvalidOperator,
            0x04 => Self::OperatorNotSupported,
            0x05 => Self::InvalidOperand,
            0x06 => Self::NoRecordsFound,
            0x07 => Self::AbortUnsuccessful,
            0x08 => Self::ProcedureNotCompleted,
            0x09 => Self::OperandNotSupported,
            _ => Self::Other,
        }
    }
}

/// A decoded RACP indication from the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RacpIndication {
    /// `0x05 0x00 <count_le_u16>` — response to "Report Number of Stored
    /// Records".
    NumberOfRecords(u16),
    /// `0x06 0x00 <request_op_code> <result_code>` — completion of a
    /// procedure such as "Report All Stored Records".
    Response { request: OpCode, result: ResultCode },
}

/// Build the wire bytes for "Report All Stored Records".
pub fn build_report_all_records() -> [u8; 2] {
    [OpCode::ReportStoredRecords as u8, Operator::AllRecords as u8]
}

/// Build the wire bytes for "Report Number of Stored Records (All)".
pub fn build_report_number_of_records() -> [u8; 2] {
    [OpCode::ReportNumberOfStoredRecords as u8, Operator::AllRecords as u8]
}

/// Build the wire bytes for "Delete All Stored Records".
///
/// Destructive on the device. Currently unused by the CLI.
pub fn build_delete_all_records() -> [u8; 2] {
    [OpCode::DeleteStoredRecords as u8, Operator::AllRecords as u8]
}

/// Build the wire bytes for "Abort Operation" (operator must be NULL).
pub fn build_abort() -> [u8; 2] {
    [OpCode::Abort as u8, Operator::Null as u8]
}

/// Decode a RACP indication. Returns the structured variant or an error
/// for malformed / unknown payloads.
pub fn decode_indication(data: &[u8]) -> Result<RacpIndication> {
    if data.len() < 2 {
        return Err(OmronError::Protocol(format!(
            "RACP indication too short ({} bytes)",
            data.len()
        )));
    }
    let op = OpCode::from_u8(data[0])
        .ok_or_else(|| OmronError::Protocol(format!("unknown RACP op code {:#04x}", data[0])))?;
    match op {
        OpCode::NumberOfStoredRecordsResponse => {
            if data.len() < 4 {
                return Err(OmronError::Protocol(
                    "NumberOfStoredRecordsResponse too short".into(),
                ));
            }
            let count = u16::from_le_bytes([data[2], data[3]]);
            Ok(RacpIndication::NumberOfRecords(count))
        }
        OpCode::Response => {
            if data.len() < 4 {
                return Err(OmronError::Protocol("RACP Response too short".into()));
            }
            let request = OpCode::from_u8(data[2]).ok_or_else(|| {
                OmronError::Protocol(format!("RACP Response: unknown request op {:#04x}", data[2]))
            })?;
            let result = ResultCode::from_u8(data[3]);
            Ok(RacpIndication::Response { request, result })
        }
        other => Err(OmronError::Protocol(format!(
            "unexpected RACP op {:#04x} from device",
            other as u8
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_all_records_bytes() {
        assert_eq!(build_report_all_records(), [0x01, 0x01]);
    }

    #[test]
    fn report_number_bytes() {
        assert_eq!(build_report_number_of_records(), [0x04, 0x01]);
    }

    #[test]
    fn delete_all_bytes() {
        assert_eq!(build_delete_all_records(), [0x02, 0x01]);
    }

    #[test]
    fn abort_bytes() {
        assert_eq!(build_abort(), [0x03, 0x00]);
    }

    #[test]
    fn decode_number_of_records_response() {
        // 0x05 0x00 <count_le>
        let bytes = [0x05, 0x00, 0x07, 0x00]; // 7 records
        assert_eq!(decode_indication(&bytes).unwrap(), RacpIndication::NumberOfRecords(7));
        // Two-byte count
        let bytes = [0x05, 0x00, 0xFF, 0x01]; // 511
        assert_eq!(decode_indication(&bytes).unwrap(), RacpIndication::NumberOfRecords(511));
    }

    #[test]
    fn decode_success_response_for_report_all() {
        // 0x06 0x00 <op of original request> <result>
        let bytes = [0x06, 0x00, 0x01, 0x01];
        assert_eq!(
            decode_indication(&bytes).unwrap(),
            RacpIndication::Response {
                request: OpCode::ReportStoredRecords,
                result: ResultCode::Success,
            }
        );
    }

    #[test]
    fn decode_no_records_found() {
        let bytes = [0x06, 0x00, 0x01, 0x06];
        assert_eq!(
            decode_indication(&bytes).unwrap(),
            RacpIndication::Response {
                request: OpCode::ReportStoredRecords,
                result: ResultCode::NoRecordsFound,
            }
        );
    }

    #[test]
    fn decode_unknown_result_code_is_other() {
        let bytes = [0x06, 0x00, 0x01, 0xFE];
        match decode_indication(&bytes).unwrap() {
            RacpIndication::Response { result: ResultCode::Other, .. } => {}
            other => panic!("expected Response::Other, got {:?}", other),
        }
    }

    #[test]
    fn rejects_truncated() {
        assert!(decode_indication(&[]).is_err());
        assert!(decode_indication(&[0x06]).is_err());
        assert!(decode_indication(&[0x06, 0x00]).is_err());
        assert!(decode_indication(&[0x05, 0x00, 0x07]).is_err()); // missing count high byte
    }

    #[test]
    fn rejects_unknown_op_code() {
        assert!(decode_indication(&[0xFA, 0x00, 0x00, 0x00]).is_err());
    }
}
