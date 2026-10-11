// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use std::process::Command;

#[test]
#[cfg(not(target_arch = "aarch64"))]
fn test_cli_help() {
    let bin = env!("CARGO_BIN_EXE_ventop-ai");
    let output = Command::new(bin)
        .arg("--help")
        .output()
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("USAGE:"));
    assert!(stdout.contains("OPTIONS:"));
    assert!(stdout.contains("Hexagon NPU"));
}

#[test]
#[cfg(not(target_arch = "aarch64"))]
fn test_cli_version() {
    let bin = env!("CARGO_BIN_EXE_ventop-ai");
    let output = Command::new(bin)
        .arg("--version")
        .output()
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
#[cfg(not(target_arch = "aarch64"))]
fn test_cli_schema() {
    let bin = env!("CARGO_BIN_EXE_ventop-ai");
    let output = Command::new(bin)
        .arg("--schema")
        .output()
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FIELD"));
    assert!(stdout.contains("TYPE"));
    assert!(stdout.contains("DESCRIPTION"));
    assert!(stdout.contains("hexagon_npu"));
}

#[test]
#[cfg(not(target_arch = "aarch64"))]
fn test_cli_oneshot_mode() {
    let bin = env!("CARGO_BIN_EXE_ventop-ai");
    let output = Command::new(bin)
        .arg("--oneshot")
        .output()
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Ventuno Q") || stdout.contains("Dual-Brain"));
}

#[test]
#[cfg(not(target_arch = "aarch64"))]
fn test_cli_invalid_flag() {
    let bin = env!("CARGO_BIN_EXE_ventop-ai");
    let output = Command::new(bin)
        .arg("--invalid-flag-xyz")
        .output()
        .expect("Failed to execute binary");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unrecognized option") || stderr.contains("error"));
}

#[test]
#[cfg(not(target_arch = "aarch64"))]
fn test_cli_json_mode() {
    let bin = env!("CARGO_BIN_EXE_ventop-ai");
    let output = Command::new(bin)
        .arg("--json")
        .output()
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("Failed to parse JSON output");
    assert!(v.get("hexagon_npu").is_some());
}

#[test]
#[cfg(not(target_arch = "aarch64"))]
fn test_cli_calibrate_idle_flag() {
    let bin = env!("CARGO_BIN_EXE_ventop-ai");
    let output = Command::new(bin)
        .arg("--calibrate-idle")
        .arg("--json")
        .output()
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("Failed to parse JSON output");
    assert!(v.get("power_fan").is_some());
}
