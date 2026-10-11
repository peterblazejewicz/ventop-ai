// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use crate::file_cache::resolve_sysfs_path;
use crate::hardware::is_big_core;
use crate::hexagon::inference::{
    parse_llm_args_from_cmdline, query_llama_slots, LlamaEngineMetrics,
};
use crate::sysinfo_ext::get_process_threads_placement_with_predicate;
use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use std::time::Instant;

/// Hexagon NPU / CDSP active process and model offload parameters
#[derive(Debug, Clone, serde::Serialize)]
pub struct FastRpcProcess {
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_size_mb: Option<u64>,
    pub target_device: String,     // e.g. "HTP0"
    pub n_threads: Option<u32>,    // -t 4
    pub n_gpu_layers: Option<u32>, // -ngl 99
    pub flash_attn: bool,          // -fa on
    pub context_size: Option<u32>, // -c 2048
    pub port: Option<u16>,         // --port 8000
    pub rss_mb: u64,
    pub device: String,
    pub devices: Vec<String>,
    pub polling_cpu_pct: f32,
    pub big_cores_cpu_pct: f32,
    pub little_cores_cpu_pct: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_metrics: Option<LlamaEngineMetrics>,
}

struct ProcessCpuTracker {
    last_ticks: HashMap<u32, (u64, Instant)>,
}

static CPU_TRACKER: Mutex<Option<ProcessCpuTracker>> = Mutex::new(None);

/// Compute CPU usage percentage of a process (e.g. llama-server polling thread)
fn compute_process_cpu_pct(pid: u32) -> f32 {
    let stat_path = resolve_sysfs_path(format!("/proc/{}/stat", pid));
    if let Ok(content) = fs::read_to_string(&stat_path) {
        if let Some(after_comm) = content.rfind(')') {
            let rest = &content[after_comm + 1..];
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if fields.len() > 12 {
                if let (Ok(utime), Ok(stime)) =
                    (fields[11].parse::<u64>(), fields[12].parse::<u64>())
                {
                    let total_ticks = utime + stime;
                    let now = Instant::now();

                    if let Ok(mut tracker_guard) = CPU_TRACKER.lock() {
                        if tracker_guard.is_none() {
                            *tracker_guard = Some(ProcessCpuTracker {
                                last_ticks: HashMap::new(),
                            });
                        }
                        let tracker = tracker_guard.as_mut().unwrap();

                        let prev = tracker.last_ticks.insert(pid, (total_ticks, now));
                        if let Some((prev_ticks, prev_time)) = prev {
                            let elapsed_sec = now.duration_since(prev_time).as_secs_f32();
                            if elapsed_sec > 0.05 && total_ticks >= prev_ticks {
                                let delta_ticks = total_ticks - prev_ticks;
                                let cpu_pct = delta_ticks as f32 / elapsed_sec;
                                return (cpu_pct * 10.0).round() / 10.0;
                            }
                        }
                    }
                }
            }
        }
    }
    0.0
}

