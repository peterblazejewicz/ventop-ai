// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use crate::file_cache::{read_cached_file, read_cached_i32, read_cached_u32, resolve_sysfs_path};
use std::fs;

/// Multi-zone on-die silicon thermals for the Hexagon Neural Signal Processor (NSP)
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct NspSiliconThermals {
    pub core_vector_celsius: Option<i32>, // tz5, tz15 (nsp-0-0-*): Scalar threads & HVX units
    pub matrix_hmx_celsius: Option<i32>,  // tz6, tz18 (nsp-0-1-*): HMX Matrix Tensor engine
    pub vtcm_sram_celsius: Option<i32>,   // tz7, tz20 (nsp-0-2-*): VTCM on-chip memory
    pub peak_celsius: Option<i32>,
}

/// Read CDSP cooling/throttling state dynamically from /sys/class/thermal/cooling_device*
pub fn get_cdsp_throttle_state() -> Option<u32> {
    let thermal_dir = resolve_sysfs_path("/sys/class/thermal");
    if let Ok(entries) = fs::read_dir(thermal_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("cooling_device") {
                continue;
            }

            let type_path = entry.path().join("type");
            if let Ok(t) = read_cached_file(&type_path.to_string_lossy()) {
                let trimmed = t.trim().to_lowercase();
                if trimmed == "cdsp:cdsp_sw"
                    || trimmed == "cdsp_sw"
                    || trimmed == "cdsp"
                    || trimmed.contains("cdsp")
                {
                    let state_path = entry.path().join("cur_state");
                    return read_cached_u32(&state_path.to_string_lossy());
                }
            }
        }
    }
    None
}

/// Read multi-zone silicon temperatures dynamically for the NSP cluster
pub fn get_nsp_silicon_thermals() -> NspSiliconThermals {
    let thermal_dir = resolve_sysfs_path("/sys/class/thermal");
    let mut core_max: Option<i32> = None;
    let mut matrix_max: Option<i32> = None;
    let mut vtcm_max: Option<i32> = None;
    let mut peak: Option<i32> = None;

    if let Ok(entries) = fs::read_dir(&thermal_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("thermal_zone") {
                continue;
            }

            let type_path = entry.path().join("type");
            let temp_path = entry.path().join("temp");

            let type_str = if let Ok(t) = read_cached_file(&type_path.to_string_lossy()) {
                t.trim().to_string()
            } else {
                continue;
            };

            let temp_c = if let Some(milli) = read_cached_i32(&temp_path.to_string_lossy()) {
                milli / 1000
            } else {
                continue;
            };

            let type_lower = type_str.to_lowercase();
            let is_nsp = type_lower.starts_with("nsp-")
                || type_lower.contains("cdsp")
                || type_lower.contains("npu");

            if !is_nsp {
                continue;
            }

            // Track overall peak across all NSP silicon sensors
            peak = Some(peak.map_or(temp_c, |p| p.max(temp_c)));

            // Sub-domain classification:
            // 1. Core / Vector engines (HVX / scalar): contains "core" or "vector", or "nsp-0-0", "nsp-0-1"
            if type_lower.contains("core")
                || type_lower.contains("vector")
                || type_str.contains("nsp-0-0")
                || type_str.contains("nsp-0-1")
            {
                core_max = Some(core_max.map_or(temp_c, |m| m.max(temp_c)));
            } else if type_lower.contains("matrix")
                || type_lower.contains("hmx")
                || type_str.contains("nsp-0-2")
                || type_str.contains("nsp-1-0")
            {
                matrix_max = Some(matrix_max.map_or(temp_c, |m| m.max(temp_c)));
            } else if type_lower.contains("vtcm")
                || type_lower.contains("sram")
                || type_str.contains("nsp-1-1")
            {
                vtcm_max = Some(vtcm_max.map_or(temp_c, |m| m.max(temp_c)));
            }
        }
    }

    NspSiliconThermals {
        core_vector_celsius: core_max,
        matrix_hmx_celsius: matrix_max,
        vtcm_sram_celsius: vtcm_max,
        peak_celsius: peak,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_cache::EnvGuard;

    #[test]
    fn test_dynamic_nsp_silicon_thermals_renumbered() {
        let temp_dir = std::env::temp_dir().join(format!("ventop_hex_tz_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let tz12 = temp_dir.join("sys/class/thermal/thermal_zone12");
        let tz13 = temp_dir.join("sys/class/thermal/thermal_zone13");
        let tz25 = temp_dir.join("sys/class/thermal/thermal_zone25");
        let tz0 = temp_dir.join("sys/class/thermal/thermal_zone0");
        std::fs::create_dir_all(&tz12).unwrap();
        std::fs::create_dir_all(&tz13).unwrap();
        std::fs::create_dir_all(&tz25).unwrap();
        std::fs::create_dir_all(&tz0).unwrap();

        std::fs::write(tz12.join("type"), "nsp-0-0-core\n").unwrap();
        std::fs::write(tz12.join("temp"), "42000\n").unwrap();

        std::fs::write(tz13.join("type"), "nsp-0-2-matrix\n").unwrap();
        std::fs::write(tz13.join("temp"), "46000\n").unwrap();

        std::fs::write(tz25.join("type"), "nsp-1-1-vtcm\n").unwrap();
        std::fs::write(tz25.join("temp"), "41000\n").unwrap();

        std::fs::write(tz0.join("type"), "cpu-0-gold\n").unwrap();
        std::fs::write(tz0.join("temp"), "55000\n").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let thermals = get_nsp_silicon_thermals();
        assert_eq!(thermals.core_vector_celsius, Some(42));
        assert_eq!(thermals.matrix_hmx_celsius, Some(46));
        assert_eq!(thermals.vtcm_sram_celsius, Some(41));
        assert_eq!(thermals.peak_celsius, Some(46));

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_dynamic_nsp_silicon_thermals_missing() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hex_tz_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let thermals = get_nsp_silicon_thermals();
        assert_eq!(thermals.core_vector_celsius, None);
        assert_eq!(thermals.matrix_hmx_celsius, None);
        assert_eq!(thermals.vtcm_sram_celsius, None);
        assert_eq!(thermals.peak_celsius, None);

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_dynamic_cdsp_cooling_device_renumbered() {
        let temp_dir = std::env::temp_dir().join(format!("ventop_hex_cool_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let cd22 = temp_dir.join("sys/class/thermal/cooling_device22");
        std::fs::create_dir_all(&cd22).unwrap();

        std::fs::write(cd22.join("type"), "cdsp:cdsp_sw\n").unwrap();
        std::fs::write(cd22.join("cur_state"), "2\n").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        assert_eq!(get_cdsp_throttle_state(), Some(2));

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_dynamic_cdsp_cooling_device_missing() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hex_cool_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        assert_eq!(get_cdsp_throttle_state(), None);

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
