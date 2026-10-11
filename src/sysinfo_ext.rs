// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use sysinfo::{Process, System};

// Global cache for UID to username mappings
static USER_CACHE: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSortMode {
    CpuDesc,
    MemoryDesc,
    PidAsc,
    NameAsc,
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub user: String,
    pub cpu: f32,
    pub mem: f32,
    pub nice: i32,
    pub cpu_core: u32,
    pub state: char,
    pub num_threads: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ZramInfo {
    pub orig_data_size: u64,
    pub compr_data_size: u64,
    pub used: u64,
    pub limit: u64,
}

struct ProcStatInfo {
    state: char,
    nice: i32,
    cpu_core: u32,
}

fn parse_proc_stat(pid: u32) -> ProcStatInfo {
    let stat_path = format!("/proc/{}/stat", pid);
    if let Ok(content) = fs::read_to_string(&stat_path) {
        if let Some(fields) = parse_stat_after_comm(&content) {
            let state = fields.first().and_then(|f| f.chars().next()).unwrap_or('U');
            let nice = if fields.len() > 16 {
                fields[16].parse::<i32>().unwrap_or(0)
            } else {
                0
            };
            let cpu_core = if fields.len() > 36 {
                fields[36].parse::<u32>().unwrap_or(0)
            } else {
                0
            };
            return ProcStatInfo {
                state,
                nice,
                cpu_core,
            };
        }
    }
    ProcStatInfo {
        state: 'U',
        nice: 0,
        cpu_core: 0,
    }
}

struct ProcStatusInfo {
    tgid: u32,
    num_threads: u32,
}

fn parse_proc_status(pid: u32) -> ProcStatusInfo {
    let mut tgid = pid;
    let mut num_threads = 1;

    let status_path = format!("/proc/{}/status", pid);
    if let Ok(content) = fs::read_to_string(&status_path) {
        for line in content.lines() {
            if line.starts_with("Tgid:") {
                if let Some(tgid_str) = line.split_whitespace().nth(1) {
                    if let Ok(parsed) = tgid_str.parse::<u32>() {
                        tgid = parsed;
                    }
                }
            } else if line.starts_with("Threads:") {
                if let Some(threads_str) = line.split_whitespace().nth(1) {
                    if let Ok(threads) = threads_str.parse::<u32>() {
                        num_threads = threads;
                    }
                }
            }
        }
    }

    ProcStatusInfo { tgid, num_threads }
}

/// Get top processes with configurable sorting
pub fn get_top_processes(
    sys: &System,
    count: usize,
    sort_mode: ProcessSortMode,
) -> Vec<ProcessInfo> {
    // First pass: collect minimal info and sort
    let mut minimal_processes: Vec<_> = sys
        .processes()
        .iter()
        .map(|(pid, process)| {
            (
                pid.as_u32(),
                process,
                process.cpu_usage(),
                process.memory() as f32 / sys.total_memory() as f32 * 100.0,
            )
        })
        .collect();

    // Sort minimal list based on selected mode
    use std::cmp::Ordering;
    match sort_mode {
        ProcessSortMode::CpuDesc => {
            minimal_processes.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal));
        }
        ProcessSortMode::MemoryDesc => {
            minimal_processes.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(Ordering::Equal));
        }
        ProcessSortMode::PidAsc => {
            minimal_processes.sort_by_key(|p| p.0);
        }
        ProcessSortMode::NameAsc => {
            minimal_processes.sort_by(|a, b| {
                a.1.name()
                    .to_string_lossy()
                    .to_lowercase()
                    .cmp(&b.1.name().to_string_lossy().to_lowercase())
            });
        }
    }

    // Second pass: filter secondary threads (tgid != pid) and parse detailed procfs stats for top N
    let mut results = Vec::with_capacity(count);
    for (pid_u32, process, cpu, mem) in minimal_processes {
        let status = parse_proc_status(pid_u32);
        if status.tgid != pid_u32 {
            continue; // Skip secondary threads to avoid thread spam
        }

        let name = process.name().to_string_lossy().to_string();
        let user = get_process_user(process);
        let stat = parse_proc_stat(pid_u32);

        results.push(ProcessInfo {
            pid: pid_u32,
            name,
            user,
            cpu,
            mem,
            nice: stat.nice,
            cpu_core: stat.cpu_core,
            state: stat.state,
            num_threads: status.num_threads,
        });

        if results.len() >= count {
            break;
        }
    }
    results
}

/// Helper to extract fields after the command name from /proc/[pid]/stat content.
/// The command name in field 2 is enclosed in parentheses and may contain spaces or parentheses.
/// Returns the remaining whitespace-separated fields starting at field 3 (state).
pub fn parse_stat_after_comm(stat_content: &str) -> Option<Vec<&str>> {
    let rparen = stat_content.rfind(')')?;
    let rest = &stat_content[rparen + 1..];
    Some(rest.split_whitespace().collect())
}

