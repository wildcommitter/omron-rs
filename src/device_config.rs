//! Device profile configuration ported from `omron_ble/devices.py`.
//!
//! Each Omron model has a `DeviceConfig` describing its BLE topology (which
//! GATT characteristics carry data, whether OS bonding is required, etc.) and
//! its on-device EEPROM layout (where records live, how settings are encoded,
//! and which record-parser to use).

use uuid::Uuid;

use crate::consts::{
    CLASSIC_STACK_PARENT_SERVICE_UUID, CLASSIC_STACK_RX_CHARACTERISTIC_UUIDS,
    CLASSIC_STACK_TX_CHARACTERISTIC_UUIDS, CLASSIC_STACK_UNLOCK_CHARACTERISTIC_UUID,
    MODERN_STACK_PARENT_SERVICE_UUID, STANDARD_BLOOD_PRESSURE_SERVICE_UUID,
};
use crate::record_parsers::{Endian, RecordParser};

/// One per-user entry in the EEPROM index pointer block.
#[derive(Debug, Clone, Copy)]
pub struct IndexUser {
    pub write_cursor_offset: usize,
    pub unread_counter_offset: usize,
    pub write_cursor_mask: u32,
    pub slot_index_min: i32,
    pub slot_index_max: i32,
    pub slot_index_bias: i32,
}

/// Index-pointer layout used by the "latest record" fast path.
#[derive(Debug, Clone)]
pub struct IndexPointerLayout {
    pub index_region_byte_size: usize,
    pub endianness: Endian,
    pub users: Vec<IndexUser>,
    pub record_addresses: Option<Vec<u16>>,
    pub record_byte_size: Option<usize>,
    pub record_step: Option<usize>,
    pub backtrack_slots: usize,
    pub collect_all_valid_in_index_window: bool,
    pub skip_full_scan_fallback_when_index_empty: bool,
}

