//! Canonical Omron BLE device profile catalog ported from
//! `omron_ble/device_catalog.py`.
//!
//! Eighteen canonical profiles plus their equivalent-model aliases. Profiles
//! describe each device's BLE topology and on-device EEPROM layout — see
//! `DeviceConfig` for what each field means.

use once_cell::sync::Lazy;
use std::collections::HashMap;

use crate::device_config::{
    DeviceConfig, DeviceModelVariant, IndexPointerLayout, IndexUser, TimeSyncLayout,
};
use crate::record_parsers::{Endian, RecordParser};

fn iu(write_cursor_offset: usize, unread_counter_offset: usize, max: i32) -> IndexUser {
    IndexUser {
        write_cursor_offset,
        unread_counter_offset,
        write_cursor_mask: 0xFF,
        slot_index_min: 0,
        slot_index_max: max,
        slot_index_bias: -1,
    }
}

fn variants_verified(ids: &[&'static str]) -> Vec<DeviceModelVariant> {
    ids.iter().copied().map(DeviceModelVariant::new).collect()
}

fn variants_mixed(
    verified: &[&'static str],
    unverified: &[&'static str],
) -> Vec<DeviceModelVariant> {
    let mut v: Vec<DeviceModelVariant> = verified.iter().copied().map(DeviceModelVariant::new).collect();
    v.extend(unverified.iter().copied().map(DeviceModelVariant::unverified));
    v
}

pub static CANONICAL_DEVICE_PROFILES: Lazy<HashMap<&'static str, DeviceConfig>> = Lazy::new(|| {
    let mut m: HashMap<&'static str, DeviceConfig> = HashMap::new();

    m.insert(
        "HEM-6320T",
        DeviceConfig::new("HEM-6320T")
            .endian(Endian::Big)
            .users(&[0x0370], &[100])
            .record_layout(0x0E, 0x38)
            .settings(0x0F74, 0x0F9A)
            .unread_counter([0x00, 0x08])
            .time_sync([0x14, 0x1E], TimeSyncLayout::Linear10)
            .index_layout(IndexPointerLayout::new(0x08, Endian::Big, vec![iu(0x00, 0x04, 99)]))
            .parser(RecordParser::ClassicVital14)
            .variants(variants_verified(&["HEM-6320T-Z"])),
    );

    m.insert(
        "HEM-6321T",
        DeviceConfig::new("HEM-6321T")
            .endian(Endian::Big)
            .users(&[0x0370, 0x08E8], &[100, 100])
            .record_layout(0x0E, 0x38)
            .settings(0x0F74, 0x0F9A)
            .unread_counter([0x00, 0x08])
            .time_sync([0x14, 0x1E], TimeSyncLayout::Linear10)
            .index_layout(IndexPointerLayout::new(
                0x08,
                Endian::Big,
                vec![iu(0x00, 0x04, 99), iu(0x02, 0x06, 99)],
            ))
            .parser(RecordParser::ClassicVital14)
            .variants(variants_verified(&["HEM-6321T-Z"])),
    );

    m.insert(
        "HEM-6401T",
        DeviceConfig::new("HEM-6401T")
            .endian(Endian::Little)
            .users(&[0x1350], &[100])
            .record_layout(0x10, 0x10)
            .settings(0x0100, 0x0160)
            .time_sync([0x10, 0x20], TimeSyncLayout::Hem6401Prefix)
            .index_layout(IndexPointerLayout::new(
                0x10,
                Endian::Big,
                vec![IndexUser {
                    write_cursor_offset: 0x06,
                    unread_counter_offset: 0x0E,
                    write_cursor_mask: 0xFFFF,
                    slot_index_min: 0,
                    slot_index_max: 99,
                    slot_index_bias: 0,
                }],
            ))
            .parser(RecordParser::ClassicVital16Hem6401Family)
            .variants(vec![
                DeviceModelVariant::unverified_reason("HEM-6401T-Z", "fallback"),
                DeviceModelVariant::unverified("HEM-6402T-Z"),
                DeviceModelVariant::unverified("HEM-6410T-Z"),
            ]),
    );

    m.insert(
        "HEM-7320T",
        DeviceConfig::new("HEM-7320T")
            .endian(Endian::Big)
            .users(&[0x02AC, 0x05F4], &[60, 60])
            .record_layout(0x0E, 0x38)
            .settings(0x0260, 0x0286)
            .unread_counter([0x00, 0x08])
            .time_sync([0x14, 0x1E], TimeSyncLayout::Linear10)
            .index_layout(IndexPointerLayout::new(
                0x08,
                Endian::Big,
                vec![iu(0x00, 0x04, 59), iu(0x02, 0x06, 59)],
            ))
            .parser(RecordParser::ClassicVital14)
            .variants(variants_mixed(
                &[
                    "HEM-7320T-CA",
                    "HEM-7320T-CACS",
                    "HEM-7320T-ZV",
                    "HEM-7320T_TI-CA",
                    "HEM-7320T_TI-Z",
                ],
                &["HEM-8725T-WM"],
            )),
    );

    m.insert(
        "HEM-7322T",
        DeviceConfig::new("HEM-7322T")
            .legacy_pairing()
            .endian(Endian::Big)
            .users(&[0x02AC, 0x0824], &[100, 100])
            .record_layout(0x0E, 0x38)
            .settings(0x0260, 0x0286)
            .unread_counter([0x00, 0x08])
            .time_sync([0x14, 0x1E], TimeSyncLayout::ClassicMixed)
            .index_layout(IndexPointerLayout::new(
                0x08,
                Endian::Big,
                vec![iu(0x00, 0x04, 99), iu(0x02, 0x06, 99)],
            ))
            .parser(RecordParser::ClassicVital14Hem7322Family)
            .variants(variants_mixed(
                &[
                    "HEM-7321T-CA",
                    "HEM-7321T_TI-CA",
                    "HEM-7321T_TI-Z",
                    "HEM-7280T-AP",
                    "HEM-7280T-E",
                    "HEM-7280T_TI-D",
                    "HEM-7280T_TI-E",
                    "HEM-7281T",
                    "HEM-7282T",
                    "HEM-7321T-ZV",
                    "HEM-7322T-D",
                    "HEM-7322T-E",
                ],
                &["HEM-7511T", "HEM-8732K-SH", "HEM-8732T-SH"],
            )),
    );

    m.insert(
        "HEM-7600T",
        DeviceConfig::new("HEM-7600T")
            .legacy_pairing()
            .endian(Endian::Big)
            .users(&[0x02AC], &[100])
            .record_layout(0x0E, 0x38)
            .settings(0x0260, 0x0286)
            .unread_counter([0x00, 0x08])
            .time_sync([0x14, 0x1E], TimeSyncLayout::Linear10)
            .index_layout(IndexPointerLayout::new(0x08, Endian::Big, vec![iu(0x00, 0x04, 99)]))
            .parser(RecordParser::ClassicVital14)
            .variants(variants_mixed(
                &[
                    "HEM-7270C",
                    "HEM-7271T",
                    "HEM-7325T",
                    "HEM-7600T",
                    "HEM-7600T-E",
                    "HEM-7600T-Z",
                    "HEM-7600T-SH3BK",
                    "HEM-7600T2-JF",
                    "HEM-7600T_W",
                    "HEM-7600T_W-SH3W",
                    "HEM-7600T_W-Z",
                ],
                &[
                    "HEM-7600T-ZCD6BK",
                    "HEM-9601T-J3",
                    "HEM-9601T2-BR3",
                    "HEM-9601T_E3",
                    "HEM-9700T",
                ],
            )),
    );

    m.insert(
        "HEM-6232T",
        DeviceConfig::new("HEM-6232T")
            .legacy_pairing()
            .endian(Endian::Big)
            .users(&[0x02E8, 0x0860], &[100, 100])
            .record_layout(0x0E, 0x38)
            .settings(0x0260, 0x02A4)
            .unread_counter([0x00, 0x08])
            .time_sync([0x2C, 0x3C], TimeSyncLayout::ClassicOffset8)
            .index_layout(IndexPointerLayout::new(
                0x10,
                Endian::Big,
                vec![iu(0x00, 0x04, 99), iu(0x02, 0x06, 99)],
            ))
            .parser(RecordParser::ClassicVital14Hem6232Family)
            .variants(variants_mixed(
                &["HEM-6232T-AP", "HEM-6232T-E", "HEM-6233T"],
                &[
                    "HEM-1026T2-AJC",
                    "HEM-1026T2-AJE",
                    "HEM-1026T2-AKA",
                    "HEM-6232T-D",
                    "HEM-6232T-Z",
                    "HEM-6320T-SH",
                    "HEM-6322T-SH",
                    "HEM-6323T",
                    "HEM-6324T",
                    "HEM-6325T",
                ],
            )),
    );

    m.insert(
        "HEM-7530T",
        DeviceConfig::new("HEM-7530T")
            .legacy_pairing()
            .endian(Endian::Big)
            .users(&[0x02E8], &[90])
            .record_layout(0x0E, 0x10)
            .settings(0x0260, 0x02A4)
            .time_sync([0x2C, 0x3C], TimeSyncLayout::ModernOffset8)
            .index_layout(IndexPointerLayout::new(0x10, Endian::Big, vec![iu(0x00, 0x04, 89)]))
            .parser(RecordParser::ClassicVital14)
            .variants(variants_mixed(
                &[
                    "HEM-6231T2-JC",
                    "HEM-6231T2-JE",
                    "HEM-6231T2-JT3",
                    "HEM-7271P-SH3",
                    "HEM-7271T_SH3",
                    "HEM-7530T1-BR3",
                    "HEM-7530T_AP3",
                    "HEM-7530T_E3",
                    "HEM-7530T_J3",
                    "HEM-7530T_JT3",
                    "HEM-8630T-SH",
                ],
                &[
                    "HEM-6161T-E",
                    "HEM-6161T-RU",
                    "HEM-6161T2-BR",
                    "HEM-6231T-SH",
                    "HEM-6231T_Z",
                    "HEM-7136T-SH3",
                    "HEM-7138JT-SH",
                    "HEM-7138T-SH",
                    "HEM-7139T-SH3",
                    "HEM-7143T1-AIN",
                    "HEM-7143T1-AP",
                    "HEM-7143T1-D",
                    "HEM-7143T1-E",
                    "HEM-7143T1_D",
                    "HEM-7143T1_EBK",
                    "HEM-7143T2-E",
                    "HEM-7143T2_ESL",
                    "HEM-7144T1-AU",
                    "HEM-7144T2-BR",
                    "HEM-7144T2-LA",
                    "HEM-716DT2-LA",
                    "HEM-7271L-SH3",
                    "HEM-7530T-Z",
                ],
            )),
    );

    m.insert(
        "HEM-7150T",
        DeviceConfig::new("HEM-7150T")
            .legacy_pairing()
            .endian(Endian::Little)
            .users(&[0x0098], &[60])
            .record_layout(0x10, 0x10)
            .settings(0x0010, 0x0054)
            .unread_counter([0x00, 0x10])
            .time_sync([0x2C, 0x3C], TimeSyncLayout::ModernOffset8)
            .index_layout(IndexPointerLayout::new(0x10, Endian::Little, vec![iu(0x00, 0x04, 59)]))
            .parser(RecordParser::ClassicVital14)
            .variants(variants_mixed(
                &[
                    "HEM-7150T-Z",
                    "HEM-7153JT_ASH",
                    "HEM-7153T_ASH",
                    "HEM-7156T-BR",
                    "HEM-7156T-LA",
                    "HEM-7156T_AAP",
                    "HEM-7156T_AP",
                ],
                &[
                    "HEM-7150T-CA",
                    "HEM-7157T-AP",
                    "HEM-7158T-JC",
                    "HEM-7158T_AP3",
                ],
            )),
    );

    m.insert(
        "HEM-7151T",
        DeviceConfig::new("HEM-7151T")
            .legacy_pairing()
            .endian(Endian::Little)
            .users(&[0x0098], &[80])
            .record_layout(0x10, 0x10)
            .settings(0x0010, 0x0054)
            .unread_counter([0x00, 0x10])
            .time_sync([0x2C, 0x3C], TimeSyncLayout::ModernOffset8)
            .index_layout(IndexPointerLayout::new(0x10, Endian::Little, vec![iu(0x00, 0x04, 79)]))
            .parser(RecordParser::ClassicVital14)
            .variants(vec![DeviceModelVariant::unverified("HEM-7151T-Z")]),
    );

    m.insert(
        "HEM-7155T",
        DeviceConfig::new("HEM-7155T")
            .legacy_pairing()
            .endian(Endian::Little)
            .users(&[0x0098, 0x0458], &[60, 60])
            .record_layout(0x10, 0x10)
            .settings(0x0010, 0x0054)
            .unread_counter([0x00, 0x10])
            .time_sync([0x2C, 0x3C], TimeSyncLayout::ModernOffset8)
            .index_layout(IndexPointerLayout::new(
                0x10,
                Endian::Little,
                vec![iu(0x00, 0x04, 59), iu(0x02, 0x06, 59)],
            ))
            .parser(RecordParser::ClassicVital14)
            .variants(variants_mixed(
                &[
                    "HEM-7155T-ALRU",
                    "HEM-7155T-D",
                    "HEM-7155T-EBK",
                    "HEM-7155T_AP",
                    "HEM-7155T_ASH3BK",
                    "HEM-7155T_ASH3SL",
                    "HEM-7155T_ESL",
                    "HEM-7340T-Z",
                    "HEM-7341T-Z",
                ],
                &[
                    "HEM-7155T_K4-D",
                    "HEM-7155T_K4-EBK",
                    "HEM-7155T_K4-ESL",
                    "HEM-7340T-CA",
                    "HEM-7340T_K4-CA",
                    "HEM-7340T_K4-Z",
                    "HEM-7341T_K4-Z",
                ],
            )),
    );

    m.insert(
        "HEM-7155T-MW",
        DeviceConfig::new("HEM-7155T-MW")
            .modern_stack_os_bonding()
            .endian(Endian::Little)
            .users(&[0x0098, 0x0458], &[60, 60])
            .record_layout(0x10, 0x38)
            .settings(0x0010, 0x0054)
            .index_layout(IndexPointerLayout::new(
                0x10,
                Endian::Little,
                vec![iu(0x00, 0x04, 59), iu(0x02, 0x06, 59)],
            ))
            .parser(RecordParser::ClassicVital14)
            .prefer_slot_index(),
    );

    m.insert(
        "HEM-7155T-MW3",
        DeviceConfig::new("HEM-7155T-MW3")
            .modern_stack_os_bonding()
            .endian(Endian::Little)
            .users(&[0x02E8, 0x06A8], &[60, 60])
            .record_layout(0x10, 0x38)
            .settings(0x0260, 0x02A4)
            .index_layout(IndexPointerLayout::new(
                0x10,
                Endian::Little,
                vec![iu(0x00, 0x04, 59), iu(0x02, 0x06, 59)],
            ))
            .parser(RecordParser::ClassicVital14)
            .prefer_slot_index(),
    );

    m.insert(
        "HEM-7146T",
        DeviceConfig::new("HEM-7146T")
            .modern_stack_os_bonding()
            .endian(Endian::Little)
            .users(&[0x02E8], &[30])
            .record_layout(0x0E, 0x38)
            .settings(0x0260, 0x02A4)
            .time_sync([0x2C, 0x3C], TimeSyncLayout::ModernOffset8)
            .index_layout(IndexPointerLayout::new(0x10, Endian::Little, vec![iu(0x00, 0x04, 29)]))
            .parser(RecordParser::ClassicVital14)
            .prefer_slot_index()
            .variants(vec![
                DeviceModelVariant::unverified("HEM-7146T2-EBK"),
                DeviceModelVariant::unverified("HEM-7146T2-ESL"),
                DeviceModelVariant::unverified("HEM-7146T2-JD"),
                DeviceModelVariant::unverified("HEM-7146T2-JF"),
            ]),
    );

    m.insert(
        "HEM-7342T",
        DeviceConfig::new("HEM-7342T")
            .legacy_pairing()
            .endian(Endian::Little)
            .users(&[0x0098, 0x06D8], &[100, 100])
            .record_layout(0x10, 0x10)
            .settings(0x0010, 0x0054)
            .unread_counter([0x00, 0x10])
            .time_sync([0x2C, 0x3C], TimeSyncLayout::ModernOffset8)
            .index_layout(IndexPointerLayout::new(
                0x10,
                Endian::Little,
                vec![iu(0x00, 0x04, 99), iu(0x02, 0x06, 99)],
            ))
            .parser(RecordParser::ClassicVital14)
            .variants(variants_mixed(
                &[
                    "HEM-7159T_AP3",
                    "HEM-7342T-Z",
                    "HEM-7343T-Z",
                    "HEM-7344JT_ASH3",
                    "HEM-7344T_ASH3BK",
                    "HEM-7344T_ASH3SL",
                    "HEM-7346T-AJC3",
                    "HEM-7346T-AJE3",
                    "HEM-7346T2-AJC32",
                    "HEM-7346T2-AJE32",
                    "HEM-7346T_ABR3",
                    "HEM-7346T_AP3",
                    "HEM-7347T-AJC3",
                    "HEM-7347T-AJE3",
                    "HEM-7347T2-AJC32",
                    "HEM-7347T2-AJE32",
                    "HEM-7349T_ABR",
                    "HEM-7361T-ALRU",
                    "HEM-7361T-AP",
                    "HEM-7361T-D",
                    "HEM-7361T-EBK",
                    "HEM-7361T_ESL",
                ],
                &["HEM-7342T-CA", "HEM-7342T1-ACACD6", "HEM-7361T1-BS"],
            )),
    );

    m.insert(
        "HEM-7361T",
        DeviceConfig::new("HEM-7361T")
            .legacy_pairing()
            .endian(Endian::Little)
            .users(&[0x0098, 0x06D8], &[100, 100])
            .record_layout(0x10, 0x10)
            .settings(0x0010, 0x0054)
            .unread_counter([0x00, 0x10])
            .time_sync([0x2C, 0x3C], TimeSyncLayout::ModernOffset8)
            .index_layout(IndexPointerLayout::new(
                0x10,
                Endian::Little,
                vec![iu(0x00, 0x04, 99), iu(0x02, 0x06, 99)],
            ))
            .parser(RecordParser::ClassicVital14),
    );

    m.insert(
        "HEM-7380T1",
        DeviceConfig::new("HEM-7380T1")
            .modern_stack_os_bonding()
            .endian(Endian::Little)
            .users(&[0x01C4, 0x0804], &[100, 100])
            .record_layout(0x10, 0x38)
            .settings(0x0010, 0x0054)
            .time_sync([0x2C, 0x3C], TimeSyncLayout::ModernOffset8)
            .index_layout(IndexPointerLayout::new(
                0x18,
                Endian::Little,
                vec![iu(0x00, 0x04, 99), iu(0x02, 0x06, 99)],
            ))
            .parser(RecordParser::ClassicVital14)
            .prefer_slot_index()
            .variants(variants_mixed(
                &[
                    "HEM-7194T1-FLAP",
                    "HEM-7196T1-FLE",
                    "HEM-7380T",
                    "HEM-7380T1-EBK",
                    "HEM-7382T1",
                    "HEM-7383T1-AP",
                    "HEM-7384T1-NBBR",
                ],
                &[
                    "HEM-7183T1-AP",
                    "HEM-7183T1-CAP",
                    "HEM-7183T1_FLBIN",
                    "HEM-7183T1_FLIN",
                    "HEM-7183T1_LAP",
                    "HEM-7188T1-LE",
                    "HEM-7188T1-LEO",
                    "HEM-7194T1-FLCAP",
                    "HEM-7194T1_FLBIN",
                    "HEM-7194T1_FLIN",
                    "HEM-7196T1-FLEO",
                    "HEM-7376T1-ACACD6",
                    "HEM-7376T1-Z",
                    "HEM-7377T1-ZAZ",
                    "HEM-7380T1-EOSL",
                    "HEM-7381T1-AZ",
                    "HEM-7382T1-AZAZ",
                    "HEM-7385T1-AJAZ3",
                    "HEM-7386T1-AJF3",
                    "HEM-7387T1-AJAZ3",
                    "HEM-7388T1-AJF3",
                    "HEM-7389T1-JM3",
                ],
            )),
    );

    m.insert(
        "HEM-7142T2",
        DeviceConfig::new("HEM-7142T2")
            .modern_stack_os_bonding()
            .endian(Endian::Little)
            .users(&[0x02E8], &[14])
            .record_layout(0x0E, 0x38)
            .settings(0x0260, 0x02A4)
            .time_sync([0x2C, 0x3C], TimeSyncLayout::ModernOffset8)
            .index_layout({
                let mut l = IndexPointerLayout::new(
                    0x10,
                    Endian::Big,
                    vec![IndexUser {
                        write_cursor_offset: 0x00,
                        unread_counter_offset: 0x04,
                        write_cursor_mask: 0xFF,
                        slot_index_min: 0,
                        slot_index_max: 13,
                        slot_index_bias: -1,
                    }],
                );
                l.record_addresses = Some(vec![0x02E8]);
                l.record_byte_size = Some(0x0E);
                l.record_step = Some(0x0E);
                l.backtrack_slots = 13;
                l.collect_all_valid_in_index_window = true;
                l.skip_full_scan_fallback_when_index_empty = true;
                l
            })
            .parser(RecordParser::ClassicVital14)
            .prefer_slot_index()
            .variants(variants_mixed(
                &[
                    "HEM-7138K-SH",
                    "HEM-7140T1-AP",
                    "HEM-7141T1-AP",
                    "HEM-7142T1-AP",
                    "HEM-7142T2-AP",
                    "HEM-7142T2_JAZ",
                ],
                &[
                    "HEM-7142T2-Z",
                    "HEM-7142T2-ZAZ",
                    "HEM-716BT2-ZAZ",
                    "HEM-716CT2-Z",
                ],
            )),
    );

    m
});
