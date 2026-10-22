// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::System;

use crate::action_brain::collect_action_brain_snapshot;
use crate::hardware::{
    get_adreno_gpu_metrics, get_board_name, get_consolidated_thermals, get_cpu_architecture,
    get_cpu_clusters_with_load, get_cpu_frequencies, get_hardware_power_fan,
    get_platform_run_facts, get_soc_model,
};
use crate::hexagon::collect_hexagon_snapshot_with_power;
use crate::sysinfo_ext::{
    get_cpu_stats, get_tracked_processes_with_predicate_opts, get_zram_info, is_ventuno_big_core,
    CpuStats,
};

use super::summary::SummaryTracker;
use super::types::{
    CpuSnapshot, HostInfo, MemorySnapshot, ProcessLifecycleEvent, StatsSnapshot, VentopSnapshot,
};

static PREV_CPU_STATS: Mutex<Option<CpuStats>> = Mutex::new(None);
static TERMINATING: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn sig_handler(_: libc::c_int) {
    TERMINATING.store(true, Ordering::SeqCst);
}

/// Reset the global previous CPU stats tracker (useful for deterministic fixture tests)
pub fn reset_prev_cpu_stats() {
    let mut lock = PREV_CPU_STATS.lock().unwrap();
    *lock = None;
}

/// Compute interval delta CPU percentages from current and previous /proc/stat tick counters.
/// Attributes hardware/software interrupt ticks (irq and softirq) to `system_pct`
/// so that `user_pct + system_pct + iowait_pct + idle_pct` mathematically sums to 100.0%.
pub fn compute_cpu_pct_from_stats(
    curr: &CpuStats,
    prev: Option<&CpuStats>,
) -> (f64, f64, f64, f64) {
    let (d_user, d_nice, d_system, d_idle, d_iowait, d_irq, d_softirq) = match prev {
        Some(p) => (
            curr.user.saturating_sub(p.user),
            curr.nice.saturating_sub(p.nice),
            curr.system.saturating_sub(p.system),
            curr.idle.saturating_sub(p.idle),
            curr.iowait.saturating_sub(p.iowait),
            curr.irq.saturating_sub(p.irq),
            curr.softirq.saturating_sub(p.softirq),
        ),
        None => (
            curr.user,
            curr.nice,
            curr.system,
            curr.idle,
            curr.iowait,
            curr.irq,
            curr.softirq,
        ),
    };

    let total_delta = d_user + d_nice + d_system + d_idle + d_iowait + d_irq + d_softirq;
    if total_delta == 0 {
        return (0.0, 0.0, 0.0, 100.0);
    }

    let total = total_delta as f64;
    let user_pct = ((d_user + d_nice) as f64 / total) * 100.0;
    let system_pct = ((d_system + d_irq + d_softirq) as f64 / total) * 100.0;
    let iowait_pct = (d_iowait as f64 / total) * 100.0;
    let idle_pct = (d_idle as f64 / total) * 100.0;

    (user_pct, system_pct, iowait_pct, idle_pct)
}

/// Collect a complete snapshot of all Ventuno Q Dual-Brain telemetry
pub fn collect_snapshot(
    sys: &mut System,
    tracked_patterns: &[String],
    include_threads: bool,
) -> VentopSnapshot {
    if !tracked_patterns.is_empty() {
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    }
    sys.refresh_cpu_all();
    std::thread::sleep(Duration::from_millis(250));
    sys.refresh_cpu_all();
    sys.refresh_memory();
    if !tracked_patterns.is_empty() {
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    }

    build_snapshot_internal(sys, tracked_patterns, include_threads)
}

/// Fast continuous snapshot collector without sleep delay
pub fn collect_snapshot_continuous(
    sys: &System,
    tracked_patterns: &[String],
    include_threads: bool,
) -> VentopSnapshot {
    build_snapshot_internal(sys, tracked_patterns, include_threads)
}

