// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use serde::Serialize;

use crate::action_brain::ActionBrainSnapshot;
use crate::hardware::{
    AdrenoGpuMetrics, CpuCluster, HardwarePowerFan, PlatformRunFacts, VentunoThermals,
};
use crate::hexagon::HexagonNpuSnapshot;
use crate::sysinfo_ext::{ProcessAffinityProfile, ZramInfo};

#[derive(Debug, Clone, Serialize)]
pub struct ProcessLifecycleEvent {
    pub event: &'static str,
    pub pid: u32,
    pub name: String,
    pub timestamp_unix_ms: u64,
}

#[derive(Serialize)]
pub struct VentopSnapshot {
    pub timestamp_unix: u64,
    pub timestamp_unix_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker: Option<String>,
    pub run_facts: &'static PlatformRunFacts,
    pub host: HostInfo,
    pub cpu: CpuSnapshot,
    pub memory: MemorySnapshot,
    pub gpu: AdrenoGpuMetrics,
    pub hexagon_npu: HexagonNpuSnapshot,
    pub action_brain: ActionBrainSnapshot,
    pub thermals: VentunoThermals,
    pub power_fan: HardwarePowerFan,
    pub stats: StatsSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracked_processes: Option<Vec<ProcessAffinityProfile>>,
    #[serde(default)]
    pub events: Vec<ProcessLifecycleEvent>,
}

#[derive(Serialize)]
pub struct HostInfo {
    pub board: String,
    pub soc: String,
    pub hostname: String,
    pub kernel: String,
    pub architecture: String,
    pub nsp_tops: u32,
    pub mcu_coprocessor: String,
}

#[derive(Serialize)]
pub struct CpuSnapshot {
    pub total_load_pct: f32,
    pub big_cluster_load_pct: f32,
    pub little_cluster_load_pct: f32,
    pub per_core_pct: Vec<f32>,
    pub per_core_freq_mhz: Vec<u32>,
    pub clusters: Vec<CpuCluster>,
    pub user_pct: f64,
    pub system_pct: f64,
    pub iowait_pct: f64,
    pub idle_pct: f64,
}

#[derive(Serialize)]
pub struct MemorySnapshot {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub free_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zram: Option<ZramInfo>,
}

#[derive(Serialize)]
pub struct StatsSnapshot {
    pub uptime_secs: u64,
    pub load_avg_1m: f64,
    pub load_avg_5m: f64,
    pub load_avg_15m: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatSummary<T> {
    pub min: T,
    pub median: T,
    pub p90: T,
    pub max: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpuPhase {
    Idle,
    PromptProcessing,
    TokenGeneration,
    OtherActive,
}

impl NpuPhase {
    pub fn from_activity_status(status: &str) -> Self {
        if status.contains("Prompt") {
            NpuPhase::PromptProcessing
        } else if status.contains("Token Generation") || status.contains("Generating") {
            NpuPhase::TokenGeneration
        } else if status.starts_with("Idle") {
            NpuPhase::Idle
        } else {
            NpuPhase::OtherActive
        }
    }
}

#[derive(Debug, Clone)]
pub struct SampleTelemetry {
    pub interval_secs: f64,
    pub power_w: Option<f64>,
    pub phase: NpuPhase,
    pub is_heuristic: bool,
    pub prompt_tokens_processed: u32,
    pub tokens_decoded: u32,
    pub completed_task_tokens: Option<u32>,
    pub cpu_big: f32,
    pub cpu_little: f32,
}

#[derive(Debug, Clone)]
pub struct MarkerRecord {
    pub label: String,
    pub sample_idx: usize,
}

pub type ProcessSnapshot = ProcessAffinityProfile;
