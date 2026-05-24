//! BLE GATT transport for Omron memory protocol, ported from
//! `omron_ble.omron_driver.GattTransport`.
//!
//! Talks the multi-channel command/reply protocol that classic-stack devices
//! use over four "RX" notify characteristics and four "TX" write characteristics
//! (modern-stack devices collapse this down to a single channel). Notifications
//! are routed off a background task into a shared state struct so callers can
//! await replies without juggling stream handles directly.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use btleplug::api::{Characteristic, Peripheral as _, WriteType};
use btleplug::platform::Peripheral;
use futures::StreamExt;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::consts::PAIRING_KEY;
use crate::device_config::DeviceConfig;
use crate::error::{OmronError, Result};
use crate::pairing::{
    is_auth_key_ack, is_key_programming_ready, is_pairing_key_ack, key_programming_probe_bytes,
    pairing_key_program_bytes, unlock_auth_bytes,
};

const MEMORY_PROTOCOL_REPLY_TIMEOUT: Duration = Duration::from_millis(3500);
const MEMORY_PROTOCOL_TX_MAX_RETRIES: u32 = 4;
const MEMORY_PROTOCOL_RETRY_BACKOFF: Duration = Duration::from_millis(250);
const NOTIFY_SUBSCRIBE_SETTLE: Duration = Duration::from_millis(750);

fn xor_crc(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, b| a ^ *b)
}

#[derive(Default, Debug, Clone)]
struct ReplyFrame {
    packet_type: [u8; 2],
    memory_address: [u8; 2],
    payload: Vec<u8>,
}

#[derive(Default)]
struct State {
    channel_fragments: [Option<Vec<u8>>; 4],
    last_reply: Option<ReplyFrame>,
    last_unlock_response: Option<Vec<u8>>,
    last_cts_response: Option<Vec<u8>>,
    notify_subscribed: bool,
    memory_session_depth: u32,
    unlocked: bool,
}

struct Inner {
    state: Mutex<State>,
    reply_ready: Notify,
    unlock_notify: Notify,
    cts_notify: Notify,
}

impl Inner {
    fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            reply_ready: Notify::new(),
            unlock_notify: Notify::new(),
            cts_notify: Notify::new(),
        }
    }
}

/// BLE GATT transport for the Omron memory protocol.
pub struct GattTransport {
    peripheral: Peripheral,
    config: DeviceConfig,
    /// UUID → Characteristic handle, resolved once after service discovery.
    chars: HashMap<Uuid, Characteristic>,
    inner: Arc<Inner>,
    notify_task: Option<JoinHandle<()>>,
}

impl GattTransport {
    pub async fn new(peripheral: Peripheral, config: DeviceConfig) -> Result<Self> {
        // Self-heal: Omron cuffs sleep aggressively, so the link may have
        // dropped between connect() and here. Try one quick reconnect before
        // giving up.
        if !peripheral.is_connected().await? {
            debug!("transport construct: link dropped, reconnecting");
            peripheral.connect().await?;
            sleep(Duration::from_millis(200)).await;
        }
        // Only force a rediscovery if btleplug doesn't already have the GATT
        // tree cached — on BlueZ a fresh discover_services() can take several
        // seconds and the cuff will sleep through it.
        if peripheral.characteristics().is_empty() {
            peripheral.discover_services().await?;
        }
        let mut chars: HashMap<Uuid, Characteristic> = HashMap::new();
        for c in peripheral.characteristics() {
            chars.insert(c.uuid, c);
        }
        if chars.is_empty() {
            return Err(OmronError::Other("no GATT characteristics found".into()));
        }
        Ok(Self {
            peripheral,
            config,
            chars,
            inner: Arc::new(Inner::new()),
            notify_task: None,
        })
    }

    pub fn config(&self) -> &DeviceConfig {
        &self.config
    }

    pub fn peripheral(&self) -> &Peripheral {
        &self.peripheral
    }

    fn char_for(&self, uuid: &Uuid) -> Result<&Characteristic> {
        self.chars
            .get(uuid)
            .ok_or_else(|| OmronError::CharNotFound(uuid.to_string()))
    }

    async fn require_connected(&self, context: &str) -> Result<()> {
        if !self.peripheral.is_connected().await? {
            return Err(OmronError::Disconnected(context.to_string()));
        }
        Ok(())
    }

