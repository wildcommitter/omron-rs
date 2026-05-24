//! High-level Omron driver — record reading + record selection.
//!
//! Ported from `omron_ble.omron_driver.OmronDeviceDriver`.  Provides
//! `get_all_records`, `get_latest_record`, `get_latest_records_per_user`, and
//! the EEPROM index-pointer fast path.

use std::collections::{HashMap, HashSet};

use chrono::{Local, NaiveDateTime, Duration as ChronoDuration};
use tracing::{debug, warn};

use crate::device_config::DeviceConfig;
use crate::error::{OmronError, Result};
use crate::record_parsers::Record;
use crate::transport::GattTransport;

/// High-level driver for reading measurement records from an Omron monitor.
pub struct OmronDeviceDriver {
    config: DeviceConfig,
}

impl OmronDeviceDriver {
    pub fn new(config: DeviceConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &DeviceConfig {
        &self.config
    }

    fn finalize_public_record(&self, mut record: Record, user: u32) -> Record {
        record.user = Some(user);
        record.slot_index = None;
        record.offset = None;
        record
    }

    /// Read every record for every configured user.  Returns `[[user1_records], [user2_records], …]`.
    pub async fn get_all_records(
        &self,
        transport: &mut GattTransport,
    ) -> Result<Vec<Vec<Record>>> {
        transport.unlock(None).await?;
        transport.open_memory_session().await?;

        let result: Result<Vec<Vec<Record>>> = async {
            let mut all = Vec::with_capacity(self.config.num_users());
            for user_idx in 0..self.config.num_users() {
                let start = self.config.user_start_addresses[user_idx];
                let total = self.config.per_user_records_count[user_idx]
                    * self.config.record_byte_size;
                let raw = transport
                    .read_memory_range(start, total, self.config.transmission_block_size)
                    .await?;
                all.push(self.parse_user_records(&raw, user_idx, None));
            }
            Ok(all)
        }
        .await;
        let _ = transport.close_memory_session().await;
        result
    }

    pub async fn get_latest_record(
        &self,
        transport: &mut GattTransport,
    ) -> Result<Option<Record>> {
        if let Some(r) = self.get_latest_via_index(transport, false).await?.0 {
            return Ok(Some(r));
        }
        let skip_fallback = self
            .config
            .index_pointer_layout
            .as_ref()
            .map(|l| l.skip_full_scan_fallback_when_index_empty)
            .unwrap_or(false);
        if skip_fallback {
            debug!(model = %self.config.model, "index empty; skipping full scan fallback");
            return Ok(None);
        }
        self.get_latest_via_full_scan(transport).await
    }

    pub async fn get_latest_records_per_user(
        &self,
        transport: &mut GattTransport,
    ) -> Result<HashMap<u32, Record>> {
        let mut latest: HashMap<u32, Record> = HashMap::new();
        let expected = self.config.per_user_records_count.len();

        let (per_user, confirmed_empty) = self.get_latest_via_index_all(transport).await?;
        latest.extend(per_user);
        if latest.len() >= expected {
            return Ok(latest);
        }
        let missing: HashSet<u32> = (1..=expected as u32)
            .filter(|u| !latest.contains_key(u))
            .collect();
        let scan_required: HashSet<u32> =
            missing.difference(&confirmed_empty).copied().collect();
        if scan_required.is_empty() {
            return Ok(latest);
        }
        let all = self.get_all_records(transport).await?;
        for (idx, records) in all.into_iter().enumerate() {
            let user = (idx + 1) as u32;
            if !scan_required.contains(&user) {
                continue;
            }
            let scored: Vec<(u32, Record)> = records.into_iter().map(|r| (user, r)).collect();
            if let Some((u, rec)) = self.select_latest_candidate(&scored) {
                latest.insert(u, self.finalize_public_record(rec, u));
            }
        }
        Ok(latest)
    }

    async fn get_latest_via_full_scan(
        &self,
        transport: &mut GattTransport,
    ) -> Result<Option<Record>> {
        let all = self.get_all_records(transport).await?;
        let mut candidates: Vec<(u32, Record)> = Vec::new();
        for (idx, records) in all.into_iter().enumerate() {
            for r in records {
                candidates.push(((idx + 1) as u32, r));
            }
        }
        Ok(self
            .select_latest_candidate(&candidates)
            .map(|(u, r)| self.finalize_public_record(r, u)))
    }

    fn wrap_pointer_to_range(pointer: i32, min: i32, max: i32) -> Option<i32> {
        if max < min {
            return None;
        }
        let span = max - min + 1;
        if span <= 0 {
            return None;
        }
        let mut p = pointer;
        while p < min {
            p += span;
        }
        while p > max {
            p -= span;
        }
        Some(p)
    }

    async fn get_latest_via_index(
        &self,
        transport: &mut GattTransport,
        _return_all_users: bool,
    ) -> Result<(Option<Record>, HashSet<u32>)> {
        let (per_user, confirmed_empty) = self.get_latest_via_index_all(transport).await?;
        if per_user.is_empty() {
            return Ok((None, confirmed_empty));
        }
        let candidates: Vec<(u32, Record)> = per_user.into_iter().collect();
        if let Some((u, r)) = self.select_latest_candidate(&candidates) {
            Ok((Some(self.finalize_public_record(r, u)), confirmed_empty))
        } else {
            Ok((None, confirmed_empty))
        }
    }

    async fn get_latest_via_index_all(
        &self,
        transport: &mut GattTransport,
    ) -> Result<(HashMap<u32, Record>, HashSet<u32>)> {
        let Some(layout) = self.config.index_pointer_layout.clone() else {
            return Ok((HashMap::new(), HashSet::new()));
        };
        let Some(settings_addr) = self.config.settings_read_address else {
            return Ok((HashMap::new(), HashSet::new()));
        };
        if self.config.record_byte_size == 0 || layout.index_region_byte_size == 0 {
            return Ok((HashMap::new(), HashSet::new()));
        }

        let record_addresses = layout
            .record_addresses
            .clone()
            .unwrap_or_else(|| self.config.user_start_addresses.clone());
        let record_byte_size = layout.record_byte_size.unwrap_or(self.config.record_byte_size);
        let record_step = layout.record_step.unwrap_or(record_byte_size);
        let backtrack_slots = layout.backtrack_slots;
        let collect_all_valid = layout.collect_all_valid_in_index_window;

        let mut candidates: Vec<(u32, Record)> = Vec::new();
        let mut confirmed_empty: HashSet<u32> = HashSet::new();

        transport.unlock(None).await?;
        transport.open_memory_session().await?;

        let result: Result<(Vec<(u32, Record)>, HashSet<u32>)> = async {
            let index_bytes = transport
                .read_memory_range(
                    settings_addr,
                    layout.index_region_byte_size,
                    self.config.transmission_block_size,
                )
                .await?;
            debug!(
                model = %self.config.model,
                index_addr = format!("{:#06x}", settings_addr),
                "index block: {}",
                hex::encode(&index_bytes)
            );

            for (idx, user_cfg) in layout.users.iter().enumerate() {
                if idx >= record_addresses.len()
                    || idx >= self.config.per_user_records_count.len()
                {
                    continue;
                }
                let cursor_off = user_cfg.write_cursor_offset;
                if cursor_off + 2 > index_bytes.len() {
                    debug!(user = idx + 1, "write_cursor_offset out of range, skipping");
                    continue;
                }
                let slice = &index_bytes[cursor_off..cursor_off + 2];
                let raw_pointer = match layout.endianness {
                    crate::record_parsers::Endian::Big => {
                        u16::from_be_bytes([slice[0], slice[1]]) as u32
                    }
                    crate::record_parsers::Endian::Little => {
                        u16::from_le_bytes([slice[0], slice[1]]) as u32
                    }
                };
                let masked = raw_pointer & user_cfg.write_cursor_mask;
                let corrected = (masked as i32) + user_cfg.slot_index_bias;
                let Some(wrapped) = Self::wrap_pointer_to_range(
                    corrected,
                    user_cfg.slot_index_min,
                    user_cfg.slot_index_max,
                ) else {
                    debug!(user = idx + 1, raw = raw_pointer, "cursor wrap failed");
                    continue;
                };
                let record_count = user_cfg.slot_index_max - user_cfg.slot_index_min + 1;
                if record_count <= 0 {
                    continue;
                }
                let max_probe = backtrack_slots.min(record_count as usize - 1);

                let base_addr = record_addresses[idx];
                let mut user_had_any_read = false;
                let mut user_all_empty = true;

                for back in 0..=max_probe {
                    let mut probe_slot = wrapped - back as i32;
                    while probe_slot < user_cfg.slot_index_min {
                        probe_slot += record_count;
                    }
                    let logical = probe_slot - user_cfg.slot_index_min;
                    let probe_addr = base_addr.wrapping_add((logical as u16) * record_step as u16);
                    let raw_record = transport
                        .read_memory_range(
                            probe_addr,
                            record_byte_size,
                            self.config.transmission_block_size,
                        )
                        .await?;
                    user_had_any_read = true;
                    if raw_record.iter().any(|b| *b != 0xFF) {
                        user_all_empty = false;
                    }
                    match self
                        .config
                        .record_parser
                        .parse(&raw_record, self.config.endianness)
                    {
                        Ok(mut parsed) => {
                            parsed.slot_index = Some(probe_slot as usize);
                            if !self.is_record_plausible(&parsed) {
                                continue;
                            }
                            candidates.push(((idx + 1) as u32, parsed));
                            if !collect_all_valid {
                                break;
                            }
                        }
                        Err(e) => {
                            debug!(user = idx + 1, slot = probe_slot, %e, "parse error");
                            continue;
                        }
                    }
                }
                if user_had_any_read && user_all_empty {
                    confirmed_empty.insert((idx + 1) as u32);
                }
            }
            Ok((candidates.clone(), confirmed_empty.clone()))
        }
        .await;

        let _ = transport.close_memory_session().await;
        let (candidates, confirmed_empty) = result?;
        if candidates.is_empty() {
            return Ok((HashMap::new(), confirmed_empty));
        }

        // Per-user selection (Python: only the latest for each user).
        let mut per_user: HashMap<u32, Record> = HashMap::new();
        for user_idx in 0..layout.users.len() {
            let user = (user_idx + 1) as u32;
            let user_candidates: Vec<(u32, Record)> = candidates
                .iter()
                .filter(|(u, _)| *u == user)
                .cloned()
                .collect();
            if let Some((u, r)) = self.select_latest_candidate(&user_candidates) {
                per_user.insert(u, self.finalize_public_record(r, u));
            }
        }
        Ok((per_user, confirmed_empty))
    }

    pub async fn get_all_records_flat(
        &self,
        transport: &mut GattTransport,
    ) -> Result<Vec<Record>> {
        let all = self.get_all_records(transport).await?;
        let mut flat: Vec<Record> = Vec::new();
        for (idx, records) in all.into_iter().enumerate() {
            for mut r in records {
                r.user = Some((idx + 1) as u32);
                flat.push(r);
            }
        }
        flat.sort_by_key(|r| r.datetime.unwrap_or(NaiveDateTime::default()));
        Ok(flat)
    }

    fn parse_user_records(
        &self,
        raw: &[u8],
        user_idx: usize,
        record_byte_size: Option<usize>,
    ) -> Vec<Record> {
        let size = record_byte_size.unwrap_or(self.config.record_byte_size);
        let mut records = Vec::new();
        let empty_record = vec![0xFFu8; size];
        let mut offset = 0;
        while offset + size <= raw.len() {
            let chunk = &raw[offset..offset + size];
            if chunk == empty_record.as_slice() {
                offset += size;
                continue;
            }
            match self.config.record_parser.parse(chunk, self.config.endianness) {
                Ok(mut record) => {
                    record.slot_index = Some(offset / size);
                    record.offset = Some(offset);
                    if self.is_record_plausible(&record) {
                        records.push(record);
                    }
                }
                Err(OmronError::InvalidRecord(_)) => {}
                Err(e) => {
                    warn!(
                        user = user_idx + 1,
                        offset,
                        %e,
                        "error parsing record"
                    );
                }
            }
            offset += size;
        }
        records
    }

    fn select_latest_candidate(
        &self,
        candidates: &[(u32, Record)],
    ) -> Option<(u32, Record)> {
        if candidates.is_empty() {
            return None;
        }
        let key_dt = NaiveDateTime::MIN;
        if self.config.prefer_latest_by_slot_index {
            candidates
                .iter()
                .max_by_key(|(_, r)| (r.slot_index.unwrap_or(0) as i64, r.datetime.unwrap_or(key_dt)))
                .cloned()
        } else {
            candidates
                .iter()
                .max_by_key(|(_, r)| (r.datetime.unwrap_or(key_dt), r.slot_index.unwrap_or(0) as i64))
                .cloned()
        }
    }

    fn is_record_plausible(&self, record: &Record) -> bool {
        let Some(dt) = record.datetime else {
            return false;
        };
        let now = Local::now().naive_local();
        if dt < chrono::NaiveDate::from_ymd_opt(2010, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap() {
            return false;
        }
        if dt > now + ChronoDuration::days(2) {
            return false;
        }
        let sys = record.sys;
        let dia = record.dia;
        let bpm = record.bpm;
        if !(60..=280).contains(&sys) {
            return false;
        }
        if !(30..=180).contains(&dia) {
            return false;
        }
        if !(30..=240).contains(&bpm) {
            return false;
        }
        if dia >= sys {
            return false;
        }
        true
    }
}
