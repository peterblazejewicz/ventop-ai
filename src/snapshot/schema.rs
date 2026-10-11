// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

/// Return static JSON Schema for VentopSnapshot telemetry
pub fn get_schema_json() -> &'static str {
    r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "VentopSnapshot",
  "type": "object",
  "properties": {
    "timestamp_unix": { "type": "integer", "description": "Unix epoch timestamp in seconds" },
    "timestamp_unix_ms": { "type": "integer", "description": "Unix epoch timestamp in milliseconds" },
    "interval_ms": { "type": ["number", "null"], "description": "Measured stream tick interval in milliseconds" },
    "marker": { "type": ["string", "null"], "description": "Transient phase marker injected from --mark-file" },
    "run_facts": { "type": "object", "description": "Platform build hash, kernel version, dpkg firmware" },
    "host": { "type": "object", "description": "Board model, SoC, architecture, NPU TOPS rating" },
    "cpu": { "type": "object", "description": "Combined and per-core CPU utilization percentages and clusters" },
    "memory": { "type": "object", "description": "Total, used, free RAM and swap/zram statistics" },
    "gpu": { "type": "object", "description": "Adreno GPU devfreq clock, governor, and duty cycle" },
    "hexagon_npu": { "type": "object", "description": "Qualcomm Hexagon NPU states, VRAM, and FastRPC processes" },
    "action_brain": { "type": "object", "description": "STM32H5 MCU coprocessor telemetry, UART, and CAN-FD links" },
    "thermals": { "type": "object", "description": "Consolidated 6 thermal zones" },
    "power_fan": { "type": "object", "description": "TI INA232 hardware power and EMC2305 fan metrics" },
    "stats": { "type": "object", "description": "System uptime and load averages" },
    "tracked_processes": { "type": ["array", "null"], "description": "Tracked processes with Big vs Little CPU allocation" },
    "events": { "type": "array", "description": "Process lifecycle events during this sample interval" }
  },
  "required": [
    "timestamp_unix",
    "timestamp_unix_ms",
    "host",
    "cpu",
    "memory",
    "gpu",
    "hexagon_npu",
    "action_brain",
    "thermals",
    "power_fan",
    "stats",
    "events"
  ]
}"#
}

