//! `omron` CLI — scan, pair, and read measurements from Omron BLE blood
//! pressure monitors. Rust port of the device-talking pieces of
//! https://github.com/eigger/hass-omron.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use btleplug::api::Peripheral as _;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use futures::StreamExt;
use omron_rs::ble::{default_adapter, find_peripheral_by_address, scan_for_omron};
use omron_rs::bps::{decode_bp_measurement, BpsMeasurement};
use omron_rs::consts::{
    BATTERY_LEVEL_UUID, BP_MEASUREMENT_CHAR_UUID, BP_RACP_CHAR_UUID, DEFAULT_DEVICE_MODEL,
    FIRMWARE_REVISION_UUID, HARDWARE_REVISION_UUID, MANUFACTURER_NAME_UUID, MODEL_NUMBER_UUID,
};
use omron_rs::racp;
use omron_rs::devices::{get_device_config, infer_model_id_from_local_name, supported_models};
use omron_rs::driver::OmronDeviceDriver;
use omron_rs::setup::{establish_connection, fetch_device_model_number, pair_and_sync_device};
use omron_rs::transport::GattTransport;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Pair, connect, and read data from Omron BLE blood-pressure monitors",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Scan for nearby Omron devices and print what was found.
    Scan {
        /// How long to scan, in seconds.
        #[arg(long, default_value_t = 8)]
        seconds: u64,
        /// Output JSON.
        #[arg(long)]
        json: bool,
    },
    /// Pair (program a new pairing key) with the device whose address is given.
    /// Put the cuff into pairing mode first (hold the BT button until -P- blinks).
    Pair {
        /// Bluetooth MAC address of the cuff.
        address: String,
        /// Override the inferred model (e.g. HEM-7155T). Defaults to the model
        /// inferred from the BLE local name, or HEM-7142T2 if not detected.
        #[arg(long)]
        model: Option<String>,
    },
    /// Connect to an already-paired device and read all measurement records.
    Read {
        /// Bluetooth MAC address of the cuff.
        address: String,
        /// Override the inferred model.
        #[arg(long)]
        model: Option<String>,
        /// Only fetch the latest record per user (fast path).
        #[arg(long)]
        latest: bool,
        /// Print as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Connect read-only and print Device Information (Model, Firmware, etc.)
    /// plus every advertised GATT service/characteristic. No pairing, no unlock,
    /// no EEPROM access. Safe to run on a never-paired cuff.
    Info {
        /// Bluetooth MAC address of the cuff.
        address: String,
    },
    /// Drain every stored measurement from the device via the BLE-standard
    /// Record Access Control Point (RACP, UUID 0x2A52). Each stored record
    /// arrives as a 0x2A35 indication, then the device sends a RACP
    /// completion response when done. Use on cuffs that expose RACP (most
    /// BPS-compliant devices); for Omron's classic memory-protocol cuffs use
    /// `read` instead. Requires OS-level bonding first
    /// (`bluetoothctl pair <addr>`).
    Sync {
        /// Bluetooth MAC address of the cuff.
        address: String,
        /// Just report the number of stored records — don't fetch them.
        #[arg(long)]
        num_only: bool,
        /// Emit one JSON object per record instead of the table format.
        #[arg(long)]
        json: bool,
        /// Overall timeout in seconds.
        #[arg(long, default_value_t = 90)]
        timeout: u64,
        /// Force a fresh OS-level pairing (drops any cached LTK first).
        /// Use this when reads fail with authentication errors because
        /// the cuff has rotated its bond table. Requires the cuff to be
        /// in -P- mode. Linux only.
        #[arg(long)]
        bond: bool,
    },
    /// Subscribe to the BLE-standard Blood Pressure Service indications
    /// (UUID 0x2A35) and print each measurement as it arrives. Use this on
    /// devices that don't support the Omron memory protocol (e.g. the
    /// BP7900 / "Omron Complete"). Requires OS-level bonding first
    /// (`bluetoothctl pair <addr>`).
    ReadBps {
        /// Bluetooth MAC address of the cuff.
        address: String,
        /// Seconds to wait for indications. After this expires the binary
        /// disconnects and exits.
        #[arg(long, default_value_t = 60)]
        timeout: u64,
        /// Exit after this many measurements have been received. 0 = no limit.
        #[arg(long, default_value_t = 0)]
        count: usize,
        /// Emit one JSON object per line instead of the table format.
        #[arg(long)]
        json: bool,
        /// Force a fresh OS-level pairing (drops any cached LTK first).
        /// Use this when subscribing fails with authentication errors
        /// because the cuff has rotated its bond table. Requires the cuff
        /// to be in -P- mode. Linux only.
        #[arg(long)]
        bond: bool,
    },
    /// Set the cuff's wall-clock via the BLE-standard Current Time Service
    /// (UUID 0x2A2B), and optionally the Local Time Information (0x2A0F)
    /// for timezone / DST. Useful when stored measurement timestamps drift
    /// after a battery change or the cuff has no UI for setting time.
    /// Requires OS-level bonding.
    SetTime {
        /// Bluetooth MAC address of the cuff.
        address: String,
        /// Skip the Local Time Information write (just the wall clock).
        #[arg(long)]
        no_tz: bool,
        /// After writing, read CTS back and print the decoded value to
        /// confirm the cuff accepted the change.
        #[arg(long)]
        verify: bool,
        /// Force a fresh OS-level pairing first. Linux only.
        #[arg(long)]
        bond: bool,
    },
    /// List all supported model IDs (including catalog variants).
    ListModels,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Scan { seconds, json } => cmd_scan(seconds, json).await,
        Command::Pair { address, model } => cmd_pair(address, model).await,
        Command::Read { address, model, latest, json } => cmd_read(address, model, latest, json).await,
        Command::Info { address } => cmd_info(address).await,
        Command::ReadBps { address, timeout, count, json, bond } => {
            cmd_read_bps(address, timeout, count, json, bond).await
        }
        Command::Sync { address, num_only, json, timeout, bond } => {
            cmd_sync(address, num_only, json, timeout, bond).await
        }
        Command::SetTime { address, no_tz, verify, bond } => {
            cmd_set_time(address, no_tz, verify, bond).await
        }
        Command::ListModels => {
            for m in supported_models() {
                println!("{m}");
            }
            Ok(())
        }
    }
}

