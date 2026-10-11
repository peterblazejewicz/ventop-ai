// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use tempfile::TempDir;

use ventop_ai::file_cache::reset_file_cache;
use ventop_ai::hardware::{
    get_adreno_gpu_metrics, get_consolidated_thermals, get_cpu_clusters_with_load,
    get_hardware_power_fan,
};
use ventop_ai::hexagon::{
    collect_hexagon_snapshot, evaluate_inference_activity, get_cdsp_throttle_state,
    get_nsp_silicon_thermals, parse_cdsp_dma_buf_mb, parse_glink_interrupt_count_from_str,
    read_glink_interrupt_count, scan_fastrpc_processes,
};
use ventop_ai::snapshot::{
    collect_snapshot_continuous, compute_cpu_pct_from_stats, reset_prev_cpu_stats, SummaryTracker,
};
use ventop_ai::sysinfo_ext::CpuStats;

static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

/// RAII environment guard ensuring single-threaded fixture activation,
/// environment variable registration, and clean cache teardown upon drop.
pub struct FixtureEnvGuard<'a> {
    _lock: MutexGuard<'a, ()>,
}

impl<'a> Drop for FixtureEnvGuard<'a> {
    fn drop(&mut self) {
        std::env::remove_var("VENTOP_SYSFS_ROOT");
        reset_file_cache();
        reset_prev_cpu_stats();
    }
}

/// Synthetic sysfs, procfs, devfs, and run-directory platform fixture.
pub struct MockPlatformFixture {
    pub dir: TempDir,
}

impl MockPlatformFixture {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("failed to create temporary fixture directory");
        let fixture = Self { dir };
        fs::create_dir_all(fixture.sysfs()).unwrap();
        fs::create_dir_all(fixture.procfs()).unwrap();
        fs::create_dir_all(fixture.devfs()).unwrap();
        fs::create_dir_all(fixture.run_dir()).unwrap();
        fixture
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn sysfs(&self) -> PathBuf {
        self.dir.path().join("sys")
    }

    pub fn procfs(&self) -> PathBuf {
        self.dir.path().join("proc")
    }

    pub fn devfs(&self) -> PathBuf {
        self.dir.path().join("dev")
    }

    pub fn run_dir(&self) -> PathBuf {
        self.dir.path().join("var/run")
    }