/// Print comprehensive JSON schema and field dictionary to stdout
pub fn print_schema() {
    println!(
        "VENTOP-AI TELEMETRY SCHEMA & FIELD DICTIONARY (v{})",
        env!("CARGO_PKG_VERSION")
    );
    println!("================================================================================");
    println!("{:<32} {:<12} {}", "FIELD", "TYPE", "DESCRIPTION");
    println!("────────────────────────────────────────────────────────────────────────────────");
    println!(
        "{:<32} {:<12} {}",
        "timestamp_unix", "u64", "Unix epoch timestamp in seconds"
    );
    println!(
        "{:<32} {:<12} {}",
        "timestamp_unix_ms", "u64", "Unix epoch timestamp in milliseconds"
    );
    println!(
        "{:<32} {:<12} {}",
        "interval_ms", "f64?", "Measured stream tick interval in milliseconds"
    );
    println!(
        "{:<32} {:<12} {}",
        "marker", "string?", "Transient phase marker injected from --mark-file into .marker"
    );
    println!(
        "{:<32} {:<12} {}",
        "run_facts", "object", "Platform build hash, kernel version, dpkg firmware"
    );
    println!(
        "{:<32} {:<12} {}",
        "host", "object", "Board model, SoC, architecture, NPU TOPS rating"
    );
    println!(
        "{:<32} {:<12} {}",
        "cpu.total_load_pct", "f32", "Combined 8-core CPU utilization percentage"
    );
    println!(
        "{:<32} {:<12} {}",
        "cpu.big_cluster_load_pct", "f32", "Gold+Prime cluster load (A78C cores 0-3)"
    );
    println!(
        "{:<32} {:<12} {}",
        "cpu.little_cluster_load_pct", "f32", "Silver cluster load (A55 cores 4-7)"
    );
    println!(
        "{:<32} {:<12} {}",
        "cpu.per_core_pct", "array", "Individual CPU core utilization percentages [0..7]"
    );
    println!(
        "{:<32} {:<12} {}",
        "cpu.per_core_freq_mhz", "array", "Live scaling frequencies per core in MHz"
    );
    println!(
        "{:<32} {:<12} {}",
        "cpu.clusters[]", "array", "Cluster topology (frequencies, governors, load)"
    );
    println!(
        "{:<32} {:<12} {}",
        "cpu.user_pct", "f64", "Interval CPU user space utilization % (delta; sums to 100%)"
    );
    println!(
        "{:<32} {:<12} {}",
        "cpu.system_pct", "f64", "Interval CPU kernel & interrupt % (delta; sums to 100%)"
    );
    println!(
        "{:<32} {:<12} {}",
        "cpu.iowait_pct", "f64", "Interval CPU I/O wait % (delta; sums to 100%)"
    );
    println!(
        "{:<32} {:<12} {}",
        "cpu.idle_pct", "f64", "Interval CPU idle % (delta; sums to 100%)"
    );
    println!(
        "{:<32} {:<12} {}",
        "memory.total_bytes", "u64", "Total physical system RAM in bytes"
    );
    println!(
        "{:<32} {:<12} {}",
        "memory.used_bytes", "u64", "Physical RAM allocated excluding page cache"
    );
    println!(
        "{:<32} {:<12} {}",
        "memory.available_bytes", "u64", "Available memory from /proc/meminfo MemAvailable"
    );
    println!(
        "{:<32} {:<12} {}",
        "memory.free_bytes", "u64", "Completely unallocated physical RAM in bytes"
    );
    println!(
        "{:<32} {:<12} {}",
        "memory.swap_total_bytes", "u64", "Total swap space in bytes"
    );
    println!(
        "{:<32} {:<12} {}",
        "memory.swap_used_bytes", "u64", "Allocated swap space in bytes"
    );
    println!(
        "{:<32} {:<12} {}",
        "memory.zram", "object?", "ZRAM compressed RAM block device statistics"
    );
    println!(
        "{:<32} {:<12} {}",
        "gpu.cur_freq_mhz", "u32?", "Adreno 623 GPU devfreq clock in MHz"
    );
    println!(
        "{:<32} {:<12} {}",
        "gpu.governor", "string?", "Adreno GPU devfreq scaling governor"
    );
    println!(
        "{:<32} {:<12} {}",
        "gpu.available_frequencies_mhz", "array", "Supported GPU devfreq clock steps in MHz"
    );
    println!(
        "{:<32} {:<12} {}",
        "gpu.duty_cycle_pct", "f32?", "Adreno GPU active processing duty cycle percentage"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.remoteproc_state",
        "string",
        "Qualcomm CDSP remoteproc state (running/offline)"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.activity_status",
        "string",
        "Inference state (Idle / Offline / Active (Token Generation) / Active (Prompt Processing))"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.activity_source",
        "string",
        "'engine_slots' (HTTP /slots) or 'hardware_heuristics'"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.polling_cpu_pct",
        "f32",
        "Host CPU % spent in dspqueue polling thread (0.0=idle)"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.power_above_idle_w",
        "f64",
        "Live power delta above idle baseline in Watts (>= 0.0)"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.power_idle_baseline_w",
        "f64",
        "Board idle power baseline in Watts (default 11.3 W)"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.dma_buf_allocated_mb",
        "u64?",
        "Exact DMA-BUF CDSP context allocation in MB (from debugfs)"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.vram_allocated_mb",
        "u64?",
        "VRAM allocation in MB (DMA-BUF or genuine client RSS)"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.vram_is_estimated", "bool", "True if VRAM reflects unprivileged process RSS"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.throttle_level",
        "u32?",
        "CDSP thermal cooling throttling level (0..4; null if absent)"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.silicon_thermals",
        "object",
        "Multi-zone thermals: vector core, HMX matrix, VTCM"
    );
    println!(
        "{:<32} {:<12} {}",
        "hexagon_npu.active_processes[]",
        "array",
        "Genuine FastRPC CDSP workloads (excluding system daemons)"
    );
    println!(
        "{:<32} {:<12} {}",
        "action_brain.mcu_model", "string", "Coprocessor model (STM32H5F5 Arm Cortex-M33 @ 250MHz)"
    );
    println!(
        "{:<32} {:<12} {}",
        "action_brain.serial_device",
        "string",
        "STM32H5 CDC-ACM serial link (/dev/ttySTM0 or Not Connected)"
    );
    println!(
        "{:<32} {:<12} {}",
        "action_brain.router_socket_active",
        "bool",
        "arduino-router.service unix socket active listener"
    );
    println!(
        "{:<32} {:<12} {}",
        "action_brain.can_interfaces[]", "array", "CAN-FD network link states (can0, can1, can2)"
    );
    println!(
        "{:<32} {:<12} {}",
        "thermals", "object", "Consolidated 6 thermal zones (Gold, Silver, GPU, NPU)"
    );
    println!(
        "{:<32} {:<12} {}",
        "power_fan.board_power_watts", "f64?", "TI INA232 hardware board power in Watts"
    );
    println!(
        "{:<32} {:<12} {}",
        "power_fan.power_above_idle_watts",
        "f64",
        "Board power delta above idle baseline in Watts (>= 0.0)"
    );
    println!(
        "{:<32} {:<12} {}",
        "power_fan.board_voltage_v", "f64?", "TI INA232 live bus voltage in Volts"
    );
    println!(
        "{:<32} {:<12} {}",
        "power_fan.board_current_a", "f64?", "TI INA232 live bus current in Amperes"
    );
    println!(
        "{:<32} {:<12} {}",
        "power_fan.fan_rpm", "u32?", "EMC2305 PWM cooling fan speed in RPM"
    );
    println!(
        "{:<32} {:<12} {}",
        "tracked_processes[]", "array?", "Processes tracked via --proc with Big vs Little CPU %"
    );
    println!(
        "{:<32} {:<12} {}",
        "events[]", "array", "Process lifecycle transitions (started/exited PIDs) in tick"
    );
    println!("================================================================================");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_completeness_dictionary() {
        // Test that print_schema() runs without error
        print_schema();
    }

    #[test]
    fn test_get_schema_json_validity() {
        let schema_str = get_schema_json();
        let v: serde_json::Value =
            serde_json::from_str(schema_str).expect("schema must be valid JSON");
        assert_eq!(v["title"], "VentopSnapshot");
        assert!(v["properties"]["hexagon_npu"].is_object());
    }
}