    /// Spawn the background notification router. Idempotent.
    async fn ensure_notify_task(&mut self) -> Result<()> {
        if self.notify_task.is_some() {
            return Ok(());
        }
        let mut stream = self.peripheral.notifications().await?;
        let inner = self.inner.clone();
        let rx_uuids: Vec<Uuid> = self.config.rx_channel_uuids.clone();
        let unlock_uuid: Uuid = self.config.unlock_uuid;
        let is_single_channel = self.config.is_single_channel();
        let cts_uuid = crate::consts::CTS_CHARACTERISTIC_UUID;

        let handle = tokio::spawn(async move {
            while let Some(n) = stream.next().await {
                let uuid = n.uuid;
                let data = n.value;

                if uuid == unlock_uuid {
                    let mut state = inner.state.lock().await;
                    state.last_unlock_response = Some(data);
                    drop(state);
                    inner.unlock_notify.notify_one();
                    continue;
                }

                if uuid == cts_uuid {
                    let mut state = inner.state.lock().await;
                    state.last_cts_response = Some(data);
                    drop(state);
                    inner.cts_notify.notify_one();
                    continue;
                }

                let channel_index = rx_uuids.iter().position(|u| *u == uuid);
                let Some(channel_index) = channel_index else {
                    debug!(?uuid, "notification on unknown handle");
                    continue;
                };

                let mut state = inner.state.lock().await;
                state.channel_fragments[channel_index] = Some(data);

                // Need channel 0 to know the packet size.
                let Some(first_chunk) = state.channel_fragments[0].clone() else {
                    continue;
                };

                let frame_bytes = if is_single_channel {
                    let bytes = first_chunk;
                    state.channel_fragments = [None, None, None, None];
                    bytes
                } else {
                    let packet_size = first_chunk[0] as usize;
                    let required = (packet_size + 15) / 16;
                    if !(0..required).all(|c| state.channel_fragments[c].is_some()) {
                        continue;
                    }
                    let mut combined = Vec::with_capacity(packet_size);
                    for c in 0..required {
                        combined.extend_from_slice(state.channel_fragments[c].as_ref().unwrap());
                    }
                    combined.truncate(packet_size);
                    state.channel_fragments = [None, None, None, None];
                    combined
                };

                if xor_crc(&frame_bytes) != 0 {
                    warn!(crc = xor_crc(&frame_bytes), "CRC error in rx data");
                    continue;
                }

                let packet_type = [frame_bytes[1], frame_bytes[2]];
                let memory_address = [frame_bytes[3], frame_bytes[4]];
                let expected_data_len = frame_bytes[5] as usize;
                let payload = if expected_data_len > frame_bytes.len() - 8 {
                    vec![0xFF; expected_data_len]
                } else if packet_type == [0x8F, 0x00] {
                    // End-of-transmission: error code in byte 6.
                    vec![frame_bytes[6]]
                } else {
                    frame_bytes[6..6 + expected_data_len].to_vec()
                };

                state.last_reply = Some(ReplyFrame {
                    packet_type,
                    memory_address,
                    payload,
                });
                drop(state);
                inner.reply_ready.notify_one();
            }
        });
        self.notify_task = Some(handle);
        Ok(())
    }

    async fn subscribe_notify_channels(&mut self) -> Result<()> {
        {
            let state = self.inner.state.lock().await;
            if state.notify_subscribed {
                return Ok(());
            }
        }
        self.ensure_notify_task().await?;

        for uuid in self.config.ctrl_notify_uuids.clone() {
            if let Ok(c) = self.char_for(&uuid) {
                if let Err(e) = self.peripheral.subscribe(c).await {
                    debug!(?uuid, %e, "ctrl_notify subscribe skipped");
                }
            }
        }
        for uuid in self.config.rx_channel_uuids.clone() {
            let c = self.char_for(&uuid)?.clone();
            self.peripheral.subscribe(&c).await?;
        }
        sleep(NOTIFY_SUBSCRIBE_SETTLE).await;
        self.inner.state.lock().await.notify_subscribed = true;
        Ok(())
    }

    async fn unsubscribe_notify_channels(&mut self) -> Result<()> {
        for uuid in self.config.rx_channel_uuids.clone() {
            if let Ok(c) = self.char_for(&uuid).cloned() {
                let _ = self.peripheral.unsubscribe(&c).await;
            }
        }
        for uuid in self.config.ctrl_notify_uuids.clone() {
            if let Ok(c) = self.char_for(&uuid).cloned() {
                let _ = self.peripheral.unsubscribe(&c).await;
            }
        }
        self.inner.state.lock().await.notify_subscribed = false;
        Ok(())
    }