async fn cmd_scan(seconds: u64, json: bool) -> Result<()> {
    let adapter = default_adapter().await?;
    let results = scan_for_omron(&adapter, Duration::from_secs(seconds)).await?;
    if json {
        let infos: Vec<_> = results.iter().map(|(_, d)| d).collect();
        println!("{}", serde_json::to_string_pretty(&infos)?);
        return Ok(());
    }
    if results.is_empty() {
        println!("No Omron devices found within {seconds}s.");
        return Ok(());
    }
    println!("Found {} Omron device(s):", results.len());
    for (_, d) in &results {
        println!(
            "  {addr}  rssi={rssi}  name={name:?}  model={model:?}  pairing_mode={pm}",
            addr = d.address,
            rssi = d.rssi.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
            name = d.local_name,
            model = d.inferred_model,
            pm = d.in_pairing_mode,
        );
    }
    Ok(())
}

async fn resolve_model(peripheral: &btleplug::platform::Peripheral, override_: Option<String>) -> Result<String> {
    if let Some(m) = override_ {
        return Ok(m);
    }
    if let Some(props) = peripheral.properties().await? {
        if let Some(name) = props.local_name.as_deref() {
            if let Some(m) = infer_model_id_from_local_name(name) {
                return Ok(m);
            }
        }
    }
    Ok(DEFAULT_DEVICE_MODEL.to_string())
}

