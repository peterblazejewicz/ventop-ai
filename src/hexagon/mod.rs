// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

pub mod dmabuf;
pub mod fastrpc;
pub mod glink;
pub mod inference;
pub mod thermals;

pub use dmabuf::{parse_cdsp_dma_buf_from_str, parse_cdsp_dma_buf_mb};
pub use fastrpc::{scan_fastrpc_processes, FastRpcProcess};
pub use glink::{
    get_glink_interrupt_rate, parse_glink_interrupt_count_from_str, read_glink_interrupt_count,
};
pub use inference::{
    evaluate_inference_activity, parse_llm_args_from_cmdline, query_llama_slots,
    LlamaEngineMetrics, LlamaSlotsMetrics,
};
pub use thermals::{get_cdsp_throttle_state, get_nsp_silicon_thermals, NspSiliconThermals};

use crate::file_cache::{read_cached_file, resolve_sysfs_path};
use std::fs;

/// Telemetry snapshot of the Qualcomm Hexagon Tensor NPU (HTP)
#[derive(Debug, Clone, serde::Serialize)]
pub struct HexagonNpuSnapshot {
    pub is_available: bool,
    pub remoteproc_state: String,
    pub core_id: &'static str,      // "HTP0"
    pub arch_version: &'static str, // "Hexagon v75 (HTP)"
    pub tops_rating: u32,           // 40 TOPS
    pub hw_threads: u32,            // 4 threads (Scalar)
    pub hvx_units: u32,             // 4x HVX (1024-bit vector engines)
    pub hmx_units: u32,             // 1x HMX (Matrix multiplication engine)
    pub vtcm_sram_mb: u32,          // 8 MB On-chip VTCM
    pub power_domain: &'static str, // "nsp0"
    pub active_processes: Vec<FastRpcProcess>,
    pub dma_buf_allocated_mb: Option<u64>,
    pub vram_allocated_mb: Option<u64>,
    pub vram_is_estimated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glink_interrupts_per_sec: Option<u64>,
    pub activity_status: &'static str,
    pub activity_source: &'static str, // "engine_slots" or "hardware_heuristics"
    pub polling_cpu_pct: f32,
    pub power_above_idle_w: f64,
    pub power_idle_baseline_w: f64,
    pub throttle_level: Option<u32>,
    pub silicon_thermals: NspSiliconThermals,
}

impl Default for HexagonNpuSnapshot {
    fn default() -> Self {
        Self {
            is_available: false,
            remoteproc_state: "offline".to_string(),
            core_id: "HTP0",
            arch_version: "Hexagon v75 (HTP)",
            tops_rating: 40,
            hw_threads: 4,
            hvx_units: 4,
            hmx_units: 1,
            vtcm_sram_mb: 8,
            power_domain: "nsp0",
            active_processes: Vec::new(),
            dma_buf_allocated_mb: None,
            vram_allocated_mb: None,
            vram_is_estimated: false,
            glink_interrupts_per_sec: None,
            activity_status: "Idle",
            activity_source: "hardware_heuristics",
            polling_cpu_pct: 0.0,
            power_above_idle_w: 0.0,
            power_idle_baseline_w: 11.3,
            throttle_level: None,
            silicon_thermals: NspSiliconThermals::default(),
        }
    }
}

/// Read board power in Watts from INA232 hardware monitor
fn get_current_board_power_w() -> Option<f64> {
    crate::hardware::get_hardware_power_fan().board_power_watts
}

/// Dynamically find and check the CDSP remoteproc state
pub fn get_cdsp_remoteproc_state() -> (bool, String) {
    let remoteproc_dir = resolve_sysfs_path("/sys/class/remoteproc");
    if let Ok(entries) = fs::read_dir(&remoteproc_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let mut is_cdsp = false;

            let firmware_path = path.join("firmware");
            if let Ok(fw) = fs::read_to_string(&firmware_path) {
                if fw.to_lowercase().contains("cdsp") {
                    is_cdsp = true;
                }
            }

            if !is_cdsp {
                let name_path = path.join("name");
                if let Ok(name) = fs::read_to_string(&name_path) {
                    if name.to_lowercase().contains("cdsp") {
                        is_cdsp = true;
                    }
                }
            }

            if is_cdsp {
                let state_path = path.join("state");
                if let Ok(state) = read_cached_file(&state_path.to_string_lossy()) {
                    let state_trimmed = state.trim().to_string();
                    return (state_trimmed == "running", state_trimmed);
                }
            }
        }
    }

    (false, "offline".to_string())
}

