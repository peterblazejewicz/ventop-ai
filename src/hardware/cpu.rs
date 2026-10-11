// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use crate::file_cache::{read_cached_file, read_cached_u32, resolve_sysfs_path};
use std::fs;

/// Helper to parse CPU lists from sysfs (e.g. "0 1", "0-1", "4,5,6,7", "4-7")
fn parse_cpu_list(s: &str) -> Vec<u32> {
    let mut cpus = Vec::new();
    for token in s.split(|c: char| c.is_whitespace() || c == ',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some((start_s, end_s)) = token.split_once('-') {
            if let (Ok(start), Ok(end)) = (start_s.parse::<u32>(), end_s.parse::<u32>()) {
                for c in start..=end {
                    cpus.push(c);
                }
            }
        } else if let Ok(c) = token.parse::<u32>() {
            cpus.push(c);
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    cpus
}

/// CPU Cluster Topology on Qualcomm Snapdragon QCS8275
#[derive(Debug, Clone, serde::Serialize)]
pub struct CpuCluster {
    pub name: &'static str,
    pub architecture: &'static str,
    pub cores: Vec<u32>,
    pub min_freq_mhz: u32,
    pub max_freq_mhz: u32,
    pub cur_freq_mhz: u32,
    pub governor: String,
    pub load_pct: f32,
}

/// Parse CPU architecture from /proc/cpuinfo
pub fn get_cpu_architecture() -> String {
    let cpuinfo_path = resolve_sysfs_path("/proc/cpuinfo");
    if let Ok(content) = fs::read_to_string(&cpuinfo_path) {
        let mut arch = None;
        let mut parts = std::collections::HashSet::new();

        for line in content.lines() {
            if line.starts_with("CPU architecture:") {
                arch = line.split(':').nth(1).map(|s| s.trim().to_string());
            } else if line.starts_with("CPU part") {
                if let Some(part) = line.split(':').nth(1).map(|s| s.trim().to_string()) {
                    parts.insert(part);
                }
            }
        }

        if let Some(arch_val) = arch {
            let mut result = format!("ARMv{}", arch_val);
            let mut core_names = Vec::new();
            for part_val in &parts {
                let core_name = match part_val.as_str() {
                    "0xd05" => Some("A55"),
                    "0xd4b" => Some("A78C"),
                    "0xd41" => Some("A78"),
                    "0xd03" => Some("A53"),
                    "0xd08" => Some("A72"),
                    _ => None,
                };
                if let Some(name) = core_name {
                    core_names.push(name);
                }
            }

            if !core_names.is_empty() {
                core_names.sort();
                result.push_str(&format!(" ({})", core_names.join("+")));
            }
            return result;
        }
    }

    "ARMv8 (A55+A78C)".to_string()
}

/// Read per-core CPU frequencies in MHz
pub fn get_cpu_frequencies() -> Vec<u32> {
    let mut freqs = Vec::new();
    let mut num_cores = 8;
    let cpu_base = resolve_sysfs_path("/sys/devices/system/cpu");
    if let Ok(entries) = fs::read_dir(&cpu_base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix("cpu") {
                if let Ok(id) = rest.parse::<usize>() {
                    if id + 1 > num_cores && id < 32 {
                        num_cores = id + 1;
                    }
                }
            }
        }
    }

    for cpu_id in 0..num_cores {
        let path = format!(
            "/sys/devices/system/cpu/cpu{}/cpufreq/scaling_cur_freq",
            cpu_id
        );
        let freq = read_cached_u32(&path).map(|f| f / 1000).unwrap_or(0);
        freqs.push(freq);
    }

    freqs
}

