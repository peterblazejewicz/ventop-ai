// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use crate::file_cache::{read_cached_file, read_cached_u32, resolve_sysfs_path};
use std::fs;

/// Qualcomm Adreno GPU telemetry
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct AdrenoGpuMetrics {
    pub cur_freq_mhz: Option<u32>,
    pub governor: Option<String>,
    pub available_frequencies_mhz: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duty_cycle_pct: Option<f32>,
}

/// Read Adreno GPU metrics dynamically from /sys/class/devfreq
pub fn get_adreno_gpu_metrics() -> AdrenoGpuMetrics {
    let devfreq_dir = resolve_sysfs_path("/sys/class/devfreq");
    let mut gpu_devfreq_path = None;

    if let Ok(entries) = fs::read_dir(&devfreq_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let name_lower = name.to_lowercase();
            if name_lower.ends_with(".gpu")
                || name_lower.contains("gpu")
                || name_lower.contains("kgsl")
                || name_lower.contains("adreno")
            {
                gpu_devfreq_path = Some(entry.path());
                break;
            }
        }
    }

    let devfreq_base = match gpu_devfreq_path {
        Some(p) => p,
        None => {
            return AdrenoGpuMetrics {
                cur_freq_mhz: None,
                governor: None,
                available_frequencies_mhz: Vec::new(),
                duty_cycle_pct: None,
            };
        }
    };

    let base_str = devfreq_base.to_string_lossy();
    let cur_freq_mhz = read_cached_u32(&format!("{}/cur_freq", base_str)).map(|hz| hz / 1_000_000);

    let governor = read_cached_file(&format!("{}/governor", base_str))
        .ok()
        .map(|s| s.trim().to_string());

    let mut available_frequencies_mhz = Vec::new();
    if let Ok(content) = fs::read_to_string(format!("{}/available_frequencies", base_str)) {
        for word in content.split_whitespace() {
            if let Ok(hz) = word.parse::<u64>() {
                available_frequencies_mhz.push((hz / 1_000_000) as u32);
            }
        }
        available_frequencies_mhz.sort_unstable();
    }

    AdrenoGpuMetrics {
        cur_freq_mhz,
        governor,
        available_frequencies_mhz,
        duty_cycle_pct: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_cache::EnvGuard;

    #[test]
    fn test_dynamic_gpu_devfreq_discovery() {
        let temp_dir = std::env::temp_dir().join(format!("ventop_hw_gpu_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let gpu_node = temp_dir.join("sys/class/devfreq/kgsl-3d0.gpu");
        std::fs::create_dir_all(&gpu_node).unwrap();

        std::fs::write(gpu_node.join("cur_freq"), "623000000\n").unwrap();
        std::fs::write(gpu_node.join("governor"), "msm-adreno-tz\n").unwrap();
        std::fs::write(
            gpu_node.join("available_frequencies"),
            "315000000 623000000 877000000\n",
        )
        .unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let metrics = get_adreno_gpu_metrics();
        assert_eq!(metrics.cur_freq_mhz, Some(623));
        assert_eq!(metrics.governor.as_deref(), Some("msm-adreno-tz"));
        assert_eq!(metrics.available_frequencies_mhz, vec![315, 623, 877]);

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_dynamic_gpu_devfreq_missing() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hw_gpu_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let metrics = get_adreno_gpu_metrics();
        assert_eq!(metrics.cur_freq_mhz, None);
        assert_eq!(metrics.governor, None);
        assert!(metrics.available_frequencies_mhz.is_empty());

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
