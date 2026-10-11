// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use crate::file_cache::resolve_sysfs_path;
use std::fs;

/// Status of a CAN interface exposed by the STM32H5
#[derive(Debug, Clone, serde::Serialize)]
pub struct CanBusStatus {
    pub name: String,
    pub is_up: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Snapshot of the STM32H5 "Action Brain" coprocessor
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct ActionBrainSnapshot {
    pub is_available: bool,
    pub mcu_model: &'static str,
    pub rtos: &'static str,
    pub serial_device: String,
    pub baudrate: u32,
    pub router_socket_active: bool,
    pub can_interfaces: Vec<CanBusStatus>,
}

/// Check if the STM32H5 serial bridge device is present
pub fn get_action_brain_serial_device() -> Option<String> {
    // 1. Dedicated on-board platform serial port
    let tty_stm0 = resolve_sysfs_path("/dev/ttySTM0");
    if tty_stm0.exists() {
        return Some("/dev/ttySTM0".to_string());
    }

    // 2. USB CDC-ACM peripheral (/dev/ttyACM0)
    let tty_acm0 = resolve_sysfs_path("/dev/ttyACM0");
    if tty_acm0.exists() {
        // Inspect USB vendor ID if exposed in sysfs: STMicroelectronics (0483) or Arduino (2341)
        let id_vendor_path = resolve_sysfs_path("/sys/class/tty/ttyACM0/device/idVendor");
        if let Ok(vid) = fs::read_to_string(&id_vendor_path) {
            let vid_trim = vid.trim().to_lowercase();
            if vid_trim == "0483" || vid_trim == "2341" {
                return Some("/dev/ttyACM0".to_string());
            }
            // Foreign USB CDC-ACM peripheral (e.g. Huawei modem, FTDI) - reject
            return None;
        }

        // Check /dev/serial/by-id/ for STM32 or Arduino symlinks
        let by_id_dir = resolve_sysfs_path("/dev/serial/by-id");
        if let Ok(entries) = fs::read_dir(&by_id_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let name_lower = name.to_lowercase();
                if name_lower.contains("stm32")
                    || name_lower.contains("arduino")
                    || name_lower.contains("ventuno")
                {
                    return Some("/dev/ttyACM0".to_string());
                }
            }
        }

        // If neither sysfs USB idVendor nor by-id directory exists (e.g. minimal mock environment),
        // accept /dev/ttyACM0 if idVendor path does not exist
        if !id_vendor_path.exists() {
            return Some("/dev/ttyACM0".to_string());
        }
    }

    None
}

/// Check if the arduino-router Unix socket is present and active (alive/listening, not stale)
pub fn is_arduino_router_active() -> bool {
    let sock_candidates = ["/var/run/arduino-router.sock", "/run/arduino-router.sock"];

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        use std::os::unix::net::UnixStream;

        for sock_candidate in &sock_candidates {
            let sock_path = resolve_sysfs_path(sock_candidate);
            if let Ok(meta) = fs::metadata(&sock_path) {
                if meta.file_type().is_socket() {
                    match UnixStream::connect(&sock_path) {
                        Ok(_) => return true,
                        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                            // Stale dead socket file left on disk
                            continue;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                            // Listening socket exists with restricted permissions
                            return true;
                        }
                        Err(_) => continue,
                    }
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        for sock_candidate in &sock_candidates {
            let sock_path = resolve_sysfs_path(sock_candidate);
            if sock_path.exists() {
                return true;
            }
        }
    }

    false
}

/// Inspect CAN interfaces exposed by the STM32H5 (can0, can1, can2)
pub fn get_can_interfaces() -> Vec<CanBusStatus> {
    let mut interfaces = Vec::new();
    let net_dir = resolve_sysfs_path("/sys/class/net");

    if let Ok(entries) = fs::read_dir(net_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("can") {
                let operstate_path = entry.path().join("operstate");
                let is_up = fs::read_to_string(&operstate_path)
                    .map(|s| s.trim() == "up" || s.trim() == "unknown")
                    .unwrap_or(false);

                let rx_path = entry.path().join("statistics/rx_bytes");
                let tx_path = entry.path().join("statistics/tx_bytes");

                let rx_bytes = fs::read_to_string(&rx_path)
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0);

                let tx_bytes = fs::read_to_string(&tx_path)
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0);

                interfaces.push(CanBusStatus {
                    name,
                    is_up,
                    rx_bytes,
                    tx_bytes,
                });
            }
        }
    }

    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    interfaces
}