/// Get CPU cluster topology, frequencies, and computed cluster load %
pub fn get_cpu_clusters_with_load(per_core_pct: &[f32]) -> Vec<CpuCluster> {
    let cpufreq_dir = resolve_sysfs_path("/sys/devices/system/cpu/cpufreq");
    let mut policy_entries = Vec::new();

    if let Ok(entries) = fs::read_dir(&cpufreq_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("policy") {
                policy_entries.push(entry.path());
            }
        }
    }

    let mut clusters = Vec::new();

    for policy_path in policy_entries {
        let mut cores = Vec::new();
        for cpus_file in &["affected_cpus", "related_cpus"] {
            let p = policy_path.join(cpus_file);
            if let Ok(content) = fs::read_to_string(&p) {
                cores = parse_cpu_list(&content);
                if !cores.is_empty() {
                    break;
                }
            }
        }

        let base_str = policy_path.to_string_lossy();
        let min_khz = read_cached_u32(&format!("{}/scaling_min_freq", base_str))
            .or_else(|| read_cached_u32(&format!("{}/cpuinfo_min_freq", base_str)))
            .unwrap_or(0);
        let max_khz = read_cached_u32(&format!("{}/scaling_max_freq", base_str))
            .or_else(|| read_cached_u32(&format!("{}/cpuinfo_max_freq", base_str)))
            .unwrap_or(0);
        let cur_khz = read_cached_u32(&format!("{}/scaling_cur_freq", base_str)).unwrap_or(min_khz);
        let governor = read_cached_file(&format!("{}/scaling_governor", base_str))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unavailable".to_string());

        let min_core = cores.first().copied().unwrap_or(0);
        let (name, arch) = if max_khz >= 2_300_000
            || (max_khz == 0
                && (cores.contains(&2) || cores.contains(&3) || (2..4).contains(&min_core)))
        {
            ("Prime Cluster (Cores 2-3)", "Cortex-A78C")
        } else if (2_050_000..2_300_000).contains(&max_khz)
            || (max_khz == 0 && (cores.contains(&0) || cores.contains(&1) || min_core < 2))
        {
            ("Gold Cluster (Cores 0-1)", "Cortex-A78C")
        } else if (max_khz > 0 && max_khz < 2_050_000) || cores.iter().any(|&c| c >= 4) {
            ("Silver Cluster (Cores 4-7)", "Cortex-A55")
        } else {
            ("CPU Cluster", "ARMv8")
        };

        let load_pct = if !cores.is_empty() && !per_core_pct.is_empty() {
            let sum: f32 = cores
                .iter()
                .filter_map(|&c| per_core_pct.get(c as usize))
                .sum();
            ((sum / cores.len() as f32) * 10.0).round() / 10.0
        } else {
            0.0
        };

        clusters.push(CpuCluster {
            name,
            architecture: arch,
            cores,
            min_freq_mhz: min_khz / 1000,
            max_freq_mhz: max_khz / 1000,
            cur_freq_mhz: cur_khz / 1000,
            governor,
            load_pct,
        });
    }

    if clusters.is_empty() {
        vec![
            CpuCluster {
                name: "Gold Cluster (Cores 0-1)",
                architecture: "Cortex-A78C",
                cores: vec![0, 1],
                min_freq_mhz: 0,
                max_freq_mhz: 0,
                cur_freq_mhz: 0,
                governor: "unavailable".to_string(),
                load_pct: 0.0,
            },
            CpuCluster {
                name: "Prime Cluster (Cores 2-3)",
                architecture: "Cortex-A78C",
                cores: vec![2, 3],
                min_freq_mhz: 0,
                max_freq_mhz: 0,
                cur_freq_mhz: 0,
                governor: "unavailable".to_string(),
                load_pct: 0.0,
            },
            CpuCluster {
                name: "Silver Cluster (Cores 4-7)",
                architecture: "Cortex-A55",
                cores: vec![4, 5, 6, 7],
                min_freq_mhz: 0,
                max_freq_mhz: 0,
                cur_freq_mhz: 0,
                governor: "unavailable".to_string(),
                load_pct: 0.0,
            },
        ]
    } else {
        clusters.sort_by_key(|c| c.cores.first().copied().unwrap_or(0));
        clusters
    }
}

/// Helper to determine if a CPU core is a Big / Prime core.
/// Checks dynamic CPU capacity or cpufreq max frequency if available in sysfs,
/// falling back to the standard Arduino Ventuno Q / Snapdragon QCS8275 layout (Cores 0-3 Big, Cores 4-7 Little).
pub fn is_big_core(core: u32) -> bool {
    let cap_path = format!("/sys/devices/system/cpu/cpu{}/cpu_capacity", core);
    if let Some(cap) = read_cached_u32(&cap_path) {
        return cap > 600;
    }

    let max_freq_path = format!(
        "/sys/devices/system/cpu/cpu{}/cpufreq/cpuinfo_max_freq",
        core
    );
    if let Some(max_khz) = read_cached_u32(&max_freq_path) {
        if max_khz > 2_000_000 {
            return true;
        } else if max_khz > 0 {
            return false;
        }
    }

    core < 4
}