/// Establish (or verify) an OS-level BlueZ bond with the cuff. Returns
/// the `BondingSession` so the caller can keep the agent registered and
/// the bond active for the rest of the operation.
///
/// `force_fresh` controls whether we drop any cached LTK first and force
/// a brand-new SMP exchange:
/// * `true` for `pair` — the whole point is to set up a fresh bond, and
///   Omron cuffs purge their bond on every power cycle anyway.
/// * `false` for `sync` / `read-bps` by default — fast path that just
///   reuses an existing bond. Pass `--bond` on the CLI to force a
///   refresh when the cached LTK has gone stale.
#[cfg(target_os = "linux")]
async fn establish_os_bond(address: &str, force_fresh: bool) -> Result<omron_rs::bluez_agent::BondingSession> {
    use omron_rs::bluez_agent::{parse_address, BondingSession};
    let addr = parse_address(address)?;
    let session = BondingSession::new()
        .await
        .context("register in-process BlueZ pairing agent")?;
    println!(
        "Registered Just-Works pairing agent on adapter {}.",
        session.adapter_name()
    );
    if force_fresh {
        let _ = session.forget(addr).await;
    } else if session.is_paired(addr).await.unwrap_or(false) {
        println!("Reusing existing OS bond with {address} (pass --bond to force refresh).");
        return Ok(session);
    }
    session
        .ensure_discovered(addr, Duration::from_secs(15))
        .await
        .context("device did not appear in BlueZ discovery (is it advertising?)")?;
    println!("Bonding with {address} via BlueZ Just-Works…");
    session
        .pair_and_trust(addr, Duration::from_secs(30))
        .await?;
    println!("OS-level bond established.");
    Ok(session)
}

async fn cmd_pair(address: String, model_override: Option<String>) -> Result<()> {
    println!(
        "Make sure the device is showing the blinking -P- pairing prompt. \
         (Hold the Bluetooth button on the cuff until -P- appears.)"
    );

    // Step 1: OS-level bond. Always force-fresh here — `pair` is the
    // setup operation, and the cuff's matching key is also about to be
    // re-programmed.
    #[cfg(target_os = "linux")]
    let bonding = establish_os_bond(&address, true).await?;

    // Step 2: locate the peripheral via btleplug for the app-level pair.
    let adapter = default_adapter().await?;
    println!("Scanning for {address}…");
    let peripheral = find_peripheral_by_address(&adapter, &address)
        .await
        .context("could not find the device — is it powered on and in range?")?;
    let resolved_model = resolve_model(&peripheral, model_override).await?;
    println!("Using model profile: {resolved_model}");

    pair_and_sync_device(peripheral, &resolved_model, Some(get_device_config(&resolved_model)))
        .await
        .map_err(|e| anyhow!(e))?;
    println!("Pairing + time sync complete.");

    #[cfg(target_os = "linux")]
    {
        let _ = bonding.close().await;
    }
    Ok(())
}

async fn cmd_read(
    address: String,
    model_override: Option<String>,
    latest_only: bool,
    json: bool,
) -> Result<()> {
    let adapter = default_adapter().await?;
    if !json {
        println!("Scanning for {address}…");
    }
    let peripheral = find_peripheral_by_address(&adapter, &address)
        .await
        .context("could not find the device — is it powered on and in range?")?;

    let mut resolved_model = resolve_model(&peripheral, model_override.clone()).await?;
    establish_connection(&peripheral).await?;

    // If we don't already have an override, try the Model Number characteristic
    // for a more precise id once we've connected.
    if model_override.is_none() {
        if let Ok(Some(actual)) = fetch_device_model_number(&peripheral).await {
            if !actual.is_empty() {
                if let Some(canonical) = infer_model_id_from_local_name(&actual) {
                    resolved_model = canonical;
                } else {
                    resolved_model = actual;
                }
            }
        }
    }
    if !json {
        println!("Using model profile: {resolved_model}");
    }

    let config = get_device_config(&resolved_model);
    let mut transport = GattTransport::new(peripheral, config.clone()).await?;
    let driver = OmronDeviceDriver::new(config);

    if latest_only {
        let per_user = driver
            .get_latest_records_per_user(&mut transport)
            .await
            .map_err(|e| anyhow!(e))?;
        let mut records: Vec<_> = per_user.into_iter().map(|(_, r)| r).collect();
        records.sort_by_key(|r| r.user);
        if json {
            println!("{}", serde_json::to_string_pretty(&records)?);
        } else {
            print_records(&records);
        }
    } else {
        let records = driver
            .get_all_records_flat(&mut transport)
            .await
            .map_err(|e| anyhow!(e))?;
        if json {
            println!("{}", serde_json::to_string_pretty(&records)?);
        } else {
            print_records(&records);
        }
    }
    Ok(())
}