fn get_process_user(process: &Process) -> String {
    if let Some(uid) = process.user_id() {
        let uid_num = **uid;

        let mut cache = USER_CACHE.lock().unwrap();
        if cache.is_none() {
            *cache = Some(HashMap::new());
        }

        let cache_map = cache.as_mut().unwrap();

        if let Some(username) = cache_map.get(&uid_num) {
            return username.clone();
        }

        let username = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid_num))
            .ok()
            .flatten()
            .map(|u| u.name)
            .unwrap_or_else(|| uid_num.to_string());

        cache_map.insert(uid_num, username.clone());
        username
    } else {
        "-".to_string()
    }
}

pub fn get_zram_info() -> Option<ZramInfo> {
    let zram_dirs = ["/sys/block/zram0", "/sys/block/zram1"];

    for dir in &zram_dirs {
        let orig_path = format!("{}/orig_data_size", dir);
        let compr_path = format!("{}/compr_data_size", dir);
        let mem_used_path = format!("{}/mem_used_total", dir);
        let disksize_path = format!("{}/disksize", dir);

        if let (Ok(orig), Ok(compr), Ok(used), Ok(limit)) = (
            fs::read_to_string(&orig_path),
            fs::read_to_string(&compr_path),
            fs::read_to_string(&mem_used_path),
            fs::read_to_string(&disksize_path),
        ) {
            if let (Ok(orig_val), Ok(compr_val), Ok(used_val), Ok(limit_val)) = (
                orig.trim().parse::<u64>(),
                compr.trim().parse::<u64>(),
                used.trim().parse::<u64>(),
                limit.trim().parse::<u64>(),
            ) {
                return Some(ZramInfo {
                    orig_data_size: orig_val,
                    compr_data_size: compr_val,
                    used: used_val,
                    limit: limit_val,
                });
            }
        }
    }

    None
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuStats {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
}

pub fn get_cpu_stats() -> CpuStats {
    let mut stats = CpuStats::default();
    let stat_path = crate::file_cache::resolve_sysfs_path("/proc/stat");

    if let Ok(content) = fs::read_to_string(&stat_path) {
        for line in content.lines() {
            if line.starts_with("cpu ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 8 {
                    stats.user = parts[1].parse().unwrap_or(0);
                    stats.nice = parts[2].parse().unwrap_or(0);
                    stats.system = parts[3].parse().unwrap_or(0);
                    stats.idle = parts[4].parse().unwrap_or(0);
                    stats.iowait = parts[5].parse().unwrap_or(0);
                    stats.irq = parts[6].parse().unwrap_or(0);
                    stats.softirq = parts[7].parse().unwrap_or(0);
                }
                break;
            }
        }
    }

    stats
}

#[derive(Debug, Clone, Serialize)]
pub struct ThreadPlacement {
    pub tid: u32,
    pub name: String,
    pub cpu_core: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessAffinityProfile {
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
    pub cpu_pct: f32,
    pub big_cores_cpu_pct: f32,    // CPU % attributed to Big/Prime cores
    pub little_cores_cpu_pct: f32, // CPU % attributed to Little cores
    pub rss_bytes: u64,
    pub rss_mb: u64,
    pub thread_count: usize,
    pub little_cores_count: usize, // Little cores
    pub big_cores_count: usize,    // Big / Prime cores
    pub core_distribution: HashMap<String, usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<ThreadPlacement>,
}

/// Identifies Big/Prime cores. Queries dynamic CPU topology from hardware discovery,
/// falling back to the standard Arduino Ventuno Q layout (Cores 0-3 Big, Cores 4-7 Little).
pub fn is_ventuno_big_core(core: u32) -> bool {
    crate::hardware::is_big_core(core)
}

/// Read thread-level placement and affinity for a specific PID.
/// `is_big_core` is a predicate determining if core index belongs to the big cluster.
/// Returns (threads, distribution, little_count, big_count, little_ticks, big_ticks)
pub fn get_process_threads_placement_with_predicate<F>(
    pid: u32,
    is_big_core: F,
) -> (
    Vec<ThreadPlacement>,
    HashMap<String, usize>,
    usize,
    usize,
    u64,
    u64,
)
where
    F: Fn(u32) -> bool,
{
    let mut threads = Vec::new();
    let mut distribution = HashMap::new();
    let mut little_count = 0;
    let mut big_count = 0;
    let mut little_ticks = 0u64;
    let mut big_ticks = 0u64;

    let task_dir = format!("/proc/{}/task", pid);
    if let Ok(entries) = fs::read_dir(task_dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let tid_str = file_name.to_string_lossy();
            if let Ok(tid) = tid_str.parse::<u32>() {
                let comm_path = entry.path().join("comm");
                let name = fs::read_to_string(&comm_path)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| "unknown".to_string());

                let stat_path = entry.path().join("stat");
                let mut cpu_core = 0;
                let mut thread_ticks = 0u64;
                if let Ok(stat_content) = fs::read_to_string(&stat_path) {
                    if let Some(fields) = parse_stat_after_comm(&stat_content) {
                        if fields.len() > 12 {
                            let utime: u64 = fields[11].parse().unwrap_or(0);
                            let stime: u64 = fields[12].parse().unwrap_or(0);
                            thread_ticks = utime + stime;
                        }
                        if fields.len() > 36 {
                            if let Ok(core) = fields[36].parse::<u32>() {
                                cpu_core = core;
                            }
                        }
                    }
                }

                if is_big_core(cpu_core) {
                    big_count += 1;
                    big_ticks += thread_ticks;
                } else {
                    little_count += 1;
                    little_ticks += thread_ticks;
                }

                let core_key = format!("core{}", cpu_core);
                *distribution.entry(core_key).or_insert(0) += 1;

                threads.push(ThreadPlacement {
                    tid,
                    name,
                    cpu_core,
                });
            }
        }
    }

    threads.sort_by_key(|t| t.tid);
    (
        threads,
        distribution,
        little_count,
        big_count,
        little_ticks,
        big_ticks,
    )
}