    /// Activate this fixture by setting VENTOP_SYSFS_ROOT and resetting all caches.
    pub fn activate(&self) -> FixtureEnvGuard<'static> {
        let lock = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VENTOP_SYSFS_ROOT", self.path());
        reset_file_cache();
        reset_prev_cpu_stats();
        FixtureEnvGuard { _lock: lock }
    }

    pub fn add_hwmon_power(
        &self,
        dir_name: &str,
        chip_name: &str,
        micro_watts: u64,
        milli_volts: u32,
        milli_amps: u32,
    ) {
        let dir = self.sysfs().join("class/hwmon").join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("name"), format!("{}\n", chip_name)).unwrap();
        fs::write(dir.join("power1_input"), format!("{}\n", micro_watts)).unwrap();
        fs::write(dir.join("in1_input"), format!("{}\n", milli_volts)).unwrap();
        fs::write(dir.join("curr1_input"), format!("{}\n", milli_amps)).unwrap();
    }

    pub fn add_hwmon_fan(&self, dir_name: &str, chip_name: &str, channel: &str, rpm: u32) {
        let dir = self.sysfs().join("class/hwmon").join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("name"), format!("{}\n", chip_name)).unwrap();
        fs::write(dir.join(format!("{}_input", channel)), format!("{}\n", rpm)).unwrap();
    }

    pub fn add_thermal_zone(&self, dir_name: &str, zone_type: &str, milli_celsius: i32) {
        let dir = self.sysfs().join("class/thermal").join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("type"), format!("{}\n", zone_type)).unwrap();
        fs::write(dir.join("temp"), format!("{}\n", milli_celsius)).unwrap();
    }

    pub fn add_cooling_device(
        &self,
        dir_name: &str,
        device_type: &str,
        cur_state: u32,
        max_state: u32,
    ) {
        let dir = self.sysfs().join("class/thermal").join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("type"), format!("{}\n", device_type)).unwrap();
        fs::write(dir.join("cur_state"), format!("{}\n", cur_state)).unwrap();
        fs::write(dir.join("max_state"), format!("{}\n", max_state)).unwrap();
    }

    pub fn add_remoteproc(&self, dir_name: &str, name_or_firmware: &str, state: &str) {
        let dir = self.sysfs().join("class/remoteproc").join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("name"), format!("{}\n", name_or_firmware)).unwrap();
        fs::write(dir.join("firmware"), format!("{}.mdt\n", name_or_firmware)).unwrap();
        fs::write(dir.join("state"), format!("{}\n", state)).unwrap();
    }

    pub fn add_cpufreq_policy(
        &self,
        policy_name: &str,
        affected_cpus: &str,
        min_khz: u32,
        max_khz: u32,
        cur_khz: u32,
        governor: &str,
    ) {
        let dir = self
            .sysfs()
            .join("devices/system/cpu/cpufreq")
            .join(policy_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("affected_cpus"), format!("{}\n", affected_cpus)).unwrap();
        fs::write(dir.join("scaling_min_freq"), format!("{}\n", min_khz)).unwrap();
        fs::write(dir.join("scaling_max_freq"), format!("{}\n", max_khz)).unwrap();
        fs::write(dir.join("scaling_cur_freq"), format!("{}\n", cur_khz)).unwrap();
        fs::write(dir.join("scaling_governor"), format!("{}\n", governor)).unwrap();
    }

    pub fn add_devfreq_gpu(
        &self,
        dir_name: &str,
        cur_freq_hz: u64,
        governor: &str,
        available_freqs_hz: &[u64],
    ) {
        let dir = self.sysfs().join("class/devfreq").join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cur_freq"), format!("{}\n", cur_freq_hz)).unwrap();
        fs::write(dir.join("governor"), format!("{}\n", governor)).unwrap();
        let freqs_str = available_freqs_hz
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        fs::write(
            dir.join("available_frequencies"),
            format!("{}\n", freqs_str),
        )
        .unwrap();
    }

    pub fn add_process_with_fds(
        &self,
        pid: u32,
        comm: &str,
        cmdline: &str,
        rss_pages: u64,
        open_devices: &[&str],
    ) {
        let pdir = self.procfs().join(pid.to_string());
        fs::create_dir_all(pdir.join("fd")).unwrap();
        fs::write(pdir.join("comm"), format!("{}\n", comm)).unwrap();
        let null_cmdline = cmdline.replace(' ', "\0");
        fs::write(pdir.join("cmdline"), format!("{}\0", null_cmdline)).unwrap();
        fs::write(pdir.join("statm"), format!("0 {} 0 0 0 0 0\n", rss_pages)).unwrap();

        for (i, dev) in open_devices.iter().enumerate() {
            let link = pdir.join("fd").join(i.to_string());
            #[cfg(unix)]
            {
                let _ = std::os::unix::fs::symlink(dev, &link);
            }
            #[cfg(not(unix))]
            {
                let _ = fs::write(&link, dev);
            }
        }
    }

    pub fn set_proc_stat(&self, content: &str) {
        fs::write(self.procfs().join("stat"), content).unwrap();
    }

    pub fn set_proc_interrupts(&self, content: &str) {
        fs::write(self.procfs().join("interrupts"), content).unwrap();
    }

    pub fn set_debugfs_dma_buf(&self, content: &str) {
        let dir = self.sysfs().join("kernel/debug/dma_buf");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bufinfo"), content).unwrap();
    }

    pub fn add_dev_node(&self, name: &str) {
        let path = self.devfs().join(name);
        File::create(path).unwrap();
    }
}