async fn cmd_info(address: String) -> Result<()> {
    let adapter = default_adapter().await?;
    println!("Scanning for {address}…");
    let peripheral = find_peripheral_by_address(&adapter, &address)
        .await
        .context("could not find the device — is it powered on and in range?")?;

    establish_connection(&peripheral).await?;
    println!("Connected. Reading Device Information…");

    async fn read_string(peripheral: &btleplug::platform::Peripheral, uuid: Uuid, label: &str) -> Option<String> {
        for c in peripheral.characteristics() {
            if c.uuid == uuid {
                match peripheral.read(&c).await {
                    Ok(bytes) => {
                        let s = String::from_utf8_lossy(&bytes)
                            .trim_matches(|c: char| c == ' ' || c == '\0')
                            .to_string();
                        println!("  {label:24} = {s:?}");
                        return Some(s);
                    }
                    Err(e) => {
                        println!("  {label:24} read failed: {e}");
                        return None;
                    }
                }
            }
        }
        println!("  {label:24} (characteristic not present)");
        None
    }

    read_string(&peripheral, MANUFACTURER_NAME_UUID, "Manufacturer (0x2A29)").await;
    let model_str = read_string(&peripheral, MODEL_NUMBER_UUID, "Model Number (0x2A24)").await;
    read_string(&peripheral, FIRMWARE_REVISION_UUID, "Firmware Rev (0x2A26)").await;
    read_string(&peripheral, HARDWARE_REVISION_UUID, "Hardware Rev (0x2A27)").await;

    // Battery Level is a single byte 0..100
    for c in peripheral.characteristics() {
        if c.uuid == BATTERY_LEVEL_UUID {
            match peripheral.read(&c).await {
                Ok(b) if !b.is_empty() => println!("  {label:24} = {pct}%", label = "Battery Level (0x2A19)", pct = b[0]),
                Ok(_) => println!("  Battery Level (0x2A19) = (empty read)"),
                Err(e) => println!("  Battery Level (0x2A19) read failed: {e}"),
            }
        }
    }

    println!("\nGATT services:");
    for s in peripheral.services() {
        println!("  service {}", s.uuid);
        for c in &s.characteristics {
            let mut props: Vec<&str> = Vec::new();
            if c.properties.contains(btleplug::api::CharPropFlags::READ) { props.push("read"); }
            if c.properties.contains(btleplug::api::CharPropFlags::WRITE) { props.push("write"); }
            if c.properties.contains(btleplug::api::CharPropFlags::WRITE_WITHOUT_RESPONSE) { props.push("write_no_resp"); }
            if c.properties.contains(btleplug::api::CharPropFlags::NOTIFY) { props.push("notify"); }
            if c.properties.contains(btleplug::api::CharPropFlags::INDICATE) { props.push("indicate"); }
            println!("    char    {}  [{}]", c.uuid, props.join(","));
        }
    }

    if let Some(model_str) = model_str {
        let inferred = infer_model_id_from_local_name(&model_str);
        println!(
            "\nInferred profile for model string {:?}: {}",
            model_str,
            inferred.as_deref().unwrap_or("(unrecognized — use --model to override)")
        );
    }
    Ok(())
}

