// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use crate::file_cache::{read_cached_u32, resolve_sysfs_path};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

/// Hardware power and fan metrics from INA232 and EMC2305 hwmon sensors
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct HardwarePowerFan {
    pub fan_rpm: Option<u32>,
    pub fan_device: Option<String>,
    pub board_power_watts: Option<f64>,
    pub idle_baseline_watts: f64,
    pub power_above_idle_watts: f64,
    pub board_voltage_v: Option<f64>,
    pub board_current_a: Option<f64>,
    pub power_device: Option<String>,
}

static IDLE_BASELINE_BITS: AtomicU64 = AtomicU64::new(0x402699999999999au64); // 11.3_f64.to_bits()

/// Override the default board idle baseline power in Watts
pub fn set_idle_baseline_watts(val: f64) {
    IDLE_BASELINE_BITS.store(val.to_bits(), Ordering::Relaxed);
}

/// Retrieve the active board idle baseline power in Watts
pub fn get_idle_baseline_watts() -> f64 {
    f64::from_bits(IDLE_BASELINE_BITS.load(Ordering::Relaxed))
}

/// Read hardware fan speed and live board power draw from /sys/class/hwmon
pub fn get_hardware_power_fan() -> HardwarePowerFan {
    let mut result = HardwarePowerFan::default();
    let hwmon_dir = resolve_sysfs_path("/sys/class/hwmon");

    let mut best_power_priority = 0u8;
    let mut best_fan_priority = 0u8;

    if let Ok(entries) = fs::read_dir(hwmon_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name_path = path.join("name");
            let dev_name = fs::read_to_string(&name_path)
                .unwrap_or_default()
                .trim()
                .to_string();
            let dev_name_lower = dev_name.to_lowercase();

            // Fan controller matching:
            // emc2305 / emc -> priority 2
            // fan -> priority 1
            let fan_priority = if dev_name_lower.contains("emc") {
                2
            } else if dev_name_lower.contains("fan") {
                1
            } else {
                0
            };

            // Scan for fan*_input in this hwmon dir
            let mut found_rpm = None;
            if let Ok(dir_entries) = fs::read_dir(&path) {
                for f_entry in dir_entries.flatten() {
                    let f_name = f_entry.file_name().to_string_lossy().to_string();
                    if f_name.starts_with("fan") && f_name.ends_with("_input") {
                        if let Some(rpm) = read_cached_u32(&f_entry.path().to_string_lossy()) {
                            found_rpm = Some(rpm);
                            break;
                        }
                    }
                }
            }

            if let Some(rpm) = found_rpm {
                let effective_prio = if fan_priority > 0 { fan_priority } else { 1 };
                if effective_prio >= best_fan_priority {
                    best_fan_priority = effective_prio;
                    result.fan_rpm = Some(rpm);
                    result.fan_device = Some(if !dev_name.is_empty() {
                        dev_name.clone()
                    } else {
                        "fan".to_string()
                    });
                }
            }

            // Power monitor matching:
            // ina232 / ina2 -> priority 2
            // ina / power -> priority 1
            let power_priority =
                if dev_name_lower.contains("ina232") || dev_name_lower.contains("ina2") {
                    2
                } else if dev_name_lower.contains("ina") || dev_name_lower.contains("power") {
                    1
                } else {
                    0
                };

            if power_priority >= best_power_priority && power_priority > 0 {
                let mut found_power_uw = None;
                let mut found_volt_mv = None;
                let mut found_curr_ma = None;

                if let Ok(dir_entries) = fs::read_dir(&path) {
                    for p_entry in dir_entries.flatten() {
                        let p_name = p_entry.file_name().to_string_lossy().to_string();
                        if p_name.starts_with("power")
                            && p_name.ends_with("_input")
                            && found_power_uw.is_none()
                        {
                            found_power_uw = read_cached_u32(&p_entry.path().to_string_lossy());
                        } else if p_name.starts_with("in")
                            && p_name.ends_with("_input")
                            && found_volt_mv.is_none()
                        {
                            found_volt_mv = read_cached_u32(&p_entry.path().to_string_lossy());
                        } else if p_name.starts_with("curr")
                            && p_name.ends_with("_input")
                            && found_curr_ma.is_none()
                        {
                            found_curr_ma = read_cached_u32(&p_entry.path().to_string_lossy());
                        }
                    }
                }

                if let Some(uw) = found_power_uw {
                    best_power_priority = power_priority;
                    result.board_power_watts = Some(uw as f64 / 1_000_000.0);
                    result.power_device = Some(dev_name.clone());

                    if let Some(mv) = found_volt_mv {
                        result.board_voltage_v = Some(mv as f64 / 1_000.0);
                    }
                    if let Some(ma) = found_curr_ma {
                        result.board_current_a = Some(ma as f64 / 1_000.0);
                    }
                }
            }
        }
    }

    let baseline = get_idle_baseline_watts();
    result.idle_baseline_watts = baseline;
    result.power_above_idle_watts = result
        .board_power_watts
        .map(|w| ((w - baseline).max(0.0) * 100.0).round() / 100.0)
        .unwrap_or(0.0);

    result
}