impl Default for MockPlatformFixture {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// SUITE 1: Renumbered Zones & Hwmon Indices
// ============================================================================
#[test]
fn test_suite1_renumbered_zones_and_hwmon() {
    let fixture = MockPlatformFixture::new();

    // Hwmon nodes at arbitrary non-standard indices: hwmon99 and hwmon13
    fixture.add_hwmon_power("hwmon99", "ina232", 15_400_000, 12_000, 1_280);
    fixture.add_hwmon_fan("hwmon13", "emc2305", "fan1", 3_250);

    // Thermal zones at arbitrary renumbered indices
    fixture.add_thermal_zone("thermal_zone42", "nsp-core", 46_000);
    fixture.add_thermal_zone("thermal_zone77", "nsp-hmx", 49_000);
    fixture.add_thermal_zone("thermal_zone88", "nsp-vtcm", 43_000);
    fixture.add_thermal_zone("thermal_zone101", "cpu-0-0", 52_000);
    fixture.add_thermal_zone("thermal_zone102", "cpu-1-0", 48_000);
    fixture.add_thermal_zone("thermal_zone103", "gpu-step", 50_000);
    fixture.add_thermal_zone("thermal_zone104", "ddrss-zone", 44_000);
    fixture.add_thermal_zone("thermal_zone105", "pmic-board", 41_000);

    // Cooling device at non-standard index: cooling_device55
    fixture.add_cooling_device("cooling_device55", "cdsp:cdsp_sw", 2, 4);

    let _guard = fixture.activate();

    // 1. Verify dynamic hwmon discovery finds ina232 at hwmon99 and emc2305 at hwmon13
    let pwr_fan = get_hardware_power_fan();
    assert_eq!(pwr_fan.board_power_watts, Some(15.4));
    assert_eq!(pwr_fan.board_voltage_v, Some(12.0));
    assert_eq!(pwr_fan.board_current_a, Some(1.28));
    assert_eq!(pwr_fan.fan_rpm, Some(3250));
    assert_eq!(pwr_fan.power_device.as_deref(), Some("ina232"));
    assert_eq!(pwr_fan.fan_device.as_deref(), Some("emc2305"));

    // 2. Verify dynamic thermals consolidation categorizes all 6 zones accurately
    let thermals = get_consolidated_thermals();
    assert_eq!(thermals.cpu_gold_celsius, Some(52));
    assert_eq!(thermals.cpu_silver_celsius, Some(48));
    assert_eq!(thermals.gpu_celsius, Some(50));
    assert_eq!(thermals.npu_nsp_celsius, Some(49));
    assert_eq!(thermals.ram_celsius, Some(44));
    assert_eq!(thermals.pmic_board_celsius, Some(41));

    // 3. Verify Hexagon NSP silicon thermals discovery finds zones 42, 77, 88
    let nsp = get_nsp_silicon_thermals();
    assert_eq!(nsp.core_vector_celsius, Some(46));
    assert_eq!(nsp.matrix_hmx_celsius, Some(49));
    assert_eq!(nsp.vtcm_sram_celsius, Some(43));
    assert_eq!(nsp.peak_celsius, Some(49));

    // 4. Verify dynamic CDSP cooling device discovery resolves cooling_device55
    let throttle = get_cdsp_throttle_state();
    assert_eq!(throttle, Some(2));

    // 5. Verify end-to-end binary JSON telemetry with renumbered nodes
    #[cfg(not(target_arch = "aarch64"))]
    {
        let bin = env!("CARGO_BIN_EXE_ventop-ai");
        let output = Command::new(bin)
            .env("VENTOP_SYSFS_ROOT", fixture.path())
            .arg("--json")
            .output()
            .expect("Failed to execute ventop-ai binary");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: serde_json::Value =
            serde_json::from_str(&stdout).expect("Failed to parse JSON output");

        assert_eq!(v["power_fan"]["board_power_watts"], 15.4);
        assert_eq!(v["power_fan"]["fan_rpm"], 3250);
        assert_eq!(v["thermals"]["cpu_gold_celsius"], 52);
        assert_eq!(v["hexagon_npu"]["throttle_level"], 2);
        assert_eq!(v["hexagon_npu"]["silicon_thermals"]["peak_celsius"], 49);
    }
}

// ============================================================================
// SUITE 2: Missing Nodes & Absent Debugfs
// ============================================================================
#[test]
fn test_suite2_missing_nodes_and_absent_debugfs() {
    let fixture = MockPlatformFixture::new();

    // Create empty parent directories without any child sensors or policy nodes
    fs::create_dir_all(fixture.sysfs().join("class/thermal")).unwrap();
    fs::create_dir_all(fixture.sysfs().join("class/devfreq")).unwrap();
    fs::create_dir_all(fixture.sysfs().join("devices/system/cpu/cpufreq")).unwrap();
    fs::create_dir_all(fixture.sysfs().join("class/hwmon")).unwrap();
    fs::create_dir_all(fixture.sysfs().join("class/remoteproc")).unwrap();
    // Intentionally do NOT create /sys/kernel/debug/dma_buf/bufinfo

    let _guard = fixture.activate();

    // 1. Missing GPU devfreq node must report None, never fabricate "simple_ondemand"
    let gpu = get_adreno_gpu_metrics();
    assert_eq!(gpu.cur_freq_mhz, None);
    assert_eq!(gpu.governor, None);
    assert!(gpu.available_frequencies_mhz.is_empty());

    // 2. Missing cooling device node must report None, never fabricate 0/4
    let throttle = get_cdsp_throttle_state();
    assert_eq!(throttle, None);

    // 3. Missing cpufreq policies must report 0 MHz and "unavailable", never fabricate 800/2000 or "schedutil"
    let clusters = get_cpu_clusters_with_load(&[]);
    assert_eq!(clusters.len(), 3);
    for cluster in &clusters {
        assert_eq!(cluster.cur_freq_mhz, 0);
        assert_eq!(cluster.min_freq_mhz, 0);
        assert_eq!(cluster.max_freq_mhz, 0);
        assert_eq!(cluster.governor, "unavailable");
    }

    // 4. Missing hwmon nodes must return None for power and fan
    let pwr_fan = get_hardware_power_fan();
    assert_eq!(pwr_fan.board_power_watts, None);
    assert_eq!(pwr_fan.fan_rpm, None);

    // 5. Absent debugfs must not panic and must return None without fabricating DMA-BUF allocation
    let dma_buf = parse_cdsp_dma_buf_mb();
    assert_eq!(dma_buf, None);

    // 6. Hexagon snapshot must report honest None/null across missing subsystems
    let hex = collect_hexagon_snapshot();
    assert_eq!(hex.dma_buf_allocated_mb, None);
    assert_eq!(hex.vram_allocated_mb, None);
    assert_eq!(hex.throttle_level, None);
    assert_eq!(hex.silicon_thermals.peak_celsius, None);

    // 7. Binary CLI serialization produces valid JSON with null fields
    #[cfg(not(target_arch = "aarch64"))]
    {
        let bin = env!("CARGO_BIN_EXE_ventop-ai");
        let output = Command::new(bin)
            .env("VENTOP_SYSFS_ROOT", fixture.path())
            .arg("--json")
            .output()
            .expect("Failed to execute ventop-ai binary");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: serde_json::Value =
            serde_json::from_str(&stdout).expect("Failed to parse JSON output");

        assert!(v["gpu"]["cur_freq_mhz"].is_null());
        assert!(v["gpu"]["governor"].is_null());
        assert!(v["hexagon_npu"]["throttle_level"].is_null());
        assert!(v["hexagon_npu"]["dma_buf_allocated_mb"].is_null());
        assert!(v["power_fan"]["board_power_watts"].is_null());
        assert!(v["power_fan"]["fan_rpm"].is_null());
    }
}

// ============================================================================
// SUITE 3: Daemon-Only FastRPC Filtering
// ============================================================================
#[test]
fn test_suite3_daemon_only_fastrpc_filtering() {
    let fixture = MockPlatformFixture::new();

    // CDSP remoteproc is running
    fixture.add_remoteproc("remoteproc1", "qcom,cdsp", "running");
    fixture.add_dev_node("fastrpc-cdsp");
    fixture.add_dev_node("fastrpc-adsp");

    // Populate system daemons in mock /proc:
    // PID 201: cdsprpcd holding /dev/fastrpc-cdsp (daemon blacklist test)
    fixture.add_process_with_fds(
        201,
        "cdsprpcd",
        "/usr/sbin/cdsprpcd",
        4_864, // ~19 MB RSS
        &["/dev/fastrpc-cdsp"],
    );

    // PID 202: adsprpcd holding /dev/fastrpc-adsp (audio DSP daemon test)
    fixture.add_process_with_fds(
        202,
        "adsprpcd",
        "/usr/sbin/adsprpcd",
        3_840, // ~15 MB RSS
        &["/dev/fastrpc-adsp"],
    );

    // PID 203: pulseaudio holding /dev/fastrpc-adsp (audio client test)
    fixture.add_process_with_fds(
        203,
        "pulseaudio",
        "/usr/bin/pulseaudio --daemonize=no",
        8_192, // ~32 MB RSS
        &["/dev/fastrpc-adsp"],
    );

    // PID 204: audio-hal holding /dev/fastrpc-adsp (HAL test)
    fixture.add_process_with_fds(
        204,
        "audio-hal",
        "/vendor/bin/hw/audio-hal",
        6_144, // ~24 MB RSS
        &["/dev/fastrpc-adsp"],
    );

    let _guard = fixture.activate();

    // 1. Verify scan_fastrpc_processes strictly rejects all daemons and non-CDSP clients
    let procs = scan_fastrpc_processes();
    assert!(
        procs.is_empty(),
        "Expected zero active NPU processes with daemon-only holders, got: {:?}",
        procs
    );

    // 2. Verify Hexagon snapshot reports Idle and does NOT attribute daemon RSS to VRAM
    let hex = collect_hexagon_snapshot();
    assert!(hex.active_processes.is_empty());
    assert_eq!(hex.vram_allocated_mb, None);
    assert_eq!(hex.activity_status, "Idle");

    // 3. Now add a genuine LLM workload process (PID 601)
    fixture.add_process_with_fds(
        601,
        "llama-server",
        "/opt/llama/bin/llama-server -m /models/qwen2.5-3b.gguf -dev HTP0 --port 8080",
        640_000, // ~2500 MB RSS
        &["/dev/fastrpc-cdsp"],
    );

    // 4. Verify genuine process is now accepted while daemons remain filtered
    let procs_updated = scan_fastrpc_processes();
    assert_eq!(procs_updated.len(), 1);
    let genuine = &procs_updated[0];
    assert_eq!(genuine.pid, 601);
    assert_eq!(genuine.name, "llama-server");
    assert_eq!(genuine.model_name.as_deref(), Some("qwen2.5-3b.gguf"));

    let hex_updated = collect_hexagon_snapshot();
    assert_eq!(hex_updated.active_processes.len(), 1);
    assert_eq!(hex_updated.vram_allocated_mb, Some(2500));
    assert!(hex_updated.vram_is_estimated);
    assert_eq!(hex_updated.activity_status, "Idle (Model Loaded)");
}

// ============================================================================
// SUITE 4: High Board Power Without NPU Client
// ============================================================================
#[test]
fn test_suite4_high_board_power_without_npu_client() {
    let fixture = MockPlatformFixture::new();

    // CDSP remoteproc is running
    fixture.add_remoteproc("remoteproc1", "qcom,cdsp", "running");
    fixture.add_dev_node("fastrpc-cdsp");

    // Simulate high board power draw: 18.5 W (well above 11.3 W baseline)
    fixture.add_hwmon_power("hwmon0", "ina232", 18_500_000, 12_000, 1_541);

    let _guard = fixture.activate();

    // Scenario A: High power (18.5 W -> +7.2 W delta), but zero active NPU processes
    let (status_a, pwr_a) = evaluate_inference_activity(0.0, 7.2, 45, 0, false);
    assert_eq!(status_a, "Idle");
    assert_eq!(pwr_a, 7.2);

    // Scenario B: Extreme power (25.0 W -> +13.7 W delta), high polling CPU (65%), hot chip (58°C), zero processes
    let (status_b, pwr_b) = evaluate_inference_activity(65.0, 13.7, 58, 200, false);
    assert_eq!(status_b, "Idle");
    assert_eq!(pwr_b, 13.7);

    // Verify status NEVER latches to Active or Model Loaded
    assert_ne!(status_b, "Active (Token Generation)");
    assert_ne!(status_b, "Active (Prompt Processing)");
    assert_ne!(status_b, "Active (FastRPC RPC)");
    assert_ne!(status_b, "Idle (Model Loaded)");

    // Verify snapshot activity status
    let hex = collect_hexagon_snapshot();
    assert_eq!(hex.activity_status, "Idle");
    assert_eq!(hex.power_above_idle_w, 7.2);

    // Verify SummaryTracker records 0 active NPU samples despite high power
    let mut tracker = SummaryTracker::default();
    let sys = sysinfo::System::new();
    let snap = collect_snapshot_continuous(&sys, &[], false);
    tracker.record_sample(&snap, 50.0);
    assert_eq!(tracker.npu_active_samples, 0);

    // CLI assertion: output JSON reports "Idle" status
    #[cfg(not(target_arch = "aarch64"))]
    {
        let bin = env!("CARGO_BIN_EXE_ventop-ai");
        let output = Command::new(bin)
            .env("VENTOP_SYSFS_ROOT", fixture.path())
            .arg("--json")
            .output()
            .expect("Failed to execute ventop-ai binary");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: serde_json::Value =
            serde_json::from_str(&stdout).expect("Failed to parse JSON output");

        assert_eq!(v["hexagon_npu"]["activity_status"], "Idle");
        assert_eq!(v["power_fan"]["board_power_watts"], 18.5);
    }
}

// ============================================================================
// SUITE 5: Multiple GLINK Interrupt Lines
// ============================================================================
#[test]
fn test_suite5_multiple_glink_interrupt_lines() {
    let fixture = MockPlatformFixture::new();

    // Multi-core interrupts file containing ADSP first, CDSP second, MPSS third
    let interrupts_sample = r#"
           CPU0       CPU1       CPU2       CPU3       CPU4       CPU5       CPU6       CPU7
 25:          0          0          0          0          0          0          0          0     GICv3  25 Level     arch_timer
128:       5420       1200        800        650        320        210        150         80     GICv3 128 Edge      glink-smem:adsp
129:       9800       4500       3200       2100       1100        800        600        400     GICv3 129 Edge      glink-smem:cdsp
130:        120         50         40         30         10          5          2          1     GICv3 130 Edge      glink-smem:mpss
"#;
    fixture.set_proc_interrupts(interrupts_sample);

    let _guard = fixture.activate();

    // 1. Direct parser verification: ADSP sum is 8830; CDSP sum is 22500
    let count = parse_glink_interrupt_count_from_str(interrupts_sample);
    assert_eq!(
        count,
        Some(22500),
        "Parser should isolate CDSP line (22500) and ignore ADSP line (8830)"
    );

    // 2. Sysfs root resolution via read_glink_interrupt_count()
    let read_count = read_glink_interrupt_count();
    assert_eq!(read_count, Some(22500));

    // 3. Reversed order verification (CDSP appears before ADSP)
    let reversed_sample = r#"
 50:       1000       2000       3000       4000          0          0          0          0     GICv3  50 Edge      glink-smem:cdsp
 51:       9000       9000          0          0          0          0          0          0     GICv3  51 Edge      glink-smem:adsp
"#;
    assert_eq!(
        parse_glink_interrupt_count_from_str(reversed_sample),
        Some(10000)
    );

    // 4. Compute alias verification (glink-edge-compute)
    let compute_alias_sample = r#"
 72:        500        500          0          0          0          0          0          0     GICv3  72 Edge      glink-edge-compute
"#;
    assert_eq!(
        parse_glink_interrupt_count_from_str(compute_alias_sample),
        Some(1000)
    );

    // 5. Absent CDSP line returns None
    let adsp_only_sample = r#"
 80:       1000       1000          0          0          0          0          0          0     GICv3  80 Edge      glink-smem:adsp
"#;
    assert_eq!(parse_glink_interrupt_count_from_str(adsp_only_sample), None);
}

// ============================================================================
// SUITE 6: Interval Delta CPU Percentages Sum
// ============================================================================
#[test]
fn test_suite6_interval_delta_cpu_percentages_sum() {
    let fixture = MockPlatformFixture::new();

    // 1. Analytical verification across consecutive tick increments
    let s0 = CpuStats {
        user: 10_000,
        nice: 500,
        system: 2_000,
        idle: 80_000,
        iowait: 100,
        irq: 50,
        softirq: 20,
    };

    let s1 = CpuStats {
        user: 10_600,  // +600 user
        nice: 500,     // +0 nice
        system: 2_300, // +300 system
        idle: 80_800,  // +800 idle
        iowait: 150,   // +50 iowait
        irq: 90,       // +40 irq (allocated to system)
        softirq: 40,   // +20 softirq (allocated to system)
    };

    let (u1, s_pct1, io1, id1) = compute_cpu_pct_from_stats(&s1, Some(&s0));
    let sum1 = u1 + s_pct1 + io1 + id1;
    assert!(
        (sum1 - 100.0).abs() < 1e-4,
        "Percentages must sum to 100.0%, got: {} (u={}, s={}, io={}, id={})",
        sum1,
        u1,
        s_pct1,
        io1,
        id1
    );

    // Total delta: 600 + 300 + 800 + 50 + 40 + 20 = 1810
    // user = 600 / 1810 = ~33.149%
    // system = (300 + 40 + 20) / 1810 = 360 / 1810 = ~19.889%
    // iowait = 50 / 1810 = ~2.762%
    // idle = 800 / 1810 = ~44.198%
    assert!((u1 - 33.149).abs() < 0.01);
    assert!((s_pct1 - 19.889).abs() < 0.01);
    assert!((io1 - 2.762).abs() < 0.01);
    assert!((id1 - 44.198).abs() < 0.01);

    // 2. Advancing to next tick step with high I/O wait and interrupts
    let s2 = CpuStats {
        user: 11_100,  // +500
        nice: 550,     // +50
        system: 2_600, // +300
        idle: 81_200,  // +400
        iowait: 450,   // +300
        irq: 190,      // +100
        softirq: 90,   // +50
    };
    let (u2, s_pct2, io2, id2) = compute_cpu_pct_from_stats(&s2, Some(&s1));
    let sum2 = u2 + s_pct2 + io2 + id2;
    assert!((sum2 - 100.0).abs() < 1e-4);

    // 3. Advancing mock /proc/stat file across consecutive live snapshot ticks
    let _guard = fixture.activate();

    fixture.set_proc_stat("cpu  10000 500 2000 80000 100 50 20 0 0 0\n");
    let sys = sysinfo::System::new();
    let snap1 = collect_snapshot_continuous(&sys, &[], false);
    let sum_snap1 =
        snap1.cpu.user_pct + snap1.cpu.system_pct + snap1.cpu.iowait_pct + snap1.cpu.idle_pct;
    assert!((sum_snap1 - 100.0).abs() < 0.01);

    // Advance /proc/stat
    fixture.set_proc_stat("cpu  10600 500 2300 80800 150 90 40 0 0 0\n");
    let snap2 = collect_snapshot_continuous(&sys, &[], false);
    let sum_snap2 =
        snap2.cpu.user_pct + snap2.cpu.system_pct + snap2.cpu.iowait_pct + snap2.cpu.idle_pct;
    assert!(
        (sum_snap2 - 100.0).abs() < 0.01,
        "Snapshot delta CPU % must sum to ~100.0%, got: {}",
        sum_snap2
    );

    // Advance /proc/stat again
    fixture.set_proc_stat("cpu  11250 520 2700 81500 200 120 70 0 0 0\n");
    let snap3 = collect_snapshot_continuous(&sys, &[], false);
    let sum_snap3 =
        snap3.cpu.user_pct + snap3.cpu.system_pct + snap3.cpu.iowait_pct + snap3.cpu.idle_pct;
    assert!(
        (sum_snap3 - 100.0).abs() < 0.01,
        "Snapshot delta CPU % must sum to ~100.0%, got: {}",
        sum_snap3
    );

    // 4. CLI JSON stream output verification
    #[cfg(not(target_arch = "aarch64"))]
    {
        let bin = env!("CARGO_BIN_EXE_ventop-ai");
        let output = Command::new(bin)
            .env("VENTOP_SYSFS_ROOT", fixture.path())
            .arg("--json")
            .output()
            .expect("Failed to execute ventop-ai binary");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: serde_json::Value =
            serde_json::from_str(&stdout).expect("Failed to parse JSON output");

        let u = v["cpu"]["user_pct"].as_f64().unwrap();
        let s = v["cpu"]["system_pct"].as_f64().unwrap();
        let io = v["cpu"]["iowait_pct"].as_f64().unwrap();
        let id = v["cpu"]["idle_pct"].as_f64().unwrap();
        let total = u + s + io + id;
        assert!(
            (total - 100.0).abs() < 0.01,
            "CLI JSON CPU % must sum to ~100.0%, got: {}",
            total
        );
    }
}
