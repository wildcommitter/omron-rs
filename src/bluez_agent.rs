//! In-process BlueZ pairing agent + bonding driver (Linux only).
//!
//! Background: the BLE-standard characteristics this project reads
//! (`0x2A35` BP Measurement, `0x2A52` RACP, plus most Omron classic-stack
//! "memory protocol" channels) are *encryption-required*. BlueZ only
//! initiates SMP when an Agent capable of handling the device's I/O
//! capability is registered on the system bus.  Without an in-process
//! agent the user has to keep a separate `bluetoothctl` session alive
//! during the whole `pair` / `read` / `sync` flow, which is brittle and
//! easy to get wrong.
//!
//! This module registers a tiny **Just Works / NoInputNoOutput** agent
//! (auto-accept everything) for the lifetime of a [`BondingSession`],
//! then drives `org.bluez.Device1.Pair()` against the target peripheral
//! and marks it trusted on success. The agent is unregistered when the
//! session is dropped (or when [`BondingSession::close`] is awaited).
//!
//! Only the bare-minimum OS-bond piece lives here. All GATT traffic
//! still goes through `btleplug`.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use bluer::{
    agent::{Agent, AgentHandle},
    Address, Session,
};
use tokio::time::timeout;
use tracing::{debug, warn};

/// One in-process BlueZ pairing session: holds the system-bus connection,
/// the registered agent handle, and the BlueZ adapter the agent is bound to.
pub struct BondingSession {
    session: Session,
    adapter: bluer::Adapter,
    _agent: AgentHandle,
}

impl BondingSession {
    /// Connect to BlueZ, register a Just-Works `NoInputNoOutput` agent on
    /// the system bus, and select the system default adapter (typically
    /// `hci0`).
    pub async fn new() -> Result<Self> {
        let session = Session::new()
            .await
            .context("connect to BlueZ over system D-Bus")?;
        let agent = Agent {
            // Take over as the system default agent for the lifetime of
            // this handle so SMP requests during pair() get routed to us.
            request_default: true,
            ..Default::default()
        };
        let agent_handle = session
            .register_agent(agent)
            .await
            .context(
                "register Just-Works agent with BlueZ (is another \
                 agent — e.g. an interactive `bluetoothctl` — already running?)",
            )?;
        let adapter = session
            .default_adapter()
            .await
            .context("acquire BlueZ default adapter")?;
        adapter
            .set_powered(true)
            .await
            .context("power on BlueZ adapter")?;
        Ok(Self { session, adapter, _agent: agent_handle })
    }

    pub fn adapter_name(&self) -> &str {
        self.adapter.name()
    }

    /// True if BlueZ currently has the device on file as bonded.
    pub async fn is_paired(&self, address: Address) -> Result<bool> {
        let device = self.adapter.device(address)?;
        Ok(device.is_paired().await?)
    }

    /// Force-remove any cached pairing for `address`. Useful when the cuff
    /// has reset its bond table (Omron monitors do this on every power
    /// cycle) and BlueZ's stored LTK would be rejected.
    pub async fn forget(&self, address: Address) -> Result<()> {
        if let Ok(device) = self.adapter.device(address) {
            // remove_device wants the *path*, not the address.
            if let Err(e) = self.adapter.remove_device(device.address()).await {
                debug!(%address, %e, "forget: remove_device ignored");
            }
        }
        Ok(())
    }

    /// Discover the device via BlueZ's adapter scan if it isn't already in
    /// BlueZ's cache. Returns once the device appears in
    /// `adapter.device_addresses()` or the timeout expires.
    ///
    /// IMPORTANT: the discovery stream returned by
    /// `Adapter::discover_devices` must be held alive while we poll — when
    /// it drops, BlueZ stops discovery for this client and the device
    /// stops getting added to the bus. So we keep `_discovery` in scope
    /// for the whole poll loop.
    pub async fn ensure_discovered(&self, address: Address, dur: Duration) -> Result<()> {
        let known = self.adapter.device_addresses().await?;
        if known.contains(&address) {
            debug!(%address, "already in BlueZ cache");
            return Ok(());
        }
        let _ = self
            .adapter
            .set_discovery_filter(bluer::DiscoveryFilter {
                transport: bluer::DiscoveryTransport::Le,
                ..Default::default()
            })
            .await;
        let _discovery = self
            .adapter
            .discover_devices()
            .await
            .context("start BlueZ discovery")?;
        let deadline = tokio::time::Instant::now() + dur;
        loop {
            let known = self.adapter.device_addresses().await?;
            if known.contains(&address) {
                debug!(%address, "appeared in BlueZ cache");
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "device {} did not appear in BlueZ discovery within {:?}",
                    address,
                    dur
                ));
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// Drive `org.bluez.Device1.Pair()` against the target. With our
    /// NoInputNoOutput agent registered SMP completes silently
    /// (Just Works), and BlueZ stores the LTK so subsequent encrypted
    /// reads succeed.  Idempotent: if the device is already bonded,
    /// returns `Ok(())` without re-pairing.
    pub async fn pair_and_trust(
        &self,
        address: Address,
        pair_timeout: Duration,
    ) -> Result<()> {
        let device = self
            .adapter
            .device(address)
            .with_context(|| format!("BlueZ does not know device {}", address))?;

        if device.is_paired().await.unwrap_or(false) {
            debug!(%address, "already bonded; skipping pair()");
        } else {
            debug!(%address, "calling org.bluez.Device1.Pair()");
            timeout(pair_timeout, device.pair())
                .await
                .map_err(|_| {
                    anyhow!(
                        "OS-level pair() timed out after {:?}. Is the cuff in -P- mode?",
                        pair_timeout
                    )
                })?
                .context("org.bluez.Device1.Pair() failed (cuff probably not in -P-)")?;
        }
        if let Err(e) = device.set_trusted(true).await {
            warn!(%address, %e, "set_trusted failed (continuing anyway)");
        }
        Ok(())
    }

    /// Best-effort agent unregister. `Drop` would handle it implicitly
    /// (the AgentHandle's destructor calls `UnregisterAgent`) but Drop
    /// can't run async cleanly; callers can `close()` to await it
    /// deterministically.
    pub async fn close(self) -> Result<()> {
        // Dropping `_agent` (an `AgentHandle`) inside `Self` triggers the
        // UnregisterAgent D-Bus call asynchronously via bluer's
        // internals.  We don't need to await anything else.
        drop(self.session);
        Ok(())
    }
}

/// Parse a 17-char `XX:XX:XX:XX:XX:XX` MAC into a [`bluer::Address`].
pub fn parse_address(s: &str) -> Result<Address> {
    s.parse::<Address>()
        .map_err(|e| anyhow!("invalid MAC address {:?}: {}", s, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_mac() {
        let a = parse_address("00:5F:BF:A2:C6:C9").unwrap();
        assert_eq!(a.to_string().to_uppercase(), "00:5F:BF:A2:C6:C9");
    }

    #[test]
    fn rejects_garbage_mac() {
        assert!(parse_address("hello").is_err());
        assert!(parse_address("00:5F:BF:A2:C6").is_err()); // too short
    }
}
