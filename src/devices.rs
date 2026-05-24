//! Device model registry — `get_device_config`, variant resolution, and
//! BLE-local-name parsing. Ported from `omron_ble/devices.py`.

use once_cell::sync::Lazy;
use std::collections::{BTreeSet, HashMap};

use crate::consts::DEFAULT_DEVICE_MODEL;
use crate::device_catalog::CANONICAL_DEVICE_PROFILES;
use crate::device_config::{DeviceConfig, DeviceModelVariant};

/// Alternate model id → (canonical profile key, variant metadata).
pub static MODEL_VARIANT_MAP: Lazy<HashMap<&'static str, (&'static str, DeviceModelVariant)>> =
    Lazy::new(|| {
        let mut idx = HashMap::new();
        for (canonical, profile) in CANONICAL_DEVICE_PROFILES.iter() {
            for variant in &profile.equivalent_model_ids {
                if idx.contains_key(variant.model_id) {
                    panic!("Duplicate catalog model variant: {}", variant.model_id);
                }
                idx.insert(variant.model_id, (*canonical, variant.clone()));
            }
        }
        idx
    });

/// Resolve a model id (canonical or alias) to a fully populated `DeviceConfig`.
///
/// Unknown ids fall back to the [`DEFAULT_DEVICE_MODEL`] profile, mirroring the
/// Python behaviour. The returned config has `.model` set to the *requested*
/// id, not the canonical key, so downstream logging matches the user's input.
pub fn get_device_config(model: &str) -> DeviceConfig {
    if let Some(profile) = CANONICAL_DEVICE_PROFILES.get(model) {
        return profile.clone();
    }
    if let Some((canonical, _)) = MODEL_VARIANT_MAP.get(model) {
        let mut cfg = CANONICAL_DEVICE_PROFILES[canonical].clone();
        cfg.model = model.to_string();
        return cfg;
    }
    tracing::warn!(
        "Unknown device model '{}', falling back to {}",
        model,
        DEFAULT_DEVICE_MODEL
    );
    let mut cfg = CANONICAL_DEVICE_PROFILES[DEFAULT_DEVICE_MODEL].clone();
    if cfg.model != model {
        cfg.model = model.to_string();
    }
    cfg
}

pub fn supported_models() -> Vec<&'static str> {
    let mut set: BTreeSet<&'static str> = BTreeSet::new();
    for k in CANONICAL_DEVICE_PROFILES.keys() {
        set.insert(*k);
    }
    for k in MODEL_VARIANT_MAP.keys() {
        set.insert(*k);
    }
    set.into_iter().collect()
}

/// Extract a `HEM-…` model code from a BLE advertisement local name, if one
/// matches a known catalog entry.
pub fn infer_model_id_from_local_name(local_name: &str) -> Option<String> {
    let trimmed = local_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Capture the longest HEM-* sequence in the name.
    let mut start = None;
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4].eq_ignore_ascii_case(b"HEM-") {
            start = Some(i);
            break;
        }
        i += 1;
    }
    let start = start?;
    let mut end = start + 4;
    while end < bytes.len() {
        let c = bytes[end];
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'-' {
            end += 1;
        } else {
            break;
        }
    }
    let token = &trimmed[start..end];
    let upper = token.to_ascii_uppercase();
    let candidates = [
        token.to_string(),
        upper.clone(),
        token.replace(' ', ""),
        upper.replace(' ', ""),
    ];
    let supported: BTreeSet<&'static str> = CANONICAL_DEVICE_PROFILES
        .keys()
        .copied()
        .chain(MODEL_VARIANT_MAP.keys().copied())
        .collect();
    for c in candidates {
        if let Some(&hit) = supported.iter().find(|k| **k == c) {
            return Some(hit.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_id_returns_its_own_profile() {
        let cfg = get_device_config("HEM-7142T2");
        assert_eq!(cfg.model, "HEM-7142T2");
        assert!(cfg.supports_os_bonding_only);
        assert!(!cfg.requires_unlock);
    }

    #[test]
    fn alias_resolves_to_parent_profile_with_alias_model_field() {
        let cfg = get_device_config("HEM-7280T-AP"); // alias of HEM-7322T
        assert_eq!(cfg.model, "HEM-7280T-AP");
        assert!(cfg.legacy_pairing_workarounds);
        assert_eq!(cfg.user_start_addresses, vec![0x02AC, 0x0824]);
    }

    #[test]
    fn unknown_model_falls_back_to_default_with_requested_model_field() {
        let cfg = get_device_config("NOT-A-REAL-MODEL");
        assert_eq!(cfg.model, "NOT-A-REAL-MODEL");
        // default is HEM-7142T2 — which is OS-bonding only.
        assert!(cfg.supports_os_bonding_only);
    }

    #[test]
    fn infer_model_id_from_local_name_handles_real_advertisements() {
        assert_eq!(
            infer_model_id_from_local_name("Omron BLE HEM-7322T-D"),
            Some("HEM-7322T-D".to_string())
        );
        assert_eq!(infer_model_id_from_local_name("HEM-7600T"), Some("HEM-7600T".to_string()));
        // alias should resolve
        assert_eq!(
            infer_model_id_from_local_name("HEM-7142T2-AP"),
            Some("HEM-7142T2-AP".to_string())
        );
        // not in catalog
        assert_eq!(infer_model_id_from_local_name("Something-Else"), None);
        assert_eq!(infer_model_id_from_local_name(""), None);
    }

    #[test]
    fn supported_models_includes_all_canonical_and_variants() {
        let models = supported_models();
        assert!(models.contains(&"HEM-7142T2"));
        assert!(models.contains(&"HEM-7322T"));
        assert!(models.contains(&"HEM-7280T-AP")); // variant
        assert!(models.contains(&"HEM-6320T-Z")); // variant
        // 18 canonical + ~184 aliases
        assert!(models.len() >= 100);
    }
}
