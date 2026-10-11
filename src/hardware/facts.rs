// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use crate::file_cache::resolve_sysfs_path;
use std::fs;

/// Platform software and build facts (kernel version, dpkg package versions, git hash)
#[derive(Debug, Clone, serde::Serialize)]
pub struct PlatformRunFacts {
    pub build_hash: &'static str,
    pub tool_version: &'static str,
    pub kernel_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fastrpc_package_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware_dragonwing_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware_linux_version: Option<String>,
}

/// Detect board name
pub fn get_board_name() -> String {
    let paths = [
        "/proc/device-tree/model",
        "/sys/firmware/devicetree/base/model",
    ];

    for path in &paths {
        let resolved = resolve_sysfs_path(path);
        if resolved.exists() {
            if let Ok(content) = fs::read(&resolved) {
                let model = String::from_utf8_lossy(&content)
                    .trim_end_matches('\0')
                    .trim()
                    .to_string();
                if !model.is_empty() {
                    // Check if Monza/Monaco or Arduino Ventuno
                    if model.contains("Monaco")
                        || model.contains("Monza")
                        || model.contains("arduino")
                    {
                        return "Arduino Ventuno Q".to_string();
                    }
                    return model;
                }
            }
        }
    }

    "Arduino Ventuno Q".to_string()
}

/// Detect Qualcomm SoC model
pub fn get_soc_model() -> String {
    // 1. Read /sys/bus/soc/devices/soc0/machine & family
    let machine_path = resolve_sysfs_path("/sys/bus/soc/devices/soc0/machine");
    let family_path = resolve_sysfs_path("/sys/bus/soc/devices/soc0/family");

    let machine = fs::read_to_string(&machine_path)
        .ok()
        .map(|s| s.trim().to_string());
    let family = fs::read_to_string(&family_path)
        .ok()
        .map(|s| s.trim().to_string());

    if let Some(m) = machine {
        if let Some(f) = family {
            return format!("{} {}", f, m);
        }
        return m;
    }

    // 2. Check DT compatible strings
    let compat_paths = [
        "/proc/device-tree/compatible",
        "/sys/firmware/devicetree/base/compatible",
    ];
    for path in &compat_paths {
        let resolved = resolve_sysfs_path(path);
        if let Ok(content) = fs::read(&resolved) {
            let text = String::from_utf8_lossy(&content);
            if text.contains("qcs8275") || text.contains("qcs8300") {
                return "Snapdragon QCS8275".to_string();
            }
        }
    }

    "Snapdragon QCS8275".to_string()
}

/// Retrieve platform software and build facts, parsed strictly once at startup
pub fn get_platform_run_facts() -> &'static PlatformRunFacts {
    static FACTS: std::sync::OnceLock<PlatformRunFacts> = std::sync::OnceLock::new();
    FACTS.get_or_init(|| {
        let osrelease_path = resolve_sysfs_path("/proc/sys/kernel/osrelease");
        let kernel_version = fs::read_to_string(&osrelease_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let mut fastrpc_ver = None;
        let mut dragonwing_ver = None;
        let mut linux_fw_ver = None;

        let dpkg_path = resolve_sysfs_path("/var/lib/dpkg/status");
        if let Ok(content) = fs::read_to_string(&dpkg_path) {
            let mut current_package: Option<String> = None;
            for line in content.lines() {
                if let Some(pkg) = line.strip_prefix("Package: ") {
                    current_package = Some(pkg.trim().to_string());
                } else if let Some(ver) = line.strip_prefix("Version: ") {
                    if let Some(ref cur) = current_package {
                        if cur == "qcom-fastrpc1" {
                            fastrpc_ver = Some(ver.trim().to_string());
                        } else if cur == "linux-firmware-dragonwing" {
                            dragonwing_ver = Some(ver.trim().to_string());
                        } else if cur == "linux-firmware" {
                            linux_fw_ver = Some(ver.trim().to_string());
                        }
                    }
                } else if line.is_empty() {
                    current_package = None;
                }
            }
        }

        PlatformRunFacts {
            build_hash: env!("GIT_HASH"),
            tool_version: env!("CARGO_PKG_VERSION"),
            kernel_version,
            fastrpc_package_version: fastrpc_ver,
            firmware_dragonwing_version: dragonwing_ver,
            firmware_linux_version: linux_fw_ver,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soc_model_fallback() {
        let model = get_soc_model();
        assert!(!model.is_empty());
    }

    #[test]
    fn test_platform_run_facts_init() {
        let facts = get_platform_run_facts();
        assert!(!facts.build_hash.is_empty());
        assert!(!facts.tool_version.is_empty());
    }
}