/// Parse /proc/[pid]/status content to determine if this PID is a secondary thread (i.e. Tgid != pid)
pub fn parse_tgid_is_thread(status_content: &str, pid: u32) -> bool {
    for line in status_content.lines() {
        if line.starts_with("Tgid:") {
            if let Some(tgid_str) = line.split_whitespace().nth(1) {
                if let Ok(tgid) = tgid_str.parse::<u32>() {
                    return pid != tgid;
                }
            }
            break;
        }
    }
    false
}

pub fn is_secondary_thread(pid: u32) -> bool {
    let status_path = format!("/proc/{}/status", pid);
    if let Ok(content) = fs::read_to_string(&status_path) {
        return parse_tgid_is_thread(&content, pid);
    }
    false
}

pub fn process_matches_patterns(process: &Process, patterns: &[String], current_pid: u32) -> bool {
    let pid_u32 = process.pid().as_u32();

    if pid_u32 == current_pid {
        return false;
    }

    if is_secondary_thread(pid_u32) {
        return false;
    }

    let name = process.name().to_string_lossy();
    let name_lower = name.to_lowercase();

    if name_lower == "sudo"
        || name_lower == "timeout"
        || name_lower == "ventop-ai"
        || name_lower == "grep"
    {
        return false;
    }

    let exe_name = process
        .exe()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let exe_lower = exe_name.to_lowercase();

    // Extract basename of first cmdline token (argv[0])
    let first_cmd = process
        .cmd()
        .first()
        .map(|arg| {
            let s = arg.to_string_lossy();
            std::path::Path::new(s.as_ref())
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| s.to_string())
        })
        .unwrap_or_default()
        .to_lowercase();

    // Check if this process is an interactive or script shell (bash, sh, zsh, etc.)
    let is_shell = name_lower == "bash"
        || name_lower == "sh"
        || name_lower == "dash"
        || name_lower == "zsh"
        || exe_lower == "bash";

    patterns.iter().any(|pat| {
        let pat_lower = pat.to_lowercase();

        // Unless the user explicitly asked to track "bash" or "sh", do not match shells
        if is_shell && pat_lower != "bash" && pat_lower != "sh" && pat_lower != "zsh" {
            return false;
        }

        name_lower.contains(&pat_lower)
            || exe_lower.contains(&pat_lower)
            || first_cmd.contains(&pat_lower)
    })
}