async fn cmd_sync(
    address: String,
    num_only: bool,
    json: bool,
    timeout_secs: u64,
    bond: bool,
) -> Result<()> {
    use btleplug::api::WriteType;
    use omron_rs::racp::RacpIndication;

    // Ensure an OS bond. By default we reuse any existing bond
    // (`force_fresh=false`); with `--bond` we drop the cached LTK and do
    // a fresh SMP, which requires the cuff to be in -P-.
    #[cfg(target_os = "linux")]
    let _bonding = establish_os_bond(&address, bond).await?;
    #[cfg(not(target_os = "linux"))]
    let _ = bond;

    let adapter = default_adapter().await?;
    if !json {
        println!("Scanning for {address}…");
    }
    let peripheral = find_peripheral_by_address(&adapter, &address)
        .await
        .context("could not find the device — is it powered on and in range?")?;
    establish_connection(&peripheral).await?;

    let chars: Vec<_> = peripheral.characteristics().into_iter().collect();
    let bp_char = chars
        .iter()
        .find(|c| c.uuid == BP_MEASUREMENT_CHAR_UUID)
        .cloned()
        .ok_or_else(|| anyhow!("device does not expose BP Measurement char {}", BP_MEASUREMENT_CHAR_UUID))?;
    let racp_char = chars
        .iter()
        .find(|c| c.uuid == BP_RACP_CHAR_UUID)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "device does not expose RACP char {} — it may not support stored-record sync",
                BP_RACP_CHAR_UUID
            )
        })?;

    let mut stream = peripheral.notifications().await?;
    // Subscribe to RACP first (so the device knows we can hear its responses)
    // then to BP Measurement.
    peripheral.subscribe(&racp_char).await.context(
        "subscribe to RACP (0x2A52) failed — usually means OS bonding is missing \
         (run `bluetoothctl pair <addr>` in a separate shell)",
    )?;
    peripheral.subscribe(&bp_char).await?;

    // Send the request.
    let request = if num_only {
        racp::build_report_number_of_records()
    } else {
        racp::build_report_all_records()
    };
    if !json {
        println!(
            "Sending RACP request {} ({})…",
            hex::encode(request),
            if num_only { "Report Number of Stored Records" } else { "Report All Stored Records" }
        );
    }
    peripheral
        .write(&racp_char, &request, WriteType::WithResponse)
        .await
        .context("RACP write failed")?;

    // Collect data-char indications until RACP sends a completion.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut records: Vec<BpsMeasurement> = Vec::new();
    let mut completion: Option<RacpIndication> = None;
    let mut announced_count: Option<u16> = None;

    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let next = tokio::time::timeout(remaining, stream.next()).await;
        match next {
            Err(_) => break, // overall deadline
            Ok(None) => break, // stream closed
            Ok(Some(n)) => {
                if n.uuid == BP_MEASUREMENT_CHAR_UUID {
                    match decode_bp_measurement(&n.value) {
                        Ok(m) => {
                            if json {
                                println!("{}", serde_json::to_string(&m)?);
                            } else {
                                print_bps_measurement(&m);
                            }
                            records.push(m);
                        }
                        Err(e) => eprintln!(
                            "malformed BP indication ({}): {}",
                            e,
                            hex::encode(&n.value)
                        ),
                    }
                } else if n.uuid == BP_RACP_CHAR_UUID {
                    match racp::decode_indication(&n.value) {
                        Ok(ind) => match ind {
                            RacpIndication::NumberOfRecords(n) => {
                                announced_count = Some(n);
                                if !json {
                                    println!("Device reports {} stored record(s).", n);
                                }
                                if num_only {
                                    completion = Some(ind);
                                    break;
                                }
                            }
                            RacpIndication::Response { .. } => {
                                completion = Some(ind);
                                break;
                            }
                        },
                        Err(e) => eprintln!(
                            "malformed RACP indication ({}): {}",
                            e,
                            hex::encode(&n.value)
                        ),
                    }
                }
            }
        }
    }

    let _ = peripheral.unsubscribe(&bp_char).await;
    let _ = peripheral.unsubscribe(&racp_char).await;
    let _ = peripheral.disconnect().await;

    if !json {
        match completion {
            Some(RacpIndication::Response { request, result }) => {
                println!(
                    "RACP completion: request={:?} result={:?} (received {} record(s))",
                    request,
                    result,
                    records.len()
                );
            }
            Some(RacpIndication::NumberOfRecords(_)) | None if num_only => {}
            None => {
                eprintln!(
                    "Timed out waiting for RACP completion after {}s ({} record(s) received{}).",
                    timeout_secs,
                    records.len(),
                    announced_count
                        .map(|n| format!(", expected {}", n))
                        .unwrap_or_default()
                );
            }
            _ => {}
        }
    }
    Ok(())
}

