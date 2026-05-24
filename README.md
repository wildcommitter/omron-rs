# omron-rs

A Rust CLI to scan, pair, and read measurement data from Omron Bluetooth blood
pressure monitors. Rust port of the BLE pieces of [eigger/hass-omron][upstream]
(a Home Assistant integration), plus a from-scratch implementation of the
BLE-standard Blood Pressure Service / RACP for devices the upstream
integration doesn't cover.

[upstream]: https://github.com/eigger/hass-omron

## Build

```sh
cargo build --release
./target/release/omron --help
```

Requires a working BLE adapter. On Linux this is BlueZ (D-Bus). Tested with
BlueZ 5.86 on a built-in Intel adapter.

## Commands

| Subcommand   | What it does                                                                                          |
| ------------ | ----------------------------------------------------------------------------------------------------- |
| `scan`       | Find nearby Omron devices.                                                                            |
| `info`       | Connect read-only, print Device Information (Model, Firmware, …) and the full GATT tree.              |
| `pair`       | Run the Omron app-level pairing handshake. Commits a 16-byte key into the cuff EEPROM. Needs `-P-`.   |
| `read`       | Drain stored measurements via Omron's classic memory protocol. For `HEM-*` cuffs in the supported list. |
| `read-bps`   | Subscribe to BLE-standard BP Measurement indications (`0x2A35`). For BPS-compliant cuffs.             |
| `sync`       | Drain *every* stored record via BLE-standard RACP (`0x2A52`). For BPS-compliant cuffs.                |
| `list-models`| Print all 202 supported model IDs (canonical profiles + aliases).                                     |

Run `omron <subcommand> --help` for flags.

## Two protocols, two paths

Omron BLE cuffs fall into two camps:

1. **Omron classic memory protocol** (proprietary, used by the `HEM-*` models
   in the upstream `hass-omron` catalog). Pairing uses a 16-byte
   application-level key written to a vendor unlock characteristic; records
   live in EEPROM and are read via a custom command/reply protocol over four
   RX + four TX vendor characteristics.
   → Use `pair` once (with the cuff in `-P-` mode), then `read`.

2. **BLE-standard Blood Pressure Service** (Bluetooth SIG specification,
   used by e.g. the BP7900 / "Omron Complete"). Measurements arrive as
   indications on `0x2A35`; stored history is drained via the RACP
   (`0x2A52`) Report-All-Stored-Records request.
   → Use `read-bps` for live measurements, `sync` for the full history.

`info` will tell you which characteristics your cuff exposes, which makes it
obvious which path you need.

## Worked example: the Omron Complete (BP7900)

```sh
# 1. Find it
$ omron scan --seconds 15
Found 1 Omron device(s):
  00:5F:BF:A2:C6:C9  name=Some("BLEsmart_00000251005FBFA2C6C9")  pairing_mode=false

# 2. Confirm what it is
$ omron info 00:5F:BF:A2:C6:C9
  Manufacturer (0x2A29)    = "OMRONHEALTHCARE"
  Model Number (0x2A24)    = "Complete"
  Firmware Rev (0x2A26)    = "D.00.7FB-12"
  …

# 3. Bond at the OS level (one-time; put the cuff in -P-, then run):
$ bluetoothctl
[bluetoothctl]> agent NoInputNoOutput
[bluetoothctl]> default-agent
[bluetoothctl]> scan on
[bluetoothctl]> pair 00:5F:BF:A2:C6:C9
[CHG] Device 00:5F:BF:A2:C6:C9 Bonded: yes
Pairing successful

# 4. Drain every stored measurement
$ omron sync 00:5F:BF:A2:C6:C9
2026-05-20 08:52:42  108/64 mmHg  MAP 78  78 bpm user=1
2026-05-20 08:53:56  109/62 mmHg  MAP 77  77 bpm user=1
…
2026-05-24 18:10:58  105/63 mmHg  MAP 77  74 bpm user=1
RACP completion: request=ReportStoredRecords result=Success (received 90 record(s))
```