pub fn get_tracked_processes_with_predicate_opts<F>(
    sys: &System,
    patterns: &[String],
    is_big_core: F,
    include_threads: bool,
) -> Vec<ProcessAffinityProfile>
where
    F: Fn(u32) -> bool + Copy,
{
    if patterns.is_empty() {
        return Vec::new();
    }

    let mut profiles = Vec::new();
    let current_pid = std::process::id();

    for (pid, process) in sys.processes() {
        if process_matches_patterns(process, patterns, current_pid) {
            let pid_u32 = pid.as_u32();
            let name = process.name().to_string_lossy().to_string();
            let cmdline = process
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            let (
                threads,
                core_distribution,
                little_cores_count,
                big_cores_count,
                little_ticks,
                big_ticks,
            ) = get_process_threads_placement_with_predicate(pid_u32, is_big_core);
            let thread_count = if !threads.is_empty() {
                threads.len()
            } else {
                1
            };
            let rss_bytes = process.memory();
            let rss_mb = rss_bytes / (1024 * 1024);

            let total_cpu = process.cpu_usage();
            let total_ticks = little_ticks + big_ticks;
            let (big_ratio, little_ratio) = if total_ticks > 0 {
                (
                    big_ticks as f32 / total_ticks as f32,
                    little_ticks as f32 / total_ticks as f32,
                )
            } else if big_cores_count + little_cores_count > 0 {
                let total = (big_cores_count + little_cores_count) as f32;
                (
                    big_cores_count as f32 / total,
                    little_cores_count as f32 / total,
                )
            } else {
                (0.0, 0.0)
            };
            let big_cores_cpu_pct = ((total_cpu * big_ratio) * 10.0).round() / 10.0;
            let little_cores_cpu_pct = ((total_cpu * little_ratio) * 10.0).round() / 10.0;

            let stored_threads = if include_threads { threads } else { Vec::new() };

            profiles.push(ProcessAffinityProfile {
                pid: pid_u32,
                name,
                cmdline,
                cpu_pct: total_cpu,
                big_cores_cpu_pct,
                little_cores_cpu_pct,
                rss_bytes,
                rss_mb,
                thread_count,
                little_cores_count,
                big_cores_count,
                core_distribution,
                threads: stored_threads,
            });
        }
    }

    profiles.sort_by(|a, b| {
        b.cpu_pct
            .partial_cmp(&a.cpu_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    profiles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stat_after_comm_standard() {
        let sample = "12345 (bash) S 1234 12345 12345 34816 12345 4194304 382 0 0 0 2 1 0 0 20 0 1 0 123456 1234567 123 18446744073709551615 0 0 0 0 0 0 0 2147483647 0 0 0 0 17 6 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let fields = parse_stat_after_comm(sample).expect("should parse");
        assert_eq!(fields[0], "S");
        assert_eq!(fields[16], "0");
        assert_eq!(fields[36], "6");
    }

    #[test]
    fn test_parse_stat_after_comm_with_spaces_and_nested_parens() {
        let sample = "8812 (llama (worker) 0) R 8800 8812 8812 0 -1 4194304 50 0 0 0 150 20 0 0 10 -5 4 0 50000 90000 500 18446744073709551615 0 0 0 0 0 0 0 2147483647 0 0 0 0 17 5 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let fields = parse_stat_after_comm(sample).expect("should parse");
        assert_eq!(fields[0], "R");
        assert_eq!(fields[16], "-5");
        assert_eq!(fields[36], "5");
    }

    #[test]
    fn test_parse_stat_after_comm_malformed() {
        assert!(parse_stat_after_comm("invalid string without parens").is_none());
        assert!(parse_stat_after_comm("").is_none());
    }

    #[test]
    fn test_parse_tgid_is_thread() {
        let status_main = "Name:\tllama-server\nUmask:\t0022\nState:\tS (sleeping)\nTgid:\t27719\nNgid:\t0\nPid:\t27719\n";
        assert!(!parse_tgid_is_thread(status_main, 27719));

        let status_thread = "Name:\tllama-server\nUmask:\t0022\nState:\tR (running)\nTgid:\t27719\nNgid:\t0\nPid:\t27899\n";
        assert!(parse_tgid_is_thread(status_thread, 27899));
    }

    #[test]
    fn test_is_ventuno_big_core() {
        assert!(is_ventuno_big_core(0));
        assert!(is_ventuno_big_core(1));
        assert!(is_ventuno_big_core(2));
        assert!(is_ventuno_big_core(3));
        assert!(!is_ventuno_big_core(4));
        assert!(!is_ventuno_big_core(5));
        assert!(!is_ventuno_big_core(6));
        assert!(!is_ventuno_big_core(7));
    }

    #[test]
    fn test_process_affinity_profile_skips_empty_threads() {
        let profile_no_threads = ProcessAffinityProfile {
            pid: 1234,
            name: "GemmaTeacher".to_string(),
            cmdline: "/opt/GemmaTeacher".to_string(),
            cpu_pct: 66.5,
            big_cores_cpu_pct: 35.0,
            little_cores_cpu_pct: 31.5,
            rss_bytes: 1024 * 1024 * 50,
            rss_mb: 50,
            thread_count: 89,
            little_cores_count: 42,
            big_cores_count: 47,
            core_distribution: HashMap::new(),
            threads: Vec::new(),
        };

        let json = serde_json::to_string(&profile_no_threads).unwrap();
        assert!(!json.contains("\"threads\":"));
        assert!(json.contains("\"thread_count\":89"));
        assert!(json.contains("\"big_cores_count\":47"));
    }
}