    pub async fn reset_session_state(&mut self) -> Result<()> {
        self.unsubscribe_notify_channels().await?;
        let mut s = self.inner.state.lock().await;
        s.unlocked = false;
        s.memory_session_depth = 0;
        s.channel_fragments = [None, None, None, None];
        Ok(())
    }

    /// Issue a memory-protocol command across the TX channels and wait for the
    /// reassembled reply. Retries on TX errors and reply timeouts up to
    /// `MEMORY_PROTOCOL_TX_MAX_RETRIES` times.
    async fn write_command_and_wait_reply(&mut self, command: Vec<u8>) -> Result<ReplyFrame> {
        let max_retries = MEMORY_PROTOCOL_TX_MAX_RETRIES;
        for retry in 0..max_retries {
            // Clear any stale reply before transmitting.
            {
                let mut s = self.inner.state.lock().await;
                s.last_reply = None;
            }
            // Subscribe to the notify_waiters wakeup *before* sending so we
            // never miss an immediate reply.
            let reply_wait = self.inner.reply_ready.notified();
            tokio::pin!(reply_wait);

            let channel_width = if self.config.is_single_channel() {
                command.len().max(16)
            } else {
                16
            };
            let num_tx_channels = (command.len() + channel_width - 1) / channel_width;

            let mut tx_ok = true;
            for ch_idx in 0..num_tx_channels {
                let segment_end = ((ch_idx + 1) * channel_width).min(command.len());
                let segment = &command[ch_idx * channel_width..segment_end];
                let uuid = self.config.tx_channel_uuids[ch_idx];
                let c = match self.char_for(&uuid) {
                    Ok(c) => c.clone(),
                    Err(e) => {
                        tx_ok = false;
                        warn!(%e, "TX char missing during write");
                        break;
                    }
                };
                let write_type = if self.config.is_single_channel() {
                    WriteType::WithoutResponse
                } else {
                    WriteType::WithResponse
                };
                if let Err(e) = self.peripheral.write(&c, segment, write_type).await {
                    warn!(retry = retry + 1, %e, "BLE error during write");
                    tx_ok = false;
                    break;
                }
            }
            if !tx_ok {
                if retry + 1 >= max_retries {
                    return Err(OmronError::Protocol("TX failed after retries".into()));
                }
                sleep(MEMORY_PROTOCOL_RETRY_BACKOFF).await;
                continue;
            }

            match timeout(MEMORY_PROTOCOL_REPLY_TIMEOUT, reply_wait).await {
                Ok(()) => {
                    let s = self.inner.state.lock().await;
                    if let Some(frame) = s.last_reply.clone() {
                        return Ok(frame);
                    }
                }
                Err(_) => {
                    warn!(retry = retry + 1, "TX timeout");
                    if !self.peripheral.is_connected().await? {
                        return Err(OmronError::Disconnected(
                            "while waiting for memory-protocol reply".into(),
                        ));
                    }
                }
            }
            if retry + 1 < max_retries {
                sleep(MEMORY_PROTOCOL_RETRY_BACKOFF).await;
            }
        }
        Err(OmronError::Timeout(format!(
            "no reply after {max_retries} retries"
        )))
    }

