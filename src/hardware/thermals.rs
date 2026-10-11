// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use crate::file_cache::{read_cached_file, read_cached_i32, resolve_sysfs_path};
use std::collections::HashMap;
use std::fs;

/// Consolidated thermal categories for Arduino Ventuno Q (condenses 48 TSENS zones)
#[derive(Debug, Clone, serde::Serialize)]
pub struct VentunoThermals {
    pub cpu_gold_celsius: Option<i32>,
    pub cpu_silver_celsius: Option<i32>,
    pub gpu_celsius: Option<i32>,
    pub npu_nsp_celsius: Option<i32>,
    pub ram_celsius: Option<i32>,
    pub pmic_board_celsius: Option<i32>,
    pub raw_zones: HashMap<String, i32>,
}

/// Filter and consolidate the 48 hardware thermal zones into 6 key operational metrics
pub fn get_consolidated_thermals() -> VentunoThermals {
    let mut raw_zones = HashMap::new();
    let mut cpu_gold_max: Option<i32> = None;
    let mut cpu_silver_max: Option<i32> = None;
    let mut gpu_max: Option<i32> = None;
    let mut npu_max: Option<i32> = None;
    let mut ram_max: Option<i32> = None;
    let mut pmic_max: Option<i32> = None;

    let thermal_dir = resolve_sysfs_path("/sys/class/thermal");
    if let Ok(entries) = fs::read_dir(thermal_dir) {
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

            raw_zones.insert(type_str.clone(), temp_c);

            // Categorization logic:
            if type_str.starts_with("cpu-0-") || type_str.starts_with("cpuss-0-") {
                // Gold / Prime A78C cores
                cpu_gold_max = Some(cpu_gold_max.map_or(temp_c, |m| m.max(temp_c)));
            } else if type_str.starts_with("cpu-1-") || type_str.starts_with("cpuss-1-") {
                // Silver A55 cores
                cpu_silver_max = Some(cpu_silver_max.map_or(temp_c, |m| m.max(temp_c)));
            } else if type_str.contains("gpu") {
                // Adreno GPU
                gpu_max = Some(gpu_max.map_or(temp_c, |m| m.max(temp_c)));
            } else if type_str.starts_with("nsp-") {
                // Hexagon NPU / Neural Signal Processor
                npu_max = Some(npu_max.map_or(temp_c, |m| m.max(temp_c)));
            } else if type_str.contains("ddrss") || type_str.contains("sdram") {
                // LPDDR5 RAM
                ram_max = Some(ram_max.map_or(temp_c, |m| m.max(temp_c)));
            } else if type_str.contains("pmm86") || type_str.contains("pmic") {
                // PMIC / Board
                pmic_max = Some(pmic_max.map_or(temp_c, |m| m.max(temp_c)));
            }
        }
    }

    VentunoThermals {
        cpu_gold_celsius: cpu_gold_max,
        cpu_silver_celsius: cpu_silver_max,
        gpu_celsius: gpu_max,
        npu_nsp_celsius: npu_max,
        ram_celsius: ram_max,
        pmic_board_celsius: pmic_max,
        raw_zones,
    }
}
