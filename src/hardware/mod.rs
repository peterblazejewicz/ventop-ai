// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

pub mod cpu;
pub mod facts;
pub mod gpu;
pub mod power;
pub mod thermals;

pub use cpu::{
    get_core_cluster_label, get_cpu_architecture, get_cpu_clusters_with_load, get_cpu_frequencies,
    is_big_core, CpuCluster,
};
pub use facts::{get_board_name, get_platform_run_facts, get_soc_model, PlatformRunFacts};
pub use gpu::{get_adreno_gpu_metrics, AdrenoGpuMetrics};
pub use power::{
    calibrate_idle_baseline, get_hardware_power_fan, get_idle_baseline_watts,
    set_idle_baseline_watts, HardwarePowerFan,
};
pub use thermals::{get_consolidated_thermals, VentunoThermals};