async fn cmd_set_time(
    address: String,
    no_tz: bool,
    verify: bool,
    bond: bool,
) -> Result<()> {
    use btleplug::api::WriteType;
    use chrono::Local;
    use omron_rs::consts::{CTS_CHARACTERISTIC_UUID, LOCAL_TIME_INFO_UUID};
    use omron_rs::time_sync::{build_cts_payload, decode_cts_payload};

    #[cfg(target_os = "linux")]
    let _bonding = establish_os_bond(&address, bond).await?;
    #[cfg(not(target_os = "linux"))]
    let _ = bond;

    let adapter = default_adapter().await?;
    println!("Scanning for {address}…");
    let peripheral = find_peripheral_by_address(&adapter, &address)
        .await
        .context("could not find the device — is it powered on and in range?")?;
    establish_connection(&peripheral).await?;

    let chars: Vec<_> = peripheral.characteristics().into_iter().collect();
    let cts = chars
        .iter()
        .find(|c| c.uuid == CTS_CHARACTERISTIC_UUID)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "device does not expose Current Time Service char {} \
                 — it may not implement BLE-standard CTS",
                CTS_CHARACTERISTIC_UUID
            )
        })?;

    let now = Local::now();
    let payload = build_cts_payload(now);
    println!(
        "Writing CTS (0x2A2B) = {} ({})",
        hex::encode(&payload),
        now.format("%Y-%m-%d %H:%M:%S %z")
    );
    peripheral
        .write(&cts, &payload, WriteType::WithResponse)
        .await
        .context("CTS write failed")?;

    if !no_tz {
        if let Some(lti) = chars.iter().find(|c| c.uuid == LOCAL_TIME_INFO_UUID).cloned() {
            // Timezone in 15-minute units (i8), DST byte: chrono doesn't surface
            // DST cleanly in stable so leave it 0 (standard time).
            let utc_off_mins = now.offset().local_minus_utc() / 60;
            let tz_offset_15m = (utc_off_mins / 15) as i8;
            let lti_payload = [tz_offset_15m as u8, 0x00];
            println!(
                "Writing Local Time Info (0x2A0F) = {} (tz_offset_15m={})",
                hex::encode(lti_payload),
                tz_offset_15m
            );
            if let Err(e) = peripheral.write(&lti, &lti_payload, WriteType::WithResponse).await {
                eprintln!("LTI write failed (continuing): {e}");
            }
        } else {
            println!("Local Time Info characteristic not present — skipping timezone write.");
        }
    }

    if verify {
        match peripheral.read(&cts).await {
            Ok(bytes) => {
                println!("CTS read-back: {}", hex::encode(&bytes));
                if let Some(decoded) = decode_cts_payload(&bytes) {
                    let now_local = now.naive_local();
                    let drift = (decoded.datetime - now_local).num_milliseconds().abs();
                    println!(
                        "  decoded: {}  weekday={}  drift={}ms",
                        decoded.datetime.format("%Y-%m-%d %H:%M:%S"),
                        decoded.day_of_week,
                        drift
                    );
                    // The cuff may be a tick ahead/behind — 2s is well
                    // within BLE round-trip + cuff clock granularity.
                    if drift > 2000 {
                        eprintln!("  warning: drift > 2s, the cuff may not have accepted the write");
                    }
                } else {
                    eprintln!("  failed to decode read-back payload");
                }
            }
            Err(e) => eprintln!("CTS read-back failed (write may still have stuck): {e}"),
        }
    }

    let _ = peripheral.disconnect().await;
    println!("Done.");
    Ok(())
}

