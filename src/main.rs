//! `omron` CLI — scan, pair, and read measurements from Omron BLE blood
//! pressure monitors. Rust port of the device-talking pieces of
//! https://github.com/eigger/hass-omron.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use btleplug::api::Peripheral as _;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use omron_rs::ble::{default_adapter, find_peripheral_by_address, scan_for_omron};
use omron_rs::consts::DEFAULT_DEVICE_MODEL;
use omron_rs::devices::{get_device_config, infer_model_id_from_local_name, supported_models};
use omron_rs::driver::OmronDeviceDriver;
use omron_rs::setup::{establish_connection, fetch_device_model_number, pair_and_sync_device};
use omron_rs::transport::GattTransport;

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

async fn cmd_pair(address: String, model_override: Option<String>) -> Result<()> {
    let adapter = default_adapter().await?;
    println!("Scanning for {address}…");
    let peripheral = find_peripheral_by_address(&adapter, &address)
        .await
        .context("could not find the device — is it powered on and in range?")?;
    let resolved_model = resolve_model(&peripheral, model_override).await?;
    println!("Using model profile: {resolved_model}");
    println!(
        "Make sure the device is showing the blinking -P- pairing prompt. \
         (Hold the Bluetooth button on the cuff until -P- appears.)"
    );
    pair_and_sync_device(peripheral, &resolved_model, Some(get_device_config(&resolved_model)))
        .await
        .map_err(|e| anyhow!(e))?;
    println!("Pairing + time sync complete.");
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