/// Collect full telemetry snapshot of the STM32H5 Action Brain
pub fn collect_action_brain_snapshot() -> ActionBrainSnapshot {
    let serial_dev = get_action_brain_serial_device();
    let is_available = serial_dev.is_some();
    let router_active = is_arduino_router_active();
    let can_interfaces = get_can_interfaces();

    ActionBrainSnapshot {
        is_available,
        mcu_model: "STM32H5F5 Arm Cortex-M33 @ 250MHz",
        rtos: "Zephyr RTOS (Arduino Core)",
        serial_device: serial_dev.unwrap_or_else(|| "Not Connected".to_string()),
        baudrate: 115200,
        router_socket_active: router_active,
        can_interfaces,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_brain_defaults() {
        let snapshot = collect_action_brain_snapshot();
        assert_eq!(snapshot.mcu_model, "STM32H5F5 Arm Cortex-M33 @ 250MHz");
        assert_eq!(snapshot.rtos, "Zephyr RTOS (Arduino Core)");
        assert_eq!(snapshot.baudrate, 115200);
    }

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set_root(path: &std::path::Path) -> Self {
            let lock = crate::file_cache::TEST_ENV_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            crate::file_cache::reset_file_cache();
            std::env::set_var("VENTOP_SYSFS_ROOT", path);
            Self { _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("VENTOP_SYSFS_ROOT");
            crate::file_cache::reset_file_cache();
        }
    }

    #[test]
    fn test_serial_device_filters_foreign_usb() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_ab_serial_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let dev_dir = temp_dir.join("dev");
        let sysfs_acm = temp_dir.join("sys/class/tty/ttyACM0/device");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::create_dir_all(&sysfs_acm).unwrap();

        // Create /dev/ttyACM0 device node file
        std::fs::write(dev_dir.join("ttyACM0"), "").unwrap();

        // Case 1: Foreign USB VID (e.g. 12d1 Huawei USB modem) -> Rejected
        std::fs::write(sysfs_acm.join("idVendor"), "12d1\n").unwrap();
        let _guard = EnvGuard::set_root(&temp_dir);
        assert_eq!(get_action_brain_serial_device(), None);

        // Case 2: STMicroelectronics USB VID (0483) -> Accepted
        std::fs::write(sysfs_acm.join("idVendor"), "0483\n").unwrap();
        assert_eq!(
            get_action_brain_serial_device(),
            Some("/dev/ttyACM0".to_string())
        );

        // Case 3: Dedicated platform port /dev/ttySTM0 -> Always accepted
        std::fs::write(dev_dir.join("ttySTM0"), "").unwrap();
        assert_eq!(
            get_action_brain_serial_device(),
            Some("/dev/ttySTM0".to_string())
        );

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_socket_stale_vs_active() {
        let temp_dir = std::env::temp_dir().join(format!("ventop_ab_sock_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let run_dir = temp_dir.join("var/run");
        std::fs::create_dir_all(&run_dir).unwrap();
        let sock_path = run_dir.join("arduino-router.sock");

        let _guard = EnvGuard::set_root(&temp_dir);

        // Case 1: Socket file does not exist
        assert!(!is_arduino_router_active());

        // Case 2: Regular file (not a socket)
        std::fs::write(&sock_path, "regular text").unwrap();
        assert!(!is_arduino_router_active());
        let _ = std::fs::remove_file(&sock_path);

        #[cfg(unix)]
        {
            use std::os::unix::net::UnixListener;

            // Case 3: Live listening socket -> true
            let listener = UnixListener::bind(&sock_path).expect("bind socket");
            assert!(is_arduino_router_active());

            // Case 4: Listener is closed, but socket file remains on disk -> Stale socket -> false
            drop(listener);
            assert!(sock_path.exists()); // inode remains on disk
            assert!(!is_arduino_router_active()); // connection refused -> reports inactive!
        }

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