async fn cmd_read_bps(
    address: String,
    timeout_secs: u64,
    count_limit: usize,
    json: bool,
    bond: bool,
) -> Result<()> {
    // Ensure an OS bond. Default fast path reuses any existing bond;
    // `--bond` forces a fresh SMP (cuff must be in -P-).
    #[cfg(target_os = "linux")]
    let _bonding = establish_os_bond(&address, bond).await?;
    #[cfg(not(target_os = "linux"))]
    let _ = bond;

    let adapter = default_adapter().await?;
    if !json {
        println!("Scanning for {address}…");
    }
    let peripheral = find_peripheral_by_address(&adapter, &address)
        .await
        .context("could not find the device — is it powered on and in range?")?;
    establish_connection(&peripheral).await?;

    // Locate the standard BP Measurement characteristic (0x2A35).
    let char = peripheral
        .characteristics()
        .into_iter()
        .find(|c| c.uuid == BP_MEASUREMENT_CHAR_UUID)
        .ok_or_else(|| {
            anyhow!(
                "device does not expose BP Measurement characteristic {} \
                 — it may not implement the BLE-standard BP Service",
                BP_MEASUREMENT_CHAR_UUID
            )
        })?;

    // Open the notification stream BEFORE subscribing so we don't miss the
    // first indication.
    let mut stream = peripheral.notifications().await?;
    peripheral.subscribe(&char).await.context(
        "subscribe to 0x2A35 failed — usually means the cuff requires OS-level \
         bonding first (run `bluetoothctl pair <addr>` in a separate shell)",
    )?;

    if !json {
        println!(
            "Subscribed to {} (BP Measurement, indicate). Waiting up to {}s for measurements{}…",
            BP_MEASUREMENT_CHAR_UUID,
            timeout_secs,
            if count_limit > 0 { format!(" (max {})", count_limit) } else { String::new() },
        );
    }

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut received = 0usize;
    let mut all: Vec<BpsMeasurement> = Vec::new();
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let next = tokio::time::timeout(remaining, stream.next()).await;
        match next {
            Err(_) => break, // overall deadline
            Ok(None) => break, // stream ended
            Ok(Some(n)) => {
                if n.uuid != BP_MEASUREMENT_CHAR_UUID {
                    continue; // unrelated notify
                }
                match decode_bp_measurement(&n.value) {
                    Ok(m) => {
                        if json {
                            println!("{}", serde_json::to_string(&m)?);
                        } else {
                            print_bps_measurement(&m);
                        }
                        all.push(m);
                        received += 1;
                        if count_limit > 0 && received >= count_limit {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("malformed indication ({}): {}", e, hex::encode(&n.value));
                    }
                }
            }
        }
    }

    let _ = peripheral.unsubscribe(&char).await;
    let _ = peripheral.disconnect().await;

    if !json && received == 0 {
        eprintln!(
            "No BP measurements received within {}s. Take a measurement on the cuff \
             (or wait for the device to push stored ones) while this is running.",
            timeout_secs
        );
    }
    let _ = all;
    Ok(())
}

fn print_bps_measurement(m: &BpsMeasurement) {
    let dt = m
        .datetime
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "?".to_string());
    let unit = match m.unit {
        omron_rs::bps::BpUnit::Mmhg => "mmHg",
        omron_rs::bps::BpUnit::Kpa => "kPa",
    };
    let bpm = m
        .bpm
        .map(|v| format!("{:.0} bpm", v))
        .unwrap_or_else(|| "?bpm".into());
    let user = m
        .user_id
        .map(|u| format!(" user={}", u))
        .unwrap_or_default();
    let status = m
        .status
        .map(|s| format!(" status={:#018b}", s.bits()))
        .unwrap_or_default();
    println!(
        "{dt}  {sys:.0}/{dia:.0} {unit}  MAP {map:.0}  {bpm}{user}{status}",
        sys = m.sys,
        dia = m.dia,
        map = m.map,
    );
}

fn print_records(records: &[omron_rs::Record]) {
    if records.is_empty() {
        println!("(no records)");
        return;
    }
    println!(
        "{:>4}  {:>19}  {:>4}  {:>4}  {:>4}  flags",
        "user", "datetime", "sys", "dia", "bpm"
    );
    for r in records {
        let dt = r
            .datetime
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "?".to_string());
        let flags = format!(
            "ihb={} mov={} pos={} cuff={} batt={}",
            r.ihb, r.mov, r.pos, r.cuff, r.battery
        );
        println!(
            "{:>4}  {:>19}  {:>4}  {:>4}  {:>4}  {}",
            r.user.map(|u| u.to_string()).unwrap_or_else(|| "?".into()),
            dt,
            r.sys,
            r.dia,
            r.bpm,
            flags
        );
    }
}
