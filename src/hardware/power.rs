// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use crate::file_cache::{read_cached_file, read_cached_u32, resolve_sysfs_path};
use std::fs;
use std::path::{Path, PathBuf};
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

/// Maximum physically sane fan RPM ceiling.
/// Guards against 0xFFFF (65535) tach register saturation or EMI spikes on floating tachometer pins.
const MAX_SANE_FAN_RPM: u32 = 30_000;

/// Override the default board idle baseline power in Watts
pub fn set_idle_baseline_watts(val: f64) {
    IDLE_BASELINE_BITS.store(val.to_bits(), Ordering::Relaxed);
}

/// Retrieve the active board idle baseline power in Watts
pub fn get_idle_baseline_watts() -> f64 {
    f64::from_bits(IDLE_BASELINE_BITS.load(Ordering::Relaxed))
}

/// Parse a numeric channel index from a hwmon `<kind><N>` prefix (after stripping `_input`).
fn parse_hwmon_channel(prefix: &str, kind: &str) -> Option<u32> {
    let digits = prefix.strip_prefix(kind)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok()
}

#[derive(Default)]
struct HwmonDeviceReadings {
    fan_rpm: Option<u32>,
    power_uw: Option<u32>,
    volt_mv: Option<u32>,
    curr_ma: Option<u32>,
}

/// Scan a single `hwmon*` device directory in one pass for fan and/or power channels.
fn scan_hwmon_device_dir(path: &Path, scan_fan: bool, scan_power: bool) -> HwmonDeviceReadings {
    let mut readings = HwmonDeviceReadings::default();
    if !scan_fan && !scan_power {
        return readings;
    }

    let mut found_bus_volt_mv: Option<u32> = None;
    let mut found_shunt_volt_mv: Option<u32> = None;

    if let Ok(dir_entries) = fs::read_dir(path) {
        for entry in dir_entries.flatten() {
            let f_name = entry.file_name().to_string_lossy().to_string();
            let Some(prefix) = f_name.strip_suffix("_input") else {
                continue;
            };

            if scan_fan && matches!(parse_hwmon_channel(prefix, "fan"), Some(ch) if ch >= 1) {
                if let Some(rpm) = read_cached_u32(&entry.path().to_string_lossy()) {
                    if rpm > MAX_SANE_FAN_RPM {
                        continue;
                    }
                    if rpm > 0 {
                        let fault_path = path.join(format!("{}_fault", prefix));
                        if let Some(fault) = read_cached_u32(&fault_path.to_string_lossy()) {
                            if fault != 0 {
                                continue;
                            }
                        }
                    }
                    readings.fan_rpm = Some(readings.fan_rpm.map_or(rpm, |prev| prev.max(rpm)));
                }
            } else if scan_power {
                if matches!(parse_hwmon_channel(prefix, "power"), Some(ch) if ch >= 1) {
                    if let Some(uw) = read_cached_u32(&entry.path().to_string_lossy()) {
                        readings.power_uw = Some(readings.power_uw.map_or(uw, |prev| prev.max(uw)));
                    }
                } else if let Some(ch) = parse_hwmon_channel(prefix, "in") {
                    if let Some(mv) = read_cached_u32(&entry.path().to_string_lossy()) {
                        if ch == 0 {
                            // in0_input on INA2xx/INA232 is shunt voltage across the sense resistor;
                            // only use as a fallback when no bus voltage channel (in[1..]_input) exists.
                            found_shunt_volt_mv =
                                Some(found_shunt_volt_mv.map_or(mv, |prev| prev.max(mv)));
                        } else {
                            found_bus_volt_mv =
                                Some(found_bus_volt_mv.map_or(mv, |prev| prev.max(mv)));
                        }
                    }
                } else if matches!(parse_hwmon_channel(prefix, "curr"), Some(ch) if ch >= 1) {
                    if let Some(ma) = read_cached_u32(&entry.path().to_string_lossy()) {
                        readings.curr_ma = Some(readings.curr_ma.map_or(ma, |prev| prev.max(ma)));
                    }
                }
            }
        }
    }

    readings.volt_mv = found_bus_volt_mv.or(found_shunt_volt_mv);
    readings
}

