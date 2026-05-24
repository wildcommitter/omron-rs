//! Pure helpers for the Omron custom-key pairing protocol.
//!
//! The classic-stack Omron BLE pairing handshake is driven by writes and
//! notifications on a single GATT characteristic (the "unlock"
//! characteristic, UUID `b305b680-…`). The wire format is:
//!
//! Host → device (writes):
//!   * `0x02 ‖ 0x00*16` — "I want to program a new key" probe.
//!   * `0x00 ‖ <16-byte key>` — "Commit this new pairing key."
//!   * `0x01 ‖ <16-byte key>` — "Authenticate me with this existing key."
//!
//! Device → host (notifications, first byte selects the message type):
//!   * `0x82 …` — "Ready, send me a new key" (entered key-programming mode).
//!   * `0x80 …` — "New key accepted" (programming succeeded).
//!   * `0x81 …` — "Existing key accepted" (auth/unlock succeeded).
//!
//! The functions in this module construct the host-side byte sequences and
//! recognize the device-side notification prefixes. Keeping them pure makes
//! the pair/unlock flows easy to unit test against the Python reference.

/// Bytes the host writes to enter key-programming mode (prefix `0x02`).
pub fn key_programming_probe_bytes() -> [u8; 17] {
    let mut b = [0u8; 17];
    b[0] = 0x02;
    b
}

/// Bytes the host writes to commit a new 16-byte pairing key (prefix `0x00`).
pub fn pairing_key_program_bytes(key: &[u8; 16]) -> [u8; 17] {
    let mut b = [0u8; 17];
    b[0] = 0x00;
    b[1..].copy_from_slice(key);
    b
}

/// Bytes the host writes to authenticate with an existing pairing key
/// (prefix `0x01`).
pub fn unlock_auth_bytes(key: &[u8; 16]) -> [u8; 17] {
    let mut b = [0u8; 17];
    b[0] = 0x01;
    b[1..].copy_from_slice(key);
    b
}

/// `0x82` — device says it has entered key-programming mode.
pub fn is_key_programming_ready(resp: &[u8]) -> bool {
    matches!(resp.first(), Some(&0x82))
}

/// `0x80` — device accepted the new pairing key we just sent.
pub fn is_pairing_key_ack(resp: &[u8]) -> bool {
    matches!(resp.first(), Some(&0x80))
}

/// `0x81` — device accepted the existing pairing key for auth/unlock.
pub fn is_auth_key_ack(resp: &[u8]) -> bool {
    matches!(resp.first(), Some(&0x81))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::PAIRING_KEY;

    // The hex values below come from running the original Python reference on
    // the same `PAIRING_KEY`; see the project session log for the generator.

    #[test]
    fn default_pairing_key_matches_python() {
        assert_eq!(
            hex::encode(PAIRING_KEY),
            "deadbeaf12341234deadbeaf12341234"
        );
    }

    #[test]
    fn key_programming_probe_matches_python() {
        let bytes = key_programming_probe_bytes();
        assert_eq!(bytes.len(), 17);
        assert_eq!(
            hex::encode(bytes),
            "0200000000000000000000000000000000"
        );
    }

    #[test]
    fn pairing_key_program_matches_python() {
        let bytes = pairing_key_program_bytes(&PAIRING_KEY);
        assert_eq!(bytes.len(), 17);
        assert_eq!(
            hex::encode(bytes),
            "00deadbeaf12341234deadbeaf12341234"
        );
        // First byte is the message-type prefix, remainder is the key verbatim.
        assert_eq!(bytes[0], 0x00);
        assert_eq!(&bytes[1..], &PAIRING_KEY);
    }

    #[test]
    fn unlock_auth_matches_python() {
        let bytes = unlock_auth_bytes(&PAIRING_KEY);
        assert_eq!(bytes.len(), 17);
        assert_eq!(
            hex::encode(bytes),
            "01deadbeaf12341234deadbeaf12341234"
        );
        assert_eq!(bytes[0], 0x01);
        assert_eq!(&bytes[1..], &PAIRING_KEY);
    }

    #[test]
    fn program_and_auth_differ_only_in_prefix() {
        let prog = pairing_key_program_bytes(&PAIRING_KEY);
        let auth = unlock_auth_bytes(&PAIRING_KEY);
        assert_ne!(prog[0], auth[0]);
        assert_eq!(&prog[1..], &auth[1..]);
    }

    #[test]
    fn custom_key_is_round_trippable() {
        let key: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let bytes = pairing_key_program_bytes(&key);
        let mut recovered = [0u8; 16];
        recovered.copy_from_slice(&bytes[1..]);
        assert_eq!(recovered, key);
    }

    #[test]
    fn response_parsers_match_python_predicates() {
        // 0x82 = key programming ready
        assert!(is_key_programming_ready(&[0x82]));
        assert!(is_key_programming_ready(&[0x82, 0x01, 0x02, 0x03]));
        assert!(!is_key_programming_ready(&[]));
        assert!(!is_key_programming_ready(&[0x80]));
        assert!(!is_key_programming_ready(&[0x81]));

        // 0x80 = new key accepted
        assert!(is_pairing_key_ack(&[0x80]));
        assert!(is_pairing_key_ack(&[0x80, 0xff]));
        assert!(!is_pairing_key_ack(&[0x82]));
        assert!(!is_pairing_key_ack(&[]));

        // 0x81 = existing key accepted
        assert!(is_auth_key_ack(&[0x81]));
        assert!(is_auth_key_ack(&[0x81, 0xab, 0xcd]));
        assert!(!is_auth_key_ack(&[0x80]));
        assert!(!is_auth_key_ack(&[0x82]));
        assert!(!is_auth_key_ack(&[]));
    }
}