fn build_snapshot_internal(
    sys: &System,
    tracked_patterns: &[String],
    include_threads: bool,
) -> VentopSnapshot {
    let now = SystemTime::now();
    let duration = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let timestamp_unix = duration.as_secs();
    let timestamp_unix_ms = duration.as_millis() as u64;

    let host = HostInfo {
        board: get_board_name(),
        soc: get_soc_model(),
        hostname: System::host_name().unwrap_or_else(|| "ventuno".to_string()),
        kernel: System::kernel_version().unwrap_or_else(|| "unknown".to_string()),
        architecture: get_cpu_architecture(),
        nsp_tops: 40,
        mcu_coprocessor: "STM32H5F5 Cortex-M33 @ 250MHz".to_string(),
    };

    let cpus = sys.cpus();
    let total_load_pct = if !cpus.is_empty() {
        cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
    } else {
        0.0
    };

    let per_core_pct: Vec<f32> = cpus.iter().map(|c| c.cpu_usage()).collect();
    let per_core_freq_mhz = get_cpu_frequencies();
    let clusters = get_cpu_clusters_with_load(&per_core_pct);

    let (big_cluster_load_pct, little_cluster_load_pct) = if !per_core_pct.is_empty() {
        let mut big_sum = 0.0f32;
        let mut big_count = 0usize;
        let mut little_sum = 0.0f32;
        let mut little_count = 0usize;
        for (idx, &load) in per_core_pct.iter().enumerate() {
            if is_ventuno_big_core(idx as u32) {
                big_sum += load;
                big_count += 1;
            } else {
                little_sum += load;
                little_count += 1;
            }
        }
        (
            if big_count > 0 {
                big_sum / big_count as f32
            } else {
                0.0
            },
            if little_count > 0 {
                little_sum / little_count as f32
            } else {
                0.0
            },
        )
    } else {
        (0.0, 0.0)
    };

    let cpu_stats = get_cpu_stats();
    let (user_pct, system_pct, iowait_pct, idle_pct) = {
        let mut lock = PREV_CPU_STATS.lock().unwrap();
        let pcts = compute_cpu_pct_from_stats(&cpu_stats, lock.as_ref());
        *lock = Some(cpu_stats);
        pcts
    };

    let cpu = CpuSnapshot {
        total_load_pct: (total_load_pct * 10.0).round() / 10.0,
        big_cluster_load_pct: (big_cluster_load_pct * 10.0).round() / 10.0,
        little_cluster_load_pct: (little_cluster_load_pct * 10.0).round() / 10.0,
        per_core_pct,
        per_core_freq_mhz,
        clusters,
        user_pct,
        system_pct,
        iowait_pct,
        idle_pct,
    };

    let memory = MemorySnapshot {
        total_bytes: sys.total_memory(),
        used_bytes: sys.used_memory(),
        available_bytes: sys.available_memory(),
        free_bytes: sys.free_memory(),
        swap_total_bytes: sys.total_swap(),
        swap_used_bytes: sys.used_swap(),
        zram: get_zram_info(),
    };

    let gpu = get_adreno_gpu_metrics();
    let power_fan = get_hardware_power_fan();
    let hexagon_npu = collect_hexagon_snapshot_with_power(power_fan.board_power_watts);
    let action_brain = collect_action_brain_snapshot();
    let thermals = get_consolidated_thermals();

    let load_avg = System::load_average();
    let stats = StatsSnapshot {
        uptime_secs: System::uptime(),
        load_avg_1m: load_avg.one,
        load_avg_5m: load_avg.five,
        load_avg_15m: load_avg.fifteen,
    };

    // On Ventuno Q: Cores 0-3 are Big (A78C Gold/Prime), Cores 4-7 are Little (A55 Silver)
    let tracked_processes = if !tracked_patterns.is_empty() {
        Some(get_tracked_processes_with_predicate_opts(
            sys,
            tracked_patterns,
            is_ventuno_big_core,
            include_threads,
        ))
    } else {
        None
    };

    VentopSnapshot {
        timestamp_unix,
        timestamp_unix_ms,
        interval_ms: None,
        marker: None,
        run_facts: get_platform_run_facts(),
        host,
        cpu,
        memory,
        gpu,
        hexagon_npu,
        action_brain,
        thermals,
        power_fan,
        stats,
        tracked_processes,
        events: Vec::new(),
    }
}

/// Output a single JSON snapshot to stdout and exit
pub fn print_json_snapshot(tracked_patterns: &[String], include_threads: bool) {
    let mut sys = System::new_all();
    let snapshot = collect_snapshot(&mut sys, tracked_patterns, include_threads);
    if let Ok(json) = serde_json::to_string_pretty(&snapshot) {
        println!("{}", json);
    }
}