`--json` on `read-bps`, `sync`, and `read` emits one record per line as JSON
for piping into other tools.

## Pairing mode (`-P-`)

For commands that need pairing (`pair`, and the first `bluetoothctl pair` for
OS bonding), put the cuff into pairing mode: **hold the Bluetooth button until
`-P-` blinks on the display.** The radio stays continuously on while `-P-` is
showing (~30s); outside of pairing mode the cuff sleeps within ~1s of
finishing a measurement, which is why most workflows have you take a fresh
reading and immediately run the CLI.

## Linux / BlueZ note

Both `read-bps` and `sync` require an **OS-level bond** because the BLE-SIG BP
characteristics are encryption-required. The cuff invalidates its stored
bond on every power-cycle, so re-pair via `bluetoothctl` before each session.
A common pattern:

```sh
# In one shell, keep a bluetoothctl agent alive:
echo -e 'agent NoInputNoOutput\ndefault-agent\nscan on\n' | bluetoothctl

# In another shell, after pressing -P- on the cuff:
bluetoothctl pair <addr>
omron sync <addr>
```

`bluez_retry_connector` -style robust connection handling isn't implemented
yet, so flaky cuffs may need a couple of retries.

## Supported devices

The Omron memory-protocol path has 18 canonical device profiles and ~180
catalog aliases ported verbatim from upstream — run `omron list-models` to
see the full list. The exact bit-packed record layouts, EEPROM addresses,
time-sync formats, and pairing-mode quirks are all carried over, and the
decoders are byte-for-byte verified against the upstream Python reference
(see *Testing* below).

The BPS / RACP path works with **any** cuff that implements the BLE-SIG Blood
Pressure Service. Verified end-to-end on a real **Omron Complete (BP7900)**,
which is *not* in the upstream HEM-\* catalog.

## Testing

```sh
cargo test
# 44 tests pass:
#   record parsers — 5 tests, byte-equal to Python on shared inputs
#   EEPROM time sync — 6 tests, byte-equal to Python encoders + a round-trip
#   pairing wire bytes — 7 tests, byte-equal to Python
#   device registry — 5 tests
#   bit-slicing helpers — 1 test
#   BPS decoder — 9 tests, against the BLE GATT spec
#   RACP module — 10 tests, against the GATT spec
#   misc — 1 test
```

The decoder modules (`record_parsers`, `bps`, `racp`, `pairing`,
`time_sync`) are pure-logic and fully unit-tested. The BLE transport layer
needs hardware to exercise.

## Known limitations

- **No OS-level pairing agent.** `omron pair` does the Omron app-level
  handshake but relies on the user to bond via `bluetoothctl` separately.
  Implementing an in-process BlueZ agent would let `omron pair` drive the
  whole flow in one shot.
- **Omron's "legacy probe"** (0x02+zeros on the unlock characteristic) is
  deliberately disabled — on the cuffs we tested it put the device into
  key-programming mode mid-unlock and broke the memory session. See the
  `unlock()` comment in `src/transport.rs` for the full story.
- **OS-bonding-only profiles** (modern stack: `HEM-7142T2`, `HEM-7380T1`,
  …) skip the in-process pair step entirely and rely on the OS bond. On
  Linux that means `bluetoothctl pair <addr>` first.
- **Robust reconnect logic** (the upstream Python integration uses
  `bleak_retry_connector` for this) isn't ported. Single-shot operations
  generally work; long-lived polling will need it.

## License

MIT. Upstream hass-omron is also MIT.

## Acknowledgements

The Omron memory-protocol logic, device catalog, and EEPROM time-sync layouts
are ports of [eigger/hass-omron][upstream], itself derived from
[userx14/omblepy](https://github.com/userx14/omblepy). All the credit for
reverse-engineering Omron's protocol belongs there.