/// Get cluster label for a core ("Gold", "Prime", "Silv", or "Core").
pub fn get_core_cluster_label(core: u32) -> &'static str {
    let max_freq_path = format!(
        "/sys/devices/system/cpu/cpu{}/cpufreq/cpuinfo_max_freq",
        core
    );
    if let Some(max_khz) = read_cached_u32(&max_freq_path) {
        if max_khz >= 2_300_000 {
            return "Prime";
        } else if max_khz >= 2_050_000 {
            return "Gold";
        } else if max_khz > 0 {
            return "Silv";
        }
    }

    let cap_path = format!("/sys/devices/system/cpu/cpu{}/cpu_capacity", core);
    if let Some(cap) = read_cached_u32(&cap_path) {
        if cap >= 1000 {
            return "Prime";
        } else if cap > 600 {
            return "Gold";
        } else if cap > 0 {
            return "Silv";
        }
    }

    if core < 2 {
        "Gold"
    } else if core < 4 {
        "Prime"
    } else if core < 8 {
        "Silv"
    } else {
        "Core"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_cache::EnvGuard;

    #[test]
    fn test_cpu_architecture_fallback() {
        let arch = get_cpu_architecture();
        assert!(!arch.is_empty());
    }

    #[test]
    fn test_dynamic_cpufreq_policy_discovery() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hw_cpufreq_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let policy0 = temp_dir.join("sys/devices/system/cpu/cpufreq/policy0");
        let policy4 = temp_dir.join("sys/devices/system/cpu/cpufreq/policy4");
        std::fs::create_dir_all(&policy0).unwrap();
        std::fs::create_dir_all(&policy4).unwrap();

        std::fs::write(policy0.join("affected_cpus"), "0 1\n").unwrap();
        std::fs::write(policy0.join("scaling_min_freq"), "600000\n").unwrap();
        std::fs::write(policy0.join("scaling_max_freq"), "2100000\n").unwrap();
        std::fs::write(policy0.join("scaling_cur_freq"), "1200000\n").unwrap();
        std::fs::write(policy0.join("scaling_governor"), "schedutil\n").unwrap();

        std::fs::write(policy4.join("related_cpus"), "4-7\n").unwrap();
        std::fs::write(policy4.join("scaling_min_freq"), "500000\n").unwrap();
        std::fs::write(policy4.join("scaling_max_freq"), "1900000\n").unwrap();
        std::fs::write(policy4.join("scaling_cur_freq"), "800000\n").unwrap();
        std::fs::write(policy4.join("scaling_governor"), "performance\n").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let per_core_pct = vec![10.0, 20.0, 0.0, 0.0, 5.0, 5.0, 5.0, 5.0];
        let clusters = get_cpu_clusters_with_load(&per_core_pct);

        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].cores, vec![0, 1]);
        assert_eq!(clusters[0].min_freq_mhz, 600);
        assert_eq!(clusters[0].max_freq_mhz, 2100);
        assert_eq!(clusters[0].cur_freq_mhz, 1200);
        assert_eq!(clusters[0].governor, "schedutil");
        assert_eq!(clusters[0].load_pct, 15.0);

        assert_eq!(clusters[1].cores, vec![4, 5, 6, 7]);
        assert_eq!(clusters[1].min_freq_mhz, 500);
        assert_eq!(clusters[1].max_freq_mhz, 1900);
        assert_eq!(clusters[1].cur_freq_mhz, 800);
        assert_eq!(clusters[1].governor, "performance");
        assert_eq!(clusters[1].load_pct, 5.0);

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_dynamic_cpufreq_missing_honest_semantics() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hw_cpufreq_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let clusters = get_cpu_clusters_with_load(&[]);
        assert!(!clusters.is_empty());
        for cluster in &clusters {
            assert_eq!(cluster.min_freq_mhz, 0);
            assert_eq!(cluster.max_freq_mhz, 0);
            assert_eq!(cluster.cur_freq_mhz, 0);
            assert_eq!(cluster.governor, "unavailable");
        }

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cpu_frequencies_offline_core_continues() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hw_cpu_freqs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let cpu0 = temp_dir.join("sys/devices/system/cpu/cpu0/cpufreq");
        let cpu1 = temp_dir.join("sys/devices/system/cpu/cpu1"); // offline: no cpufreq dir
        let cpu2 = temp_dir.join("sys/devices/system/cpu/cpu2/cpufreq");
        std::fs::create_dir_all(&cpu0).unwrap();
        std::fs::create_dir_all(&cpu1).unwrap();
        std::fs::create_dir_all(&cpu2).unwrap();

        std::fs::write(cpu0.join("scaling_cur_freq"), "1000000\n").unwrap();
        std::fs::write(cpu2.join("scaling_cur_freq"), "2000000\n").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let freqs = get_cpu_frequencies();
        assert!(freqs.len() >= 3);
        assert_eq!(freqs[0], 1000);
        assert_eq!(freqs[1], 0); // offline core yields 0, loop did not abort
        assert_eq!(freqs[2], 2000); // subsequent core was successfully read

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_dynamic_core_topology_and_labels() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hw_topology_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let cpu0 = temp_dir.join("sys/devices/system/cpu/cpu0/cpufreq");
        let cpu2 = temp_dir.join("sys/devices/system/cpu/cpu2/cpufreq");
        let cpu4 = temp_dir.join("sys/devices/system/cpu/cpu4/cpufreq");
        std::fs::create_dir_all(&cpu0).unwrap();
        std::fs::create_dir_all(&cpu2).unwrap();
        std::fs::create_dir_all(&cpu4).unwrap();

        std::fs::write(cpu0.join("cpuinfo_max_freq"), "2112000\n").unwrap();
        std::fs::write(cpu2.join("cpuinfo_max_freq"), "2361600\n").unwrap();
        std::fs::write(cpu4.join("cpuinfo_max_freq"), "1958400\n").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        assert!(is_big_core(0));
        assert!(is_big_core(2));
        assert!(!is_big_core(4));

        assert_eq!(get_core_cluster_label(0), "Gold");
        assert_eq!(get_core_cluster_label(2), "Prime");
        assert_eq!(get_core_cluster_label(4), "Silv");

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