/// Output a single human-readable plaintext snapshot to stdout and exit
pub fn print_oneshot_snapshot(tracked_patterns: &[String], include_threads: bool) {
    let mut sys = System::new_all();
    let s = collect_snapshot(&mut sys, tracked_patterns, include_threads);

    println!("================================================================================");
    println!(
        "  VENTOP-AI TELEMETRY SNAPSHOT - {} ({})",
        s.host.board, s.host.soc
    );
    println!("================================================================================");
    println!(
        "Kernel: {} | Arch: {} | Uptime: {}s",
        s.host.kernel, s.host.architecture, s.stats.uptime_secs
    );
    println!(
        "Load Avg: {:.2}, {:.2}, {:.2}",
        s.stats.load_avg_1m, s.stats.load_avg_5m, s.stats.load_avg_15m
    );
    println!();
    println!("--- CPU CLUSTERS (8 Cores) ---");
    for cluster in &s.cpu.clusters {
        println!(
            "  {}: {} MHz [governor: {}]",
            cluster.name, cluster.cur_freq_mhz, cluster.governor
        );
    }
    println!(
        "  Total CPU Load: {:.1}% (User: {:.1}%, Sys: {:.1}%, Idle: {:.1}%)",
        s.cpu.total_load_pct, s.cpu.user_pct, s.cpu.system_pct, s.cpu.idle_pct
    );
    println!();
    println!("--- THE AI BRAIN: Qualcomm Hexagon NPU (40 TOPS) ---");
    println!(
        "  Core: {} ({}) | Activity: {} | GLINK IRQs: {}/s",
        s.hexagon_npu.core_id,
        s.hexagon_npu.arch_version,
        s.hexagon_npu.activity_status,
        s.hexagon_npu.glink_interrupts_per_sec.unwrap_or(0)
    );
    println!(
        "  Silicon Units: {}x HW Threads | {}x HVX (1024-bit) | {}x HMX Matrix | {} MB VTCM",
        s.hexagon_npu.hw_threads,
        s.hexagon_npu.hvx_units,
        s.hexagon_npu.hmx_units,
        s.hexagon_npu.vtcm_sram_mb
    );
    let therm = &s.hexagon_npu.silicon_thermals;
    println!(
        "  Silicon Thermals: Core/HVX: {} | Matrix: {} | VTCM: {} (Peak: {})",
        therm
            .core_vector_celsius
            .map(|t| format!("{}°C", t))
            .unwrap_or_else(|| "-".to_string()),
        therm
            .matrix_hmx_celsius
            .map(|t| format!("{}°C", t))
            .unwrap_or_else(|| "-".to_string()),
        therm
            .vtcm_sram_celsius
            .map(|t| format!("{}°C", t))
            .unwrap_or_else(|| "-".to_string()),
        therm
            .peak_celsius
            .map(|t| format!("{}°C", t))
            .unwrap_or_else(|| "-".to_string())
    );
    let vram_type = if s.hexagon_npu.vram_is_estimated {
        "RSS estimated"
    } else {
        "DMA-BUF"
    };
    let vram_str = s
        .hexagon_npu
        .vram_allocated_mb
        .map(|m| format!("{} MB", m))
        .unwrap_or_else(|| {
            if s.hexagon_npu.is_available {
                "Dynamic".to_string()
            } else {
                "N/A".to_string()
            }
        });
    let throttle_str = s
        .hexagon_npu
        .throttle_level
        .map(|t| format!("{}/4", t))
        .unwrap_or_else(|| "N/A".to_string());
    println!(
        "  Status: {} | VRAM Allocated ({}): {} | Throttle Level: {}",
        s.hexagon_npu.remoteproc_state, vram_type, vram_str, throttle_str
    );
    if s.hexagon_npu.power_above_idle_w > 0.0 {
        println!(
            "  Inference Power: +{:.1} W above idle (baseline: {:.1} W)",
            s.hexagon_npu.power_above_idle_w, s.hexagon_npu.power_idle_baseline_w
        );
    }
    if s.hexagon_npu.polling_cpu_pct > 0.0 {
        println!("  Polling Host CPU: {:.1}%", s.hexagon_npu.polling_cpu_pct);
    }
    if !s.hexagon_npu.active_processes.is_empty() {
        println!("  Active FastRPC Workloads:");
        for p in &s.hexagon_npu.active_processes {
            println!(
                "    - PID {}: {} [RSS: {} MB] (Model: {})",
                p.pid,
                p.name,
                p.rss_mb,
                p.model_name.as_deref().unwrap_or("Unknown")
            );
            let poll_str = if p.polling_cpu_pct > 0.0 {
                format!(" | poll_cpu={:.1}%", p.polling_cpu_pct)
            } else {
                String::new()
            };
            println!("      Offload: dev={} | layers={:?} | threads={:?} | flash_attn={} | ctx={:?} | port={:?}{}",
                     p.device, p.n_gpu_layers, p.n_threads, p.flash_attn, p.context_size, p.port, poll_str);
            if let Some(ref em) = p.engine_metrics {
                println!("      Engine Metrics: prompt={:.1} TPS | decode={:.1} TPS | cached_tokens={} | slot_state={}",
                         em.prompt_tokens_per_sec, em.decode_tokens_per_sec, em.prompt_tokens_cached, em.slot_state);
            }
            if p.devices.len() > 1 {
                println!("      Open FastRPC Nodes: {}", p.devices.join(", "));
            }
        }
    } else {
        println!("  Active FastRPC Workloads: None");
    }
    println!();
    println!("--- THE ACTION BRAIN: STM32H5 Real-Time Coprocessor ---");
    println!(
        "  MCU: {} | RTOS: {}",
        s.action_brain.mcu_model, s.action_brain.rtos
    );
    println!(
        "  Serial Link: {} ({} baud) | router.sock: {}",
        s.action_brain.serial_device,
        s.action_brain.baudrate,
        if s.action_brain.router_socket_active {
            "ACTIVE"
        } else {
            "INACTIVE"
        }
    );
    if !s.action_brain.can_interfaces.is_empty() {
        print!("  CAN Interfaces: ");
        for can in &s.action_brain.can_interfaces {
            print!("{}: {}  ", can.name, if can.is_up { "UP" } else { "DOWN" });
        }
        println!();
    }
    println!();
    println!("--- ADRENO 623 GPU ---");
    let clock_str = s
        .gpu
        .cur_freq_mhz
        .map(|f| format!("{} MHz", f))
        .unwrap_or_else(|| "N/A".to_string());
    println!(
        "  Clock: {} | Governor: {}",
        clock_str,
        s.gpu.governor.as_deref().unwrap_or("N/A")
    );
    println!();
    println!("--- HARDWARE POWER & FAN ---");
    let pwr_str = s
        .power_fan
        .board_power_watts
        .map(|p| format!("{:.2} W", p))
        .unwrap_or_else(|| "N/A".to_string());
    let volt_str = s
        .power_fan
        .board_voltage_v
        .map(|v| format!("{:.2} V", v))
        .unwrap_or_else(|| "N/A".to_string());
    let curr_str = s
        .power_fan
        .board_current_a
        .map(|c| format!("{:.2} A", c))
        .unwrap_or_else(|| "N/A".to_string());
    println!(
        "  Board Power: {} | Voltage: {} | Current: {} ({})",
        pwr_str,
        volt_str,
        curr_str,
        s.power_fan.power_device.as_deref().unwrap_or("N/A")
    );
    println!(
        "  Fan Speed: {} ({})",
        s.power_fan
            .fan_rpm
            .map(|r| format!("{} RPM", r))
            .unwrap_or_else(|| "N/A".to_string()),
        s.power_fan.fan_device.as_deref().unwrap_or("N/A")
    );
    println!();
    println!("--- THERMALS (6 Consolidated Zones) ---");
    println!(
        "  CPU Gold: {}°C | CPU Silver: {}°C | GPU: {}°C | NPU: {}°C | RAM: {}°C | Board: {}°C",
        s.thermals
            .cpu_gold_celsius
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".to_string()),
        s.thermals
            .cpu_silver_celsius
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".to_string()),
        s.thermals
            .gpu_celsius
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".to_string()),
        s.thermals
            .npu_nsp_celsius
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".to_string()),
        s.thermals
            .ram_celsius
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".to_string()),
        s.thermals
            .pmic_board_celsius
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!();
    println!("--- MEMORY ---");
    println!(
        "  RAM Used: {:.2} / {:.2} GB ({:.1}%) | Available: {:.2} GB | Free: {:.2} GB",
        s.memory.used_bytes as f64 / 1_073_741_824.0,
        s.memory.total_bytes as f64 / 1_073_741_824.0,
        (s.memory.used_bytes as f64 / s.memory.total_bytes as f64) * 100.0,
        s.memory.available_bytes as f64 / 1_073_741_824.0,
        s.memory.free_bytes as f64 / 1_073_741_824.0
    );

    if let Some(ref procs) = s.tracked_processes {
        println!();
        println!("--- TRACKED PROCESSES & CORE PLACEMENT ---");
        for p in procs {
            println!(
                "  PID {}: {} (CPU: {:.1}% [Big: {:.1}%, Little: {:.1}%], RSS: {} MB, Threads: {})",
                p.pid,
                p.name,
                p.cpu_pct,
                p.big_cores_cpu_pct,
                p.little_cores_cpu_pct,
                p.rss_mb,
                p.thread_count
            );
            println!(
                "    Core Distribution: Big (Gold/Prime): {} threads | Little (Silver): {} threads",
                p.big_cores_count, p.little_cores_count
            );
            if !p.threads.is_empty() {
                for t in &p.threads {
                    let cluster_label = crate::hardware::get_core_cluster_label(t.cpu_core);
                    println!(
                        "      TID {}: {} -> Core {} ({})",
                        t.tid, t.name, t.cpu_core, cluster_label
                    );
                }
            }
        }
    }
    println!("================================================================================");
}

pub struct StreamOptions {
    pub interval_ms: u64,
    pub tracked_patterns: Vec<String>,
    pub include_threads: bool,
    pub duration_secs: Option<u64>,
    pub summary: bool,
    pub mark_file: Option<String>,
    pub active_threshold_pct: f32,
}

/// Stream continuous NDJSON snapshots at fixed-rate intervals
pub fn stream_json_snapshots(opts: StreamOptions) {
    TERMINATING.store(false, Ordering::SeqCst);
    #[cfg(unix)]
    // SAFETY: Registering a C-ABI signal handler for SIGINT and SIGTERM.
    // The signal handler `sig_handler` only performs an atomic store (SeqCst)
    // to static `TERMINATING`, which is strictly async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, sig_handler as *const () as libc::sighandler_t);
        libc::signal(
            libc::SIGTERM,
            sig_handler as *const () as libc::sighandler_t,
        );
    }

    let mut sys = System::new_all();
    if !opts.tracked_patterns.is_empty() {
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    }
    sys.refresh_cpu_all();

    let interval = Duration::from_millis(opts.interval_ms.max(20));
    let mut last_tick_time = Instant::now();
    let mut next_tick = Instant::now();
    let start_run_time = Instant::now();

    let mut prev_tracked_pids: HashMap<u32, String> = HashMap::new();
    let mut first_iteration = true;
    let mut summary_tracker = SummaryTracker::default();

    loop {
        if TERMINATING.load(Ordering::SeqCst) {
            break;
        }

        if let Some(dur) = opts.duration_secs {
            if start_run_time.elapsed().as_secs() >= dur {
                break;
            }
        }

        let now = Instant::now();
        let elapsed_since_last = now.duration_since(last_tick_time).as_secs_f64() * 1000.0;
        last_tick_time = now;

        sys.refresh_cpu_all();
        sys.refresh_memory();
        if !opts.tracked_patterns.is_empty() {
            sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        }

        let mut snapshot =
            collect_snapshot_continuous(&sys, &opts.tracked_patterns, opts.include_threads);
        snapshot.interval_ms = Some((elapsed_since_last * 10.0).round() / 10.0);

        // Process lifecycle events
        let mut curr_tracked_pids = HashMap::new();
        if let Some(ref procs) = snapshot.tracked_processes {
            for p in procs {
                curr_tracked_pids.insert(p.pid, p.name.clone());
            }
        }
        if !first_iteration {
            let mut events = Vec::new();
            for (&pid, name) in &curr_tracked_pids {
                if !prev_tracked_pids.contains_key(&pid) {
                    events.push(ProcessLifecycleEvent {
                        event: "started",
                        pid,
                        name: name.clone(),
                        timestamp_unix_ms: snapshot.timestamp_unix_ms,
                    });
                }
            }
            for (&pid, name) in &prev_tracked_pids {
                if !curr_tracked_pids.contains_key(&pid) {
                    events.push(ProcessLifecycleEvent {
                        event: "exited",
                        pid,
                        name: name.clone(),
                        timestamp_unix_ms: snapshot.timestamp_unix_ms,
                    });
                }
            }
            snapshot.events = events;
        }
        prev_tracked_pids = curr_tracked_pids;
        first_iteration = false;

        // Check marker file
        if let Some(ref mark_path) = opts.mark_file {
            if let Ok(content) = std::fs::read_to_string(mark_path) {
                let lines: Vec<&str> = content
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .collect();
                if !lines.is_empty() {
                    snapshot.marker = Some(lines.join(" | "));
                    let _ = std::fs::OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(mark_path);
                }
            }
        }

        if opts.summary {
            summary_tracker.record_sample(&snapshot, opts.active_threshold_pct);
        }

        if let Ok(json) = serde_json::to_string(&snapshot) {
            println!("{}", json);
        }

        next_tick += interval;
        let now_after_work = Instant::now();
        if next_tick > now_after_work {
            std::thread::sleep(next_tick - now_after_work);
        } else {
            next_tick = now_after_work;
        }
    }

    if opts.summary {
        summary_tracker.print_report(
            opts.active_threshold_pct,
            start_run_time.elapsed().as_secs_f64(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_cpu_pct_from_stats_sums_to_100() {
        reset_prev_cpu_stats();
        // Initial tick (prev = None)
        let t1 = CpuStats {
            user: 10000,
            nice: 500,
            system: 2000,
            idle: 80000,
            iowait: 100,
            irq: 50,
            softirq: 20,
        };
        let (u1, s1, io1, id1) = compute_cpu_pct_from_stats(&t1, None);
        let sum1 = u1 + s1 + io1 + id1;
        assert!(
            (sum1 - 100.0).abs() < 1e-4,
            "Initial tick must sum to 100.0%: {}",
            sum1
        );

        // Consecutive tick (prev = Some(t1))
        // Deliberate deltas:
        // user: 600, nice: 0, system: 300, idle: 800, iowait: 50, irq: 40, softirq: 20
        // total delta = 1810
        let t2 = CpuStats {
            user: 10600,
            nice: 500,
            system: 2300,
            idle: 80800,
            iowait: 150,
            irq: 90,
            softirq: 40,
        };
        let (u2, s2, io2, id2) = compute_cpu_pct_from_stats(&t2, Some(&t1));
        let sum2 = u2 + s2 + io2 + id2;
        assert!(
            (sum2 - 100.0).abs() < 1e-4,
            "Consecutive interval delta must sum to 100.0%: {}",
            sum2
        );

        // Expected mathematical values:
        // user: (600 + 0) / 1810 * 100 = 33.14917...%
        // system: (300 + 40 + 20) / 1810 * 100 = 19.88950...% (includes irq + softirq!)
        // iowait: 50 / 1810 * 100 = 2.76243...%
        // idle: 800 / 1810 * 100 = 44.19889...%
        assert!((u2 - 33.149).abs() < 0.01);
        assert!((s2 - 19.889).abs() < 0.01);
        assert!((io2 - 2.762).abs() < 0.01);
        assert!((id2 - 44.198).abs() < 0.01);

        // Edge case: zero delta ticks
        let (u0, s0, io0, id0) = compute_cpu_pct_from_stats(&t2, Some(&t2));
        assert_eq!((u0, s0, io0, id0), (0.0, 0.0, 0.0, 100.0));
    }

    #[test]
    fn test_ventop_snapshot_serialization() {
        let sys = System::new();
        let snapshot = collect_snapshot_continuous(&sys, &[], false);
        let json = serde_json::to_string(&snapshot).expect("should serialize");
        assert!(json.contains("Arduino Ventuno Q") || json.contains("Snapdragon"));
        assert!(json.contains("\"available_bytes\":"));
        assert!(json.contains("\"big_cluster_load_pct\":"));
        assert!(json.contains("\"little_cluster_load_pct\":"));
        assert!(json.contains("\"run_facts\":"));
        assert!(json.contains("\"events\":"));
        assert!(json.contains("\"power_idle_baseline_w\":"));
        assert!(json.contains("\"power_above_idle_w\":"));
        // Ensure zram is omitted when None
        if snapshot.memory.zram.is_none() {
            assert!(!json.contains("\"zram\":null"));
        }
        // Ensure duty_cycle_pct is omitted when None
        if snapshot.gpu.duty_cycle_pct.is_none() {
            assert!(!json.contains("\"duty_cycle_pct\":null"));
        }
    }
}