/// Scan active AI workloads using a hybrid strategy:
/// 1. Command-line inspection (/proc/*/cmdline, world-readable) matching HTP / QNN / llama-server
/// 2. File descriptor inspection (/proc/*/fd, when elevated) matching /dev/fastrpc-cdsp*
pub fn scan_fastrpc_processes() -> Vec<FastRpcProcess> {
    let mut found_map: HashMap<u32, FastRpcProcess> = HashMap::new();
    let current_pid = std::process::id();

    let proc_dir = resolve_sysfs_path("/proc");
    let entries = match fs::read_dir(&proc_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();
        let pid: u32 = match name_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        if pid == current_pid {
            continue;
        }

        let cmdline_path = entry.path().join("cmdline");
        let cmdline = fs::read(&cmdline_path)
            .map(|bytes| {
                bytes
                    .split(|&b| b == 0)
                    .map(|s| String::from_utf8_lossy(s))
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        let comm_path = entry.path().join("comm");
        let name = fs::read_to_string(&comm_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        let name_lower = name.to_lowercase();

        // Skip shell scripts, supervisor wrappers, and standard utility interpreters
        if name_lower == "bash"
            || name_lower == "sh"
            || name_lower == "dash"
            || name_lower == "zsh"
            || name_lower.ends_with(".sh")
            || name_lower.ends_with(".s")
        {
            continue;
        }

        // Blacklist system daemons and non-compute DSP / audio / sensor routing services
        const DAEMON_BLACKLIST: &[&str] = &[
            "cdsprpcd",
            "adsprpcd",
            "fastrpc-daemon",
            "qrtr-lookup",
            "sensors-daemon",
            "pulseaudio",
            "pipewire",
            "audio-hal",
        ];
        let first_cmd_word = cmdline
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();
        let is_daemon = DAEMON_BLACKLIST.iter().any(|d| {
            name_lower.contains(d)
                || first_cmd_word.contains(d)
                || cmdline.to_lowercase().contains(&format!("/{}", d))
        });
        if is_daemon {
            continue;
        }

        // Check if executable is an AI / NPU binary candidate
        let is_npu_binary = name_lower.contains("llama")
            || name_lower.contains("qnn")
            || name_lower.contains("snpe");

        let is_npu_cmdline = is_npu_binary
            && (cmdline.contains("-dev HTP")
                || cmdline.contains("HTP0")
                || cmdline.contains("libggml-hexagon")
                || cmdline.contains("-m")
                || cmdline.contains("--model"));

        // Inspect open file descriptors in /proc/[pid]/fd
        let mut open_fastrpc_nodes = Vec::new();
        let fd_dir = entry.path().join("fd");
        let mut fd_read_success = false;
        if let Ok(fd_entries) = fs::read_dir(&fd_dir) {
            fd_read_success = true;
            for fd_entry in fd_entries.flatten() {
                if let Ok(target) = fs::read_link(fd_entry.path()) {
                    let target_str = target.to_string_lossy();
                    // Must specifically hold open CDSP FastRPC device (not ADSP, SMD, or other DSPs)
                    let is_cdsp_dev = target_str == "/dev/fastrpc-cdsp"
                        || target_str == "/dev/fastrpc-cdsp-secure"
                        || target_str.ends_with("/fastrpc-cdsp")
                        || target_str.ends_with("/fastrpc-cdsp-secure");
                    if is_cdsp_dev {
                        let node = target_str.to_string();
                        if !open_fastrpc_nodes.contains(&node) {
                            open_fastrpc_nodes.push(node);
                        }
                    }
                }
            }
        }

        // Qualification rule:
        // When elevated (/proc/[pid]/fd is readable), MUST hold open /dev/fastrpc-cdsp(-secure).
        // When unprivileged (fd returns EACCES), fallback to confirmed AI binary cmdline with model or device flags.
        let is_qualified = if fd_read_success {
            !open_fastrpc_nodes.is_empty()
        } else {
            is_npu_cmdline
        };

        if !is_qualified {
            continue;
        }

        // Read RSS memory from /proc/[pid]/statm (world-readable)
        let mut rss_mb = 0;
        let statm_path = entry.path().join("statm");
        if let Ok(statm) = fs::read_to_string(&statm_path) {
            let parts: Vec<&str> = statm.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(pages) = parts[1].parse::<u64>() {
                    rss_mb = (pages * 4096) / (1024 * 1024);
                }
            }
        }

        let (
            model_name,
            model_path,
            model_size_mb,
            target_device,
            n_threads,
            n_gpu_layers,
            flash_attn,
            context_size,
            port,
        ) = parse_llm_args_from_cmdline(&cmdline);

        let devices = if !open_fastrpc_nodes.is_empty() {
            open_fastrpc_nodes.clone()
        } else {
            vec!["/dev/fastrpc-cdsp".to_string()]
        };

        let device = if open_fastrpc_nodes
            .iter()
            .any(|d| d.ends_with("fastrpc-cdsp"))
        {
            "/dev/fastrpc-cdsp".to_string()
        } else if let Some(first) = open_fastrpc_nodes.first() {
            first.clone()
        } else {
            "/dev/fastrpc-cdsp".to_string()
        };

        let polling_cpu_pct = compute_process_cpu_pct(pid);

        // Compute Big vs Little core split for this FastRPC process
        let (_, _, _, _, little_ticks, big_ticks) =
            get_process_threads_placement_with_predicate(pid, is_big_core);
        let total_ticks = little_ticks + big_ticks;
        let (big_ratio, little_ratio) = if total_ticks > 0 {
            (
                big_ticks as f32 / total_ticks as f32,
                little_ticks as f32 / total_ticks as f32,
            )
        } else {
            (1.0, 0.0) // default to big cores for NPU driver thread
        };
        let big_cores_cpu_pct = ((polling_cpu_pct * big_ratio) * 10.0).round() / 10.0;
        let little_cores_cpu_pct = ((polling_cpu_pct * little_ratio) * 10.0).round() / 10.0;

        // Query /slots if HTTP port is present (e.g. llama-server)
        let engine_metrics = port.and_then(query_llama_slots);

        found_map.insert(
            pid,
            FastRpcProcess {
                pid,
                name,
                cmdline,
                model_name,
                model_path,
                model_size_mb,
                target_device,
                n_threads,
                n_gpu_layers,
                flash_attn,
                context_size,
                port,
                rss_mb,
                device,
                devices,
                polling_cpu_pct,
                big_cores_cpu_pct,
                little_cores_cpu_pct,
                engine_metrics,
            },
        );
    }

    if let Ok(mut tracker_guard) = CPU_TRACKER.lock() {
        if let Some(tracker) = tracker_guard.as_mut() {
            tracker
                .last_ticks
                .retain(|pid, _| found_map.contains_key(pid));
        }
    }

    let mut processes: Vec<FastRpcProcess> = found_map.into_values().collect();
    processes.sort_by_key(|p| std::cmp::Reverse(p.rss_mb));
    processes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_cache::EnvGuard;

    #[test]
    fn test_scan_fastrpc_processes_filters_daemons_and_adsp() {
        let temp_dir =
            std::env::temp_dir().join(format!("ventop_proc_scan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);

        // 1. PID 101: cdsprpcd (Qualcomm Compute DSP daemon) opening /dev/fastrpc-cdsp
        let p101 = temp_dir.join("proc/101");
        std::fs::create_dir_all(p101.join("fd")).unwrap();
        std::fs::write(p101.join("comm"), "cdsprpcd\n").unwrap();
        std::fs::write(p101.join("cmdline"), "/usr/sbin/cdsprpcd\0").unwrap();
        std::fs::write(p101.join("statm"), "0 3500 0\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/dev/fastrpc-cdsp", p101.join("fd/0")).unwrap();
        #[cfg(not(unix))]
        std::fs::write(p101.join("fd/0"), "/dev/fastrpc-cdsp").unwrap();

        // 2. PID 102: adsprpcd (Audio DSP daemon) opening /dev/fastrpc-adsp
        let p102 = temp_dir.join("proc/102");
        std::fs::create_dir_all(p102.join("fd")).unwrap();
        std::fs::write(p102.join("comm"), "adsprpcd\n").unwrap();
        std::fs::write(p102.join("cmdline"), "/usr/sbin/adsprpcd\0").unwrap();
        std::fs::write(p102.join("statm"), "0 2500 0\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/dev/fastrpc-adsp", p102.join("fd/0")).unwrap();
        #[cfg(not(unix))]
        std::fs::write(p102.join("fd/0"), "/dev/fastrpc-adsp").unwrap();

        // 3. PID 103: fastrpc-daemon opening /dev/fastrpc-cdsp
        let p103 = temp_dir.join("proc/103");
        std::fs::create_dir_all(p103.join("fd")).unwrap();
        std::fs::write(p103.join("comm"), "fastrpc-daemon\n").unwrap();
        std::fs::write(p103.join("cmdline"), "/usr/bin/fastrpc-daemon\0").unwrap();
        std::fs::write(p103.join("statm"), "0 1000 0\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/dev/fastrpc-cdsp", p103.join("fd/0")).unwrap();
        #[cfg(not(unix))]
        std::fs::write(p103.join("fd/0"), "/dev/fastrpc-cdsp").unwrap();

        // 4. PID 104: pulseaudio audio client opening /dev/fastrpc-adsp
        let p104 = temp_dir.join("proc/104");
        std::fs::create_dir_all(p104.join("fd")).unwrap();
        std::fs::write(p104.join("comm"), "pulseaudio\n").unwrap();
        std::fs::write(p104.join("cmdline"), "/usr/bin/pulseaudio\0").unwrap();
        std::fs::write(p104.join("statm"), "0 8000 0\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/dev/fastrpc-adsp", p104.join("fd/0")).unwrap();
        #[cfg(not(unix))]
        std::fs::write(p104.join("fd/0"), "/dev/fastrpc-adsp").unwrap();

        // 5. PID 105: genuine llama-server process opening /dev/fastrpc-cdsp
        let p105 = temp_dir.join("proc/105");
        std::fs::create_dir_all(p105.join("fd")).unwrap();
        std::fs::write(p105.join("comm"), "llama-server\n").unwrap();
        let cmd = [
            "/opt/bin/llama-server",
            "-m",
            "/models/qwen.gguf",
            "-dev",
            "HTP0",
            "-t",
            "4",
        ]
        .join("\0");
        std::fs::write(p105.join("cmdline"), cmd).unwrap();
        std::fs::write(p105.join("statm"), "0 716800 0\n").unwrap(); // 716800 pages * 4KB = 2800 MB
        #[cfg(unix)]
        std::os::unix::fs::symlink("/dev/fastrpc-cdsp", p105.join("fd/0")).unwrap();
        #[cfg(not(unix))]
        std::fs::write(p105.join("fd/0"), "/dev/fastrpc-cdsp").unwrap();

        let _guard = EnvGuard::set_root(&temp_dir);
        let procs = scan_fastrpc_processes();

        // ONLY PID 105 must be admitted; daemons and ADSP clients must be filtered out!
        assert_eq!(
            procs.len(),
            1,
            "Only genuine llama-server process should be admitted"
        );
        assert_eq!(procs[0].pid, 105);
        assert_eq!(procs[0].name, "llama-server");
        assert_eq!(procs[0].model_name, Some("qwen.gguf".to_string()));
        assert_eq!(procs[0].rss_mb, 2800);

        drop(_guard);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