impl IndexPointerLayout {
    pub fn new(index_region_byte_size: usize, endianness: Endian, users: Vec<IndexUser>) -> Self {
        Self {
            index_region_byte_size,
            endianness,
            users,
            record_addresses: None,
            record_byte_size: None,
            record_step: None,
            backtrack_slots: 0,
            collect_all_valid_in_index_window: false,
            skip_full_scan_fallback_when_index_empty: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeSyncLayout {
    /// `[2:8] = month, year-2000, hour, day, second, minute` (default for the
    /// classic `[0x14, 0x1E]` 10-byte settings block).
    ClassicMixed,
    /// `[2:8] = year-2000, month, day, hour, minute, second` — same 10-byte
    /// window, chronological field order.
    Linear10,
    /// `[8:14] = year-2000, month, day, hour, minute, second` in a 16-byte
    /// `[0x2C, 0x3C]` block.
    ModernOffset8,
    /// `[8:14] = month, year-2000, hour, day, second, minute` in a 16-byte
    /// `[0x2C, 0x3C]` block.
    ClassicOffset8,
    /// HEM-6401 family: `[0:6] = year-2000, month, day, hour, minute, second`
    /// in a 16-byte settings slice; no trailing checksum.
    Hem6401Prefix,
}

impl TimeSyncLayout {
    pub fn from_key(key: &str) -> Self {
        match key {
            "eeprom_time_linear_10" => Self::Linear10,
            "eeprom_time_modern_offset8" => Self::ModernOffset8,
            "eeprom_time_classic_offset8" => Self::ClassicOffset8,
            "eeprom_time_hem6401_prefix" => Self::Hem6401Prefix,
            _ => Self::ClassicMixed,
        }
    }
}

/// Alternate catalog model id that maps onto a canonical profile.
#[derive(Debug, Clone)]
pub struct DeviceModelVariant {
    pub model_id: &'static str,
    pub unverified: bool,
    pub reason: Option<&'static str>,
}

impl DeviceModelVariant {
    pub const fn new(model_id: &'static str) -> Self {
        Self { model_id, unverified: false, reason: None }
    }
    pub const fn unverified(model_id: &'static str) -> Self {
        Self { model_id, unverified: true, reason: None }
    }
    pub const fn unverified_reason(model_id: &'static str, reason: &'static str) -> Self {
        Self { model_id, unverified: true, reason: Some(reason) }
    }
}

#[derive(Debug, Clone)]
pub struct DeviceConfig {
    pub model: String,

    pub parent_service_uuid: Uuid,
    pub rx_channel_uuids: Vec<Uuid>,
    pub tx_channel_uuids: Vec<Uuid>,
    pub unlock_uuid: Uuid,
    pub requires_unlock: bool,
    pub supports_pairing: bool,
    pub supports_os_bonding_only: bool,
    pub ctrl_notify_uuids: Vec<Uuid>,
    pub legacy_pairing_workarounds: bool,

    pub endianness: Endian,
    pub user_start_addresses: Vec<u16>,
    pub per_user_records_count: Vec<usize>,
    pub record_byte_size: usize,
    pub transmission_block_size: usize,

    pub settings_read_address: Option<u16>,
    pub settings_write_address: Option<u16>,
    pub settings_unread_records_bytes: Option<[usize; 2]>,
    pub settings_time_sync_bytes: Option<[usize; 2]>,
    pub time_sync_layout: Option<TimeSyncLayout>,
    pub index_pointer_layout: Option<IndexPointerLayout>,

    pub record_parser: RecordParser,
    pub prefer_latest_by_slot_index: bool,
    pub equivalent_model_ids: Vec<DeviceModelVariant>,
}

impl DeviceConfig {
    /// Build a config with the defaults that match the Python `DeviceConfig`
    /// dataclass: classic-stack BLE topology, 4 RX + 4 TX channels, unlock
    /// required, big-endian EEPROM, 0x0E-byte records, 0x38-byte blocks.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            parent_service_uuid: CLASSIC_STACK_PARENT_SERVICE_UUID,
            rx_channel_uuids: CLASSIC_STACK_RX_CHARACTERISTIC_UUIDS.to_vec(),
            tx_channel_uuids: CLASSIC_STACK_TX_CHARACTERISTIC_UUIDS.to_vec(),
            unlock_uuid: CLASSIC_STACK_UNLOCK_CHARACTERISTIC_UUID,
            requires_unlock: true,
            supports_pairing: true,
            supports_os_bonding_only: false,
            ctrl_notify_uuids: vec![],
            legacy_pairing_workarounds: false,
            endianness: Endian::Big,
            user_start_addresses: vec![],
            per_user_records_count: vec![],
            record_byte_size: 0x0E,
            transmission_block_size: 0x38,
            settings_read_address: None,
            settings_write_address: None,
            settings_unread_records_bytes: None,
            settings_time_sync_bytes: None,
            time_sync_layout: None,
            index_pointer_layout: None,
            record_parser: RecordParser::ClassicVital14,
            prefer_latest_by_slot_index: false,
            equivalent_model_ids: vec![],
        }
    }

    pub fn num_users(&self) -> usize {
        self.user_start_addresses.len()
    }

    pub fn is_single_channel(&self) -> bool {
        self.tx_channel_uuids.len() == 1
    }

    pub fn supports_unread_counter(&self) -> bool {
        self.settings_unread_records_bytes.is_some()
    }

    pub fn supports_eeprom_time_sync(&self) -> bool {
        self.settings_time_sync_bytes.is_some()
            && self.settings_read_address.is_some()
            && self.settings_write_address.is_some()
    }

    pub fn resolved_time_sync_layout(&self) -> TimeSyncLayout {
        if let Some(layout) = self.time_sync_layout {
            return layout;
        }
        if matches!(self.settings_time_sync_bytes, Some([0x2C, 0x3C])) {
            return TimeSyncLayout::ModernOffset8;
        }
        TimeSyncLayout::ClassicMixed
    }

    pub fn parent_service_stack_is_modern(&self) -> bool {
        self.parent_service_uuid == MODERN_STACK_PARENT_SERVICE_UUID
    }

    pub fn is_service_compatible(&self, service_uuids: &[Uuid]) -> bool {
        if self.parent_service_stack_is_modern() {
            service_uuids.contains(&MODERN_STACK_PARENT_SERVICE_UUID)
        } else {
            service_uuids.contains(&CLASSIC_STACK_PARENT_SERVICE_UUID)
        }
    }

    pub fn is_advertisement_compatible(&self, service_uuids: &[Uuid]) -> bool {
        if service_uuids.is_empty() {
            return true;
        }
        if self.is_service_compatible(service_uuids) {
            return true;
        }
        service_uuids.contains(&STANDARD_BLOOD_PRESSURE_SERVICE_UUID)
    }

    // ---- Builder helpers -----------------------------------------------

    pub fn modern_stack_os_bonding(mut self) -> Self {
        self.parent_service_uuid = MODERN_STACK_PARENT_SERVICE_UUID;
        self.rx_channel_uuids = vec![CLASSIC_STACK_RX_CHARACTERISTIC_UUIDS[0]];
        self.tx_channel_uuids = vec![CLASSIC_STACK_TX_CHARACTERISTIC_UUIDS[0]];
        self.requires_unlock = false;
        self.supports_pairing = false;
        self.supports_os_bonding_only = true;
        self
    }

    pub fn legacy_pairing(mut self) -> Self {
        self.legacy_pairing_workarounds = true;
        self
    }

    pub fn endian(mut self, e: Endian) -> Self {
        self.endianness = e;
        self
    }

    pub fn users(mut self, starts: &[u16], counts: &[usize]) -> Self {
        self.user_start_addresses = starts.to_vec();
        self.per_user_records_count = counts.to_vec();
        self
    }

    pub fn record_layout(mut self, byte_size: usize, block_size: usize) -> Self {
        self.record_byte_size = byte_size;
        self.transmission_block_size = block_size;
        self
    }

    pub fn settings(mut self, read: u16, write: u16) -> Self {
        self.settings_read_address = Some(read);
        self.settings_write_address = Some(write);
        self
    }

    pub fn unread_counter(mut self, range: [usize; 2]) -> Self {
        self.settings_unread_records_bytes = Some(range);
        self
    }

    pub fn time_sync(mut self, range: [usize; 2], layout: TimeSyncLayout) -> Self {
        self.settings_time_sync_bytes = Some(range);
        self.time_sync_layout = Some(layout);
        self
    }

    pub fn index_layout(mut self, layout: IndexPointerLayout) -> Self {
        self.index_pointer_layout = Some(layout);
        self
    }

    pub fn parser(mut self, parser: RecordParser) -> Self {
        self.record_parser = parser;
        self
    }

    pub fn prefer_slot_index(mut self) -> Self {
        self.prefer_latest_by_slot_index = true;
        self
    }

    pub fn variants(mut self, variants: Vec<DeviceModelVariant>) -> Self {
        self.equivalent_model_ids = variants;
        self
    }
}