/// Read hardware fan speed and live board power draw from /sys/class/hwmon
pub fn get_hardware_power_fan() -> HardwarePowerFan {
    let mut result = HardwarePowerFan::default();
    let hwmon_dir = resolve_sysfs_path("/sys/class/hwmon");

    let mut best_power_priority = 0u8;
    let mut best_power_uw = 0u32;
    let mut best_fan_priority = 0u8;

    if let Ok(entries) = fs::read_dir(hwmon_dir) {
        let mut sorted_entries: Vec<_> = entries.flatten().collect();
        sorted_entries.sort_by_key(|e| e.file_name());

        let mut fallback_fan_dirs: Vec<(PathBuf, String)> = Vec::new();

        for entry in sorted_entries {
            let path = entry.path();
            let name_path = path.join("name");
            let dev_name = read_cached_file(&name_path.to_string_lossy())
                .unwrap_or_default()
                .trim()
                .to_string();
            let dev_name_lower = dev_name.to_lowercase();

            // Explicit fan controller matching:
            // emc2305 / emc -> priority 3
            // fan -> priority 2
            // any other device exposing fan[1..]_input -> priority 1 (fallback)
            let fan_priority = if dev_name_lower.contains("emc") {
                3
            } else if dev_name_lower.contains("fan") {
                2
            } else {
                0
            };

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

            if fan_priority == 0 && power_priority == 0 {
                // Defer non-fan/non-power hwmon directories (e.g. 44 thermal zones on Ventuno Q)
                // so we never call fs::read_dir on them when a dedicated fan controller is present.
                fallback_fan_dirs.push((path, dev_name));
                continue;
            }

            let effective_fan_prio = if fan_priority > 0 { fan_priority } else { 1 };
            let scan_fan = effective_fan_prio >= best_fan_priority;
            let scan_power = power_priority >= best_power_priority && power_priority > 0;

            let readings = scan_hwmon_device_dir(&path, scan_fan, scan_power);

            if let Some(rpm) = readings.fan_rpm {
                let should_replace_fan = effective_fan_prio > best_fan_priority
                    || (effective_fan_prio == best_fan_priority
                        && result.fan_rpm.map_or(true, |current_rpm| rpm > current_rpm));
                if should_replace_fan {
                    best_fan_priority = effective_fan_prio;
                    result.fan_rpm = Some(rpm);
                    result.fan_device = Some(if !dev_name.is_empty() {
                        dev_name.clone()
                    } else {
                        "fan".to_string()
                    });
                }
            }

            if let Some(uw) = readings.power_uw {
                let should_replace_power = power_priority > best_power_priority
                    || (power_priority == best_power_priority
                        && (result.board_power_watts.is_none() || uw > best_power_uw));
                if should_replace_power {
                    best_power_priority = power_priority;
                    best_power_uw = uw;
                    result.board_power_watts = Some(uw as f64 / 1_000_000.0);
                    result.power_device = Some(dev_name);
                    result.board_voltage_v = readings.volt_mv.map(|mv| mv as f64 / 1_000.0);
                    result.board_current_a = readings.curr_ma.map(|ma| ma as f64 / 1_000.0);
                }
            }
        }

        // Only scan deferred generic hwmon directories if no dedicated fan controller (priority >= 2)
        // was found during the primary pass.
        if best_fan_priority <= 1 {
            for (path, dev_name) in fallback_fan_dirs {
                let readings = scan_hwmon_device_dir(&path, true, false);
                if let Some(rpm) = readings.fan_rpm {
                    let effective_prio = 1;
                    let should_replace_fan = effective_prio > best_fan_priority
                        || (effective_prio == best_fan_priority
                            && result.fan_rpm.map_or(true, |current_rpm| rpm > current_rpm));
                    if should_replace_fan {
                        best_fan_priority = effective_prio;
                        result.fan_rpm = Some(rpm);
                        result.fan_device = Some(if !dev_name.is_empty() {
                            dev_name
                        } else {
                            "fan".to_string()
                        });
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

        // hwmon8 is emc2305 with all 5 channels (fan1_input..fan5_input, only fan3_input active)
        std::fs::write(hwmon8.join("name"), "emc2305\n").unwrap();
        std::fs::write(hwmon8.join("fan1_input"), "0\n").unwrap();
        std::fs::write(hwmon8.join("fan2_input"), "0\n").unwrap();
        std::fs::write(hwmon8.join("fan3_input"), "2850\n").unwrap();
        std::fs::write(hwmon8.join("fan4_input"), "0\n").unwrap();
        std::fs::write(hwmon8.join("fan5_input"), "0\n").unwrap();

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
    fn test_ventuno_emc2305_five_channel_all_positions_active() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hw_emc5ch_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let hwmon0 = temp_dir.join("sys/class/hwmon/hwmon0");
        std::fs::create_dir_all(&hwmon0).unwrap();
        std::fs::write(hwmon0.join("name"), "emc2305\n").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);

        for active_ch in 1..=5 {
            for ch in 1..=5 {
                let rpm = if ch == active_ch { 2818 } else { 0 };
                std::fs::write(
                    hwmon0.join(format!("fan{}_input", ch)),
                    format!("{}\n", rpm),
                )
                .unwrap();
                std::fs::write(hwmon0.join(format!("fan{}_fault", ch)), "0\n").unwrap();
            }
            crate::file_cache::reset_file_cache();

            let pf = get_hardware_power_fan();
            assert_eq!(
                pf.fan_rpm,
                Some(2818),
                "Failed to read 2818 RPM when active channel was fan{}",
                active_ch
            );
            assert_eq!(pf.fan_device.as_deref(), Some("emc2305"));
        }

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_emc2305_ignores_faulted_and_saturated_fan_channels() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hw_emcfault_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let hwmon0 = temp_dir.join("sys/class/hwmon/hwmon0");
        std::fs::create_dir_all(&hwmon0).unwrap();
        std::fs::write(hwmon0.join("name"), "emc2305\n").unwrap();

        // fan1: valid 2818 RPM, no fault
        std::fs::write(hwmon0.join("fan1_input"), "2818\n").unwrap();
        std::fs::write(hwmon0.join("fan1_fault"), "0\n").unwrap();
        // fan2: bogus 9500 RPM, faulted (fan2_fault = 1)
        std::fs::write(hwmon0.join("fan2_input"), "9500\n").unwrap();
        std::fs::write(hwmon0.join("fan2_fault"), "1\n").unwrap();
        // fan3: saturated 65535 RPM (> MAX_SANE_FAN_RPM), even with fan3_fault = 0
        std::fs::write(hwmon0.join("fan3_input"), "65535\n").unwrap();
        std::fs::write(hwmon0.join("fan3_fault"), "0\n").unwrap();
        // fan4..fan5: unused 0 RPM
        std::fs::write(hwmon0.join("fan4_input"), "0\n").unwrap();
        std::fs::write(hwmon0.join("fan5_input"), "0\n").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let pf = get_hardware_power_fan();
        assert_eq!(pf.fan_rpm, Some(2818));
        assert_eq!(pf.fan_device.as_deref(), Some("emc2305"));

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_ina232_shunt_in0_vs_bus_in1_voltage_selection() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hw_ina232_in0_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let hwmon12 = temp_dir.join("sys/class/hwmon/hwmon12");
        std::fs::create_dir_all(&hwmon12).unwrap();
        std::fs::write(hwmon12.join("name"), "ina232\n").unwrap();
        std::fs::write(hwmon12.join("power1_input"), "12400000\n").unwrap();
        // in0_input is shunt voltage (8 mV), in1_input is 12V bus voltage (12050 mV)
        std::fs::write(hwmon12.join("in0_input"), "8\n").unwrap();
        std::fs::write(hwmon12.join("in1_input"), "12050\n").unwrap();
        std::fs::write(hwmon12.join("curr1_input"), "1030\n").unwrap();
        // Non-numeric / bogus *_input files must be ignored
        std::fs::write(hwmon12.join("in_label_input"), "99999\n").unwrap();
        std::fs::write(hwmon12.join("power_avg_input"), "99999999\n").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let pf = get_hardware_power_fan();
        assert_eq!(pf.board_power_watts, Some(12.4));
        assert_eq!(pf.board_voltage_v, Some(12.05));
        assert_eq!(pf.board_current_a, Some(1.03));

        // Verify fallback to in0_input when no in[1..]_input exists
        std::fs::remove_file(hwmon12.join("in1_input")).unwrap();
        crate::file_cache::reset_file_cache();
        let pf_fallback = get_hardware_power_fan();
        assert_eq!(pf_fallback.board_voltage_v, Some(0.008));

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_multi_device_priority_hierarchy_and_no_metric_bleed() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hw_prio_bleed_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let hwmon0 = temp_dir.join("sys/class/hwmon/hwmon0");
        let hwmon1 = temp_dir.join("sys/class/hwmon/hwmon1");
        let hwmon2 = temp_dir.join("sys/class/hwmon/hwmon2");
        let hwmon3 = temp_dir.join("sys/class/hwmon/hwmon3");
        std::fs::create_dir_all(&hwmon0).unwrap();
        std::fs::create_dir_all(&hwmon1).unwrap();
        std::fs::create_dir_all(&hwmon2).unwrap();
        std::fs::create_dir_all(&hwmon3).unwrap();

        // Generic sensor (fan priority 1, 1500 RPM; power priority 1) vs pwm_fan (fan priority 2, 0 RPM)
        std::fs::write(hwmon0.join("name"), "generic_power\n").unwrap();
        std::fs::write(hwmon0.join("fan1_input"), "1500\n").unwrap();

        std::fs::write(hwmon1.join("name"), "pwm_fan\n").unwrap();
        std::fs::write(hwmon1.join("fan1_input"), "0\n").unwrap();

        // Lower-priority power sensors in both hwmon0 and hwmon3 (so at least one is visited
        // before hwmon2 regardless of read_dir order) with voltage and current attributes
        std::fs::write(hwmon0.join("power1_input"), "5000000\n").unwrap();
        std::fs::write(hwmon0.join("in1_input"), "5000\n").unwrap();
        std::fs::write(hwmon0.join("curr1_input"), "500\n").unwrap();

        std::fs::write(hwmon3.join("name"), "generic_power\n").unwrap();
        std::fs::write(hwmon3.join("power1_input"), "5000000\n").unwrap();
        std::fs::write(hwmon3.join("in1_input"), "5000\n").unwrap();
        std::fs::write(hwmon3.join("curr1_input"), "500\n").unwrap();

        // Higher-priority ina232 in hwmon2 exposing ONLY power1_input (no in*_input or curr*_input)
        std::fs::write(hwmon2.join("name"), "ina232\n").unwrap();
        std::fs::write(hwmon2.join("power1_input"), "11800000\n").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let pf = get_hardware_power_fan();

        // pwm_fan (priority 2) must beat generic_dev (priority 1) even at 0 RPM
        assert_eq!(pf.fan_device.as_deref(), Some("pwm_fan"));
        assert_eq!(pf.fan_rpm, Some(0));

        // ina232 (priority 2) must win and clear stale voltage/current from lower-priority sensors
        assert_eq!(pf.power_device.as_deref(), Some("ina232"));
        assert_eq!(pf.board_power_watts, Some(11.8));
        assert_eq!(pf.board_voltage_v, None);
        assert_eq!(pf.board_current_a, None);

        // Adding emc2305 (priority 3) must beat pwm_fan (priority 2)
        std::fs::write(hwmon3.join("name"), "emc2305\n").unwrap();
        std::fs::write(hwmon3.join("fan1_input"), "2400\n").unwrap();
        crate::file_cache::reset_file_cache();

        let pf2 = get_hardware_power_fan();
        assert_eq!(pf2.fan_device.as_deref(), Some("emc2305"));
        assert_eq!(pf2.fan_rpm, Some(2400));

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

    #[test]
    fn test_fallback_generic_fan_and_thermal_hwmon_skip() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_hw_thermal_skip_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);

        // Create 44 thermal hwmon directories alongside emc2305 (hwmon0) and ina232 (hwmon4)
        for i in 0..46 {
            let dir = temp_dir.join(format!("sys/class/hwmon/hwmon{}", i));
            std::fs::create_dir_all(&dir).unwrap();
            if i == 0 {
                std::fs::write(dir.join("name"), "emc2305\n").unwrap();
                std::fs::write(dir.join("fan1_input"), "2818\n").unwrap();
                std::fs::write(dir.join("fan1_fault"), "0\n").unwrap();
            } else if i == 4 {
                std::fs::write(dir.join("name"), "ina232\n").unwrap();
                std::fs::write(dir.join("power1_input"), "11500000\n").unwrap();
                std::fs::write(dir.join("in1_input"), "12000\n").unwrap();
                std::fs::write(dir.join("curr1_input"), "958\n").unwrap();
            } else {
                std::fs::write(dir.join("name"), format!("tsens_tz_sensor{}\n", i)).unwrap();
                std::fs::write(dir.join("temp1_input"), "45000\n").unwrap();
            }
        }

        let _guard = EnvGuard::set_root(&temp_dir);
        let pf = get_hardware_power_fan();
        assert_eq!(pf.fan_rpm, Some(2818));
        assert_eq!(pf.fan_device.as_deref(), Some("emc2305"));
        assert_eq!(pf.board_power_watts, Some(11.5));
        assert_eq!(pf.power_device.as_deref(), Some("ina232"));

        // Verify fallback pass discovers generic fan device when no emc/fan device is present
        std::fs::write(
            temp_dir.join("sys/class/hwmon/hwmon0/name"),
            "some_custom_asic\n",
        )
        .unwrap();
        std::fs::write(temp_dir.join("sys/class/hwmon/hwmon0/fan1_input"), "1800\n").unwrap();
        crate::file_cache::reset_file_cache();

        let pf_fallback = get_hardware_power_fan();
        assert_eq!(pf_fallback.fan_rpm, Some(1800));
        assert_eq!(pf_fallback.fan_device.as_deref(), Some("some_custom_asic"));

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