/// Collect full telemetry snapshot of the Hexagon NPU
pub fn collect_hexagon_snapshot() -> HexagonNpuSnapshot {
    let (is_running, remoteproc_state) = get_cdsp_remoteproc_state();
    let dev_cdsp = resolve_sysfs_path("/dev/fastrpc-cdsp");
    let dev_cdsp_sec = resolve_sysfs_path("/dev/fastrpc-cdsp-secure");
    let dev_exists = dev_cdsp.exists() || dev_cdsp_sec.exists();
    let is_available = is_running && dev_exists;

    let active_processes = scan_fastrpc_processes();

    // Dual VRAM strategy: exact debugfs DMA-BUF or unprivileged LLM RSS estimation
    let mut vram_is_estimated = false;
    let dma_buf_allocated_mb = parse_cdsp_dma_buf_mb();

    let vram_allocated_mb = if dma_buf_allocated_mb.is_some() {
        dma_buf_allocated_mb
    } else if !active_processes.is_empty() {
        let sum_rss: u64 = active_processes.iter().map(|p| p.rss_mb).sum();
        if sum_rss > 0 {
            vram_is_estimated = true;
            Some(sum_rss)
        } else {
            None
        }
    } else {
        None
    };

    let glink_interrupts_per_sec = get_glink_interrupt_rate();
    let glink_interrupts_opt = if glink_interrupts_per_sec > 0 {
        Some(glink_interrupts_per_sec)
    } else {
        None
    };

    let throttle_level = get_cdsp_throttle_state();
    let silicon_thermals = get_nsp_silicon_thermals();
    let peak_temp = silicon_thermals.peak_celsius.unwrap_or(0);

    let current_power_w = get_current_board_power_w();
    let idle_baseline_w = crate::hardware::get_idle_baseline_watts();
    let raw_power_delta = current_power_w
        .map(|w| (w - idle_baseline_w).max(0.0))
        .unwrap_or(0.0);
    let power_above_idle_w = (raw_power_delta * 10.0).round() / 10.0;

    let max_polling_cpu = active_processes
        .iter()
        .map(|p| p.polling_cpu_pct)
        .fold(0.0f32, f32::max);

    // 1. Check if engine metrics (llama-server /slots) explicitly reports slot state
    let mut engine_state: Option<(&'static str, bool)> = None; // (state, is_cached)

    for p in &active_processes {
        if let Some(ref em) = p.engine_metrics {
            let is_cached = em.age_ms.map(|a| a > 0).unwrap_or(false);
            if em.slot_state == "processing_prompt" {
                engine_state = Some(("Active (Prompt Processing)", is_cached));
                break;
            } else if em.slot_state == "generating_tokens" {
                engine_state = Some(("Active (Token Generation)", is_cached));
            } else if em.slot_state == "idle" && engine_state.is_none() {
                engine_state = Some(("Idle (Model Loaded)", is_cached));
            }
        }
    }

    let (activity_status, activity_source) = if !is_running {
        ("Offline", "hardware_heuristics")
    } else if let Some((state, is_cached)) = engine_state {
        // Hardware quorum gate for cached results during timeouts:
        // If cached state said Active, but hardware is completely quiescent (power < 1.5W above idle, poll CPU < 5%, 0 irqs),
        // then the request completed; do not phantom-latch!
        let hardware_silent =
            raw_power_delta < 1.5 && max_polling_cpu < 5.0 && glink_interrupts_per_sec == 0;
        if is_cached && hardware_silent && state.starts_with("Active") {
            ("Idle (Model Loaded)", "engine_slots")
        } else {
            (state, "engine_slots")
        }
    } else {
        let (status, _) = evaluate_inference_activity(
            max_polling_cpu,
            raw_power_delta,
            peak_temp,
            glink_interrupts_per_sec,
            !active_processes.is_empty(),
        );
        (status, "hardware_heuristics")
    };

    HexagonNpuSnapshot {
        is_available,
        remoteproc_state,
        core_id: "HTP0",
        arch_version: "Hexagon v75 (HTP)",
        tops_rating: 40,
        hw_threads: 4,
        hvx_units: 4,
        hmx_units: 1,
        vtcm_sram_mb: 8,
        power_domain: "nsp0",
        active_processes,
        dma_buf_allocated_mb,
        vram_allocated_mb,
        vram_is_estimated,
        glink_interrupts_per_sec: glink_interrupts_opt,
        activity_status,
        activity_source,
        polling_cpu_pct: max_polling_cpu,
        power_above_idle_w,
        power_idle_baseline_w: idle_baseline_w,
        throttle_level,
        silicon_thermals,
    }
}