    pub async fn open_memory_session(&mut self) -> Result<()> {
        {
            let mut s = self.inner.state.lock().await;
            s.memory_session_depth += 1;
            if s.memory_session_depth > 1 {
                return Ok(());
            }
        }
        let result = async {
            self.require_connected("open_memory_session").await?;
            self.subscribe_notify_channels().await?;
            // ubpm cmd_init: byte[5]=0x10 for all devices.
            let start_cmd = hex::decode("0800000000100018").unwrap();
            let reply = self.write_command_and_wait_reply(start_cmd).await?;
            if reply.packet_type != [0x80, 0x00] {
                return Err(OmronError::Protocol(
                    "invalid response to data readout start".into(),
                ));
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            let mut s = self.inner.state.lock().await;
            if s.memory_session_depth > 0 {
                s.memory_session_depth -= 1;
            }
            s.unlocked = false;
            drop(s);
            let _ = self.unsubscribe_notify_channels().await;
            return Err(e);
        }
        Ok(())
    }

    pub async fn close_memory_session(&mut self) -> Result<()> {
        {
            let mut s = self.inner.state.lock().await;
            if s.memory_session_depth == 0 {
                return Ok(());
            }
            s.memory_session_depth -= 1;
            if s.memory_session_depth > 0 {
                return Ok(());
            }
        }
        let stop_cmd = hex::decode("080f000000000007").unwrap();
        let reply = self.write_command_and_wait_reply(stop_cmd).await?;
        if reply.packet_type != [0x8F, 0x00] {
            warn!("invalid response to data readout end");
        } else if reply.payload.first().copied().unwrap_or(0) != 0 {
            warn!(code = reply.payload[0], "device reported error code during session close");
        }
        self.unsubscribe_notify_channels().await?;
        Ok(())
    }

    pub async fn read_memory_block(&mut self, address: u16, block_size: u8) -> Result<Vec<u8>> {
        let mut cmd: Vec<u8> = Vec::with_capacity(8);
        cmd.extend_from_slice(&[0x08, 0x01, 0x00]);
        cmd.extend_from_slice(&address.to_be_bytes());
        cmd.push(block_size);
        let crc = xor_crc(&cmd);
        cmd.push(0x00);
        cmd.push(crc);

        let reply = self.write_command_and_wait_reply(cmd).await?;
        if reply.memory_address != address.to_be_bytes() {
            return Err(OmronError::Protocol(format!(
                "address mismatch: got {:?}, expected {:#06x}",
                reply.memory_address, address
            )));
        }
        if reply.packet_type != [0x81, 0x00] {
            return Err(OmronError::Protocol("invalid packet type in EEPROM read".into()));
        }
        Ok(reply.payload)
    }

    pub async fn write_memory_block(&mut self, address: u16, data: &[u8]) -> Result<()> {
        let mut cmd: Vec<u8> = Vec::with_capacity(data.len() + 10);
        cmd.push((data.len() + 8) as u8);
        cmd.extend_from_slice(&[0x01, 0xc0]);
        cmd.extend_from_slice(&address.to_be_bytes());
        cmd.push(data.len() as u8);
        cmd.extend_from_slice(data);
        let crc = xor_crc(&cmd);
        cmd.push(0x00);
        cmd.push(crc);

        let reply = self.write_command_and_wait_reply(cmd).await?;
        if reply.memory_address != address.to_be_bytes() {
            return Err(OmronError::Protocol(format!(
                "address mismatch in write: got {:?}, expected {:#06x}",
                reply.memory_address, address
            )));
        }
        if reply.packet_type != [0x81, 0xc0] {
            return Err(OmronError::Protocol("invalid packet type in EEPROM write".into()));
        }
        Ok(())
    }

    pub async fn read_memory_range(
        &mut self,
        start_address: u16,
        bytes_to_read: usize,
        block_size: usize,
    ) -> Result<Vec<u8>> {
        let mut remaining = bytes_to_read;
        let mut addr = start_address;
        let mut out = Vec::with_capacity(bytes_to_read);
        while remaining > 0 {
            let chunk = remaining.min(block_size).min(u8::MAX as usize);
            let payload = self.read_memory_block(addr, chunk as u8).await?;
            out.extend_from_slice(&payload);
            addr = addr.wrapping_add(chunk as u16);
            remaining -= chunk;
        }
        Ok(out)
    }

    pub async fn write_memory_range(
        &mut self,
        start_address: u16,
        data: &[u8],
        block_size: usize,
    ) -> Result<()> {
        let mut addr = start_address;
        let mut offset = 0;
        while offset < data.len() {
            let chunk = (data.len() - offset).min(block_size);
            self.write_memory_block(addr, &data[offset..offset + chunk]).await?;
            addr = addr.wrapping_add(chunk as u16);
            offset += chunk;
        }
        Ok(())
    }

    /// Authenticate to the device with the application-level pairing key.
    ///
    /// This is the unlock step performed before every memory session on
    /// classic-stack devices. Modern-stack / OS-bonding profiles skip it.
    pub async fn unlock(&mut self, key: Option<&[u8; 16]>) -> Result<()> {
        if !self.config.requires_unlock {
            return Ok(());
        }
        {
            let s = self.inner.state.lock().await;
            if s.unlocked {
                return Ok(());
            }
        }
        self.require_connected("unlock").await?;
        let key_bytes: [u8; 16] = key.copied().unwrap_or(PAIRING_KEY);

        self.ensure_notify_task().await?;
        // Prime RX notify so the stack establishes encryption before we write.
        let mut primed_rx = false;
        if let Ok(rx0) = self.char_for(&self.config.rx_channel_uuids[0]).cloned() {
            if self.peripheral.subscribe(&rx0).await.is_ok() {
                primed_rx = true;
                sleep(NOTIFY_SUBSCRIBE_SETTLE).await;
            }
        }

        let unlock_char = self.char_for(&self.config.unlock_uuid)?.clone();
        self.peripheral.subscribe(&unlock_char).await?;
        sleep(NOTIFY_SUBSCRIBE_SETTLE).await;

        let result: Result<()> = async {
            if self.config.legacy_pairing_workarounds {
                // Best-effort "confirm encryption" probe. We pin notified()
                // *before* writing so the wakeup isn't lost if the response
                // arrives before we start awaiting.
                let notified = self.inner.unlock_notify.notified();
                tokio::pin!(notified);
                let probe = key_programming_probe_bytes();
                if self
                    .peripheral
                    .write(&unlock_char, &probe, WriteType::WithResponse)
                    .await
                    .is_ok()
                {
                    let _ = timeout(Duration::from_secs(2), notified).await;
                }
            }

            {
                let mut s = self.inner.state.lock().await;
                s.last_unlock_response = None;
            }
            let notified = self.inner.unlock_notify.notified();
            tokio::pin!(notified);

            let auth = unlock_auth_bytes(&key_bytes);
            self.peripheral
                .write(&unlock_char, &auth, WriteType::WithResponse)
                .await?;
            // `notify_one` stores a permit if no one was registered yet, so
            // this await won't race with the device replying before us.
            timeout(Duration::from_secs(5), notified).await.map_err(|_| {
                OmronError::Unlock("notify timeout while authenticating".into())
            })?;

            let resp = self
                .inner
                .state
                .lock()
                .await
                .last_unlock_response
                .clone()
                .unwrap_or_default();
            if !is_auth_key_ack(&resp) {
                return Err(OmronError::Unlock(format!(
                    "pairing key mismatch (response={})",
                    hex::encode(&resp)
                )));
            }
            self.inner.state.lock().await.unlocked = true;
            Ok(())
        }
        .await;

        let _ = self.peripheral.unsubscribe(&unlock_char).await;
        if primed_rx {
            if let Ok(rx0) = self.char_for(&self.config.rx_channel_uuids[0]).cloned() {
                let _ = self.peripheral.unsubscribe(&rx0).await;
            }
        }
        result
    }

    /// Program a new application-level pairing key. The device must be in
    /// pairing mode (`-P-` blinking) for this to succeed on classic-stack
    /// profiles.
    pub async fn pair(&mut self, key: Option<&[u8; 16]>) -> Result<()> {
        let key_bytes: [u8; 16] = key.copied().unwrap_or(PAIRING_KEY);

        if self.config.supports_os_bonding_only {
            // btleplug doesn't expose an OS-level pair() entry point on every
            // platform; mirror Bleak's "best effort" behaviour and let the
            // platform bond at first encrypted-read. Refresh the GATT cache
            // so any encryption-required characteristics become visible.
            debug!(model = %self.config.model, "OS-bonding-only device — skipping app-level pair");
            self.peripheral.discover_services().await?;
            return Ok(());
        }
        if !self.config.supports_pairing {
            return Err(OmronError::Pairing("device does not support pairing".into()));
        }

        self.ensure_notify_task().await?;

        // Step 1: prime RX notify to trigger SMP Security Request.
        let rx0 = self.char_for(&self.config.rx_channel_uuids[0])?.clone();
        if let Err(e) = self.peripheral.subscribe(&rx0).await {
            debug!(%e, "ignored error starting RX notify");
        }
        sleep(Duration::from_millis(if self.config.legacy_pairing_workarounds {
            250
        } else {
            1000
        }))
        .await;

        // Step 2: subscribe unlock channel.
        let unlock_char = self.char_for(&self.config.unlock_uuid)?.clone();
        let mut subscribed = false;
        let attempts = if self.config.legacy_pairing_workarounds { 10 } else { 5 };
        for attempt in 0..attempts {
            match self.peripheral.subscribe(&unlock_char).await {
                Ok(()) => {
                    subscribed = true;
                    break;
                }
                Err(e) => {
                    debug!(attempt = attempt + 1, %e, "unlock characteristic not ready");
                    sleep(Duration::from_millis(
                        if self.config.legacy_pairing_workarounds { 500 } else { 1000 },
                    ))
                    .await;
                }
            }
        }
        if !subscribed {
            return Err(OmronError::Pairing(format!(
                "characteristic {} not found — clear OS Bluetooth cache and retry in -P- mode",
                self.config.unlock_uuid
            )));
        }

        // Step 3: drive the device into key-programming mode by writing the
        // 0x02 probe until we see a notification prefixed 0x82.
        let mut entered_programming = false;
        let max_retries = 5;
        for attempt in 0..max_retries {
            {
                let s = self.inner.state.lock().await;
                if matches!(s.last_unlock_response.as_deref(), Some(r) if is_key_programming_ready(r))
                {
                    entered_programming = true;
                    break;
                }
            }
            {
                let mut s = self.inner.state.lock().await;
                s.last_unlock_response = None;
            }
            // Register the waiter *before* writing — `notify_one` stores a
            // permit, so even if the device responds in the gap, we won't
            // miss the wakeup.
            let notified = self.inner.unlock_notify.notified();
            tokio::pin!(notified);

            let probe = key_programming_probe_bytes();
            if let Err(e) = self
                .peripheral
                .write(&unlock_char, &probe, WriteType::WithResponse)
                .await
            {
                debug!(attempt = attempt + 1, %e, "key-programming write failed");
            }
            let _ = timeout(Duration::from_secs(2), notified).await;

            let s = self.inner.state.lock().await;
            if let Some(resp) = &s.last_unlock_response {
                if is_key_programming_ready(resp) {
                    entered_programming = true;
                    break;
                }
            }
            drop(s);
            sleep(Duration::from_secs(1)).await;
        }
        if !entered_programming {
            let _ = self.peripheral.unsubscribe(&unlock_char).await;
            let _ = self.peripheral.unsubscribe(&rx0).await;
            return Err(OmronError::Pairing(
                "could not enter key-programming mode — is the device in pairing mode? \
                 (hold the bluetooth button until -P- appears)"
                    .into(),
            ));
        }

        // Step 4: send the new pairing key; the device acknowledges with 0x80.
        {
            let mut s = self.inner.state.lock().await;
            s.last_unlock_response = None;
        }
        let notified = self.inner.unlock_notify.notified();
        tokio::pin!(notified);
        let program = pairing_key_program_bytes(&key_bytes);
        if let Err(e) = self
            .peripheral
            .write(&unlock_char, &program, WriteType::WithResponse)
            .await
        {
            warn!(%e, "failed to write new key");
        }
        let _ = timeout(Duration::from_secs(5), notified).await;
        let resp = self
            .inner
            .state
            .lock()
            .await
            .last_unlock_response
            .clone()
            .unwrap_or_default();
        let _ = self.peripheral.unsubscribe(&unlock_char).await;
        let _ = self.peripheral.unsubscribe(&rx0).await;

        if !is_pairing_key_ack(&resp) {
            return Err(OmronError::Pairing(format!(
                "failed to program pairing key. response={}",
                hex::encode(&resp)
            )));
        }
        sleep(Duration::from_secs(1)).await;
        Ok(())
    }

    /// Subscribe / wait helpers used by the CTS time-sync flow.
    pub async fn subscribe_cts(&mut self) -> Result<()> {
        self.ensure_notify_task().await?;
        let c = self
            .char_for(&crate::consts::CTS_CHARACTERISTIC_UUID)?
            .clone();
        self.peripheral.subscribe(&c).await?;
        Ok(())
    }

    pub async fn unsubscribe_cts(&mut self) -> Result<()> {
        if let Ok(c) = self
            .char_for(&crate::consts::CTS_CHARACTERISTIC_UUID)
            .cloned()
        {
            let _ = self.peripheral.unsubscribe(&c).await;
        }
        Ok(())
    }

    pub async fn wait_cts_notify(&self, dur: Duration) -> Option<Vec<u8>> {
        let notified = self.inner.cts_notify.notified();
        tokio::pin!(notified);
        timeout(dur, notified).await.ok()?;
        self.inner.state.lock().await.last_cts_response.clone()
    }

    pub async fn read_char(&self, uuid: &Uuid) -> Result<Vec<u8>> {
        let c = self.char_for(uuid)?;
        Ok(self.peripheral.read(c).await?)
    }

    pub async fn write_char(&self, uuid: &Uuid, data: &[u8], with_response: bool) -> Result<()> {
        let c = self.char_for(uuid)?;
        let wt = if with_response {
            WriteType::WithResponse
        } else {
            WriteType::WithoutResponse
        };
        self.peripheral.write(c, data, wt).await?;
        Ok(())
    }

    pub fn has_characteristic(&self, uuid: &Uuid) -> bool {
        self.chars.contains_key(uuid)
    }
}

impl Drop for GattTransport {
    fn drop(&mut self) {
        if let Some(h) = self.notify_task.take() {
            h.abort();
        }
    }
}