/// Calibrate board idle baseline power by taking `samples` consecutive readings
/// from live hardware power sensors (e.g. INA232) spaced by `interval`.
/// Updates the active idle baseline if successful and returns the calibrated value in Watts.
pub fn calibrate_idle_baseline(samples: usize, interval: std::time::Duration) -> Option<f64> {
    let mut readings = Vec::with_capacity(samples);
    for i in 0..samples {
        if i > 0 && !interval.is_zero() {
            std::thread::sleep(interval);
        }
        let pwr = get_hardware_power_fan();
        if let Some(w) = pwr.board_power_watts {
            readings.push(w);
        }
    }
    if readings.is_empty() {
        return None;
    }
    let sum: f64 = readings.iter().sum();
    let avg = ((sum / readings.len() as f64) * 100.0).round() / 100.0;
    set_idle_baseline_watts(avg);
    Some(avg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_cache::EnvGuard;

    #[test]
    fn test_dynamic_hwmon_prioritization_and_fan_channels() {
        let temp_dir = std::env::temp_dir().join(format!("ventop_hw_hwmon_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let hwmon0 = temp_dir.join("sys/class/hwmon/hwmon0");
        let hwmon17 = temp_dir.join("sys/class/hwmon/hwmon17");
        let hwmon8 = temp_dir.join("sys/class/hwmon/hwmon8");
        std::fs::create_dir_all(&hwmon0).unwrap();
        std::fs::create_dir_all(&hwmon17).unwrap();
        std::fs::create_dir_all(&hwmon8).unwrap();

        // hwmon0 is generic power device (priority 1)
        std::fs::write(hwmon0.join("name"), "generic_power\n").unwrap();
        std::fs::write(hwmon0.join("power1_input"), "1000000\n").unwrap();

        // hwmon17 is ina232 (priority 2, must override generic_power)
        std::fs::write(hwmon17.join("name"), "ina232\n").unwrap();
        std::fs::write(hwmon17.join("power1_input"), "12400000\n").unwrap();
        std::fs::write(hwmon17.join("in1_input"), "12050\n").unwrap();
        std::fs::write(hwmon17.join("curr1_input"), "1030\n").unwrap();

        // hwmon8 is emc2305 with non-fan1 channel: fan3_input
        std::fs::write(hwmon8.join("name"), "emc2305\n").unwrap();
        std::fs::write(hwmon8.join("fan3_input"), "2850\n").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let pf = get_hardware_power_fan();
        assert_eq!(pf.board_power_watts, Some(12.4));
        assert_eq!(pf.board_voltage_v, Some(12.05));
        assert_eq!(pf.board_current_a, Some(1.03));
        assert_eq!(pf.power_device.as_deref(), Some("ina232"));
        assert_eq!(pf.fan_rpm, Some(2850));
        assert_eq!(pf.fan_device.as_deref(), Some("emc2305"));

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_calibrate_idle_baseline() {
        let temp_dir = std::env::temp_dir().join(format!("ventop_hw_calib_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let hwmon0 = temp_dir.join("sys/class/hwmon/hwmon0");
        std::fs::create_dir_all(&hwmon0).unwrap();
        std::fs::write(hwmon0.join("name"), "ina232\n").unwrap();
        std::fs::write(hwmon0.join("power1_input"), "10500000\n").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let calibrated = calibrate_idle_baseline(2, std::time::Duration::from_millis(1));
        assert_eq!(calibrated, Some(10.5));
        assert_eq!(get_idle_baseline_watts(), 10.5);

        // Reset to default
        set_idle_baseline_watts(11.3);

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
