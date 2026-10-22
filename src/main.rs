// SPDX-License-Identifier: BSD-3-Clause AND MIT
// Portions Copyright (c) 2025 Alex Jokela and contributors (BSD-3-Clause)
// Portions Copyright (c) 2026 Piotr Błażejewicz and contributors (MIT)

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::{Duration, Instant};
use sysinfo::System;

use ventop_ai::action_brain::*;
use ventop_ai::hardware::*;
use ventop_ai::hexagon::*;
use ventop_ai::snapshot::{
    print_json_snapshot, print_oneshot_snapshot, print_schema, stream_json_snapshots, StreamOptions,
};
use ventop_ai::sysinfo_ext::{get_top_processes, ProcessSortMode};
use ventop_ai::ui::{render_ui, UiState};

fn print_help() {
    println!(
        "ventop-ai {} - Arduino Ventuno Q Dual-Brain & Hexagon NPU System Monitor",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("USAGE:");
    println!("    ventop-ai [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!(
        "    -j, --json               Output a single JSON telemetry snapshot to stdout and exit"
    );
    println!("    -1, --oneshot            Output a single plaintext telemetry snapshot to stdout and exit");
    println!("    -s, --stream [MS]        Stream continuous NDJSON snapshots at interval in ms (default: 200ms)");
    println!("    -w, --watch [SECS]       Stream continuous NDJSON snapshots at interval in seconds (default: 1s)");
    println!("    -d, --duration <SECS>    Run streaming session for specified duration in seconds and exit");
    println!(
        "    --summary                Print run summary report (min/median/p90/max) at stream exit"
    );
    println!("    --active-threshold-pct <PCT> CPU threshold % for tracked process co-activity (default: 50.0)");
    println!("    --calibrate-idle         Calibrate board idle power baseline at startup from live sensor");
    println!("    --idle-baseline-w <W>    Override board idle power baseline in Watts (default: 11.3 W)");
    println!("    --mark-file <PATH>       Inject phase event markers from file into top-level .marker field");
    println!(
        "    --schema                 Print comprehensive JSON schema & field dictionary and exit"
    );
    println!("    -p, --proc <names>       Track specific processes & thread-to-core placement (comma-separated)");
    println!("    -t, --threads            Include thread-level placement detail in JSON stream");
    println!("    -v, --version            Print version information and exit");
    println!("    -h, --help               Print this help message and exit");
    println!();
    println!("INTERACTIVE TUI CONTROLS:");
    println!("    q, Q, Esc        Quit");
    println!("    ▲ / ▼ or j / k   Scroll process table");
    println!("    PgUp / PgDn      Scroll process table by 10 rows");
    println!("    Home / g         Jump to top of process table");
    println!("    c / m / p / n    Sort processes by CPU / Memory / PID / Name");
    println!("    /                Filter processes by name");
    println!();
    println!("STREAMING & SCRIPTING EXAMPLES:");
    println!("    ./ventop-ai --json | jq .hexagon_npu");
    println!("    ./ventop-ai --stream 200 --proc llama-server,arduino-router");
    println!("    ./ventop-ai --stream 500 --duration 60 --summary --proc llama-server");
    println!("    ./ventop-ai --schema");
    println!("    ./ventop-ai --oneshot --proc llama");
    println!();
    println!("NOTES:");
    println!("    * Real-time Hexagon NPU monitoring tracks FastRPC sessions, DMA-BUF VRAM, and GLINK mailbox rates.");
    println!("    * Dual-Brain architecture monitors both the AI Brain (QCS8275) and the Action Brain (STM32H5).");
    println!("    * Thread placement maps threads to Big (Gold/Prime A78C) vs Little (Silver A55) CPU clusters.");
    println!("    * Event markers (--mark-file) are injected directly into the snapshot's top-level .marker field, while process starts/exits appear in .events[].");
}

fn main() -> Result<()> {
    // Install panic hook to restore terminal
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let msg = panic_info.to_string();
        if !msg.contains("Broken pipe") {
            default_hook(panic_info);
        }
    }));

    let args: Vec<String> = std::env::args().collect();
    let mut tracked_procs: Vec<String> = Vec::new();
    let mut stream_ms: Option<u64> = None;
    let mut watch_secs: Option<u64> = None;
    let mut duration_secs: Option<u64> = None;
    let mut summary_mode = false;
    let mut active_threshold_pct: f32 = 50.0;
    let mut mark_file: Option<String> = None;
    let mut json_mode = false;
    let mut oneshot_mode = false;
    let mut include_threads = false;
    let mut calibrate_idle = false;
    let mut explicit_idle_baseline = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            "-v" | "--version" => {
                println!("ventop-ai {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--schema" => {
                print_schema();
                return Ok(());
            }
            "-j" | "--json" => {
                json_mode = true;
            }
            "-1" | "--oneshot" | "--snapshot" => {
                oneshot_mode = true;
            }
            "-t" | "--threads" => {
                include_threads = true;
            }
            "--summary" => {
                summary_mode = true;
            }
            "-d" | "--duration" => {
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                    match args[i].parse::<u64>() {
                        Ok(d) => duration_secs = Some(d),
                        Err(_) => {
                            eprintln!("error: invalid value '{}' for '{}'", args[i], args[i - 1]);
                            eprintln!("For more information, try '--help'");
                            std::process::exit(2);
                        }
                    }
                } else {
                    eprintln!("error: option requires an argument: '{}'", args[i]);
                    eprintln!("For more information, try '--help'");
                    std::process::exit(2);
                }
            }
            "--active-threshold-pct" => {
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                    match args[i].parse::<f32>() {
                        Ok(val) => active_threshold_pct = val,
                        Err(_) => {
                            eprintln!("error: invalid value '{}' for '{}'", args[i], args[i - 1]);
                            eprintln!("For more information, try '--help'");
                            std::process::exit(2);
                        }
                    }
                } else {
                    eprintln!("error: option requires an argument: '{}'", args[i]);
                    eprintln!("For more information, try '--help'");
                    std::process::exit(2);
                }
            }
            "--calibrate-idle" => {
                calibrate_idle = true;
            }
            "--idle-baseline-w" => {
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                    match args[i].parse::<f64>() {
                        Ok(w) => {
                            set_idle_baseline_watts(w);
                            explicit_idle_baseline = true;
                        }
                        Err(_) => {
                            eprintln!("error: invalid value '{}' for '{}'", args[i], args[i - 1]);
                            eprintln!("For more information, try '--help'");
                            std::process::exit(2);
                        }
                    }
                } else {
                    eprintln!("error: option requires an argument: '{}'", args[i]);
                    eprintln!("For more information, try '--help'");
                    std::process::exit(2);
                }
            }
            "--mark-file" => {
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                    mark_file = Some(args[i].clone());
                } else {
                    eprintln!("error: option requires an argument: '{}'", args[i]);
                    eprintln!("For more information, try '--help'");
                    std::process::exit(2);
                }
            }
            "-s" | "--stream" => {
                let ms = if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                    match args[i].parse::<u64>() {
                        Ok(val) => val,
                        Err(_) => {
                            eprintln!("error: invalid value '{}' for '{}'", args[i], args[i - 1]);
                            eprintln!("For more information, try '--help'");
                            std::process::exit(2);
                        }
                    }
                } else {
                    200
                };
                stream_ms = Some(ms);
            }
            "-w" | "--watch" => {
                let secs = if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                    match args[i].parse::<u64>() {
                        Ok(val) => val,
                        Err(_) => {
                            eprintln!("error: invalid value '{}' for '{}'", args[i], args[i - 1]);
                            eprintln!("For more information, try '--help'");
                            std::process::exit(2);
                        }
                    }
                } else {
                    1
                };
                watch_secs = Some(secs);
            }
            "-p" | "--proc" => {
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                    for name in args[i].split(',') {
                        let trimmed = name.trim().to_string();
                        if !trimmed.is_empty() {
                            tracked_procs.push(trimmed);
                        }
                    }
                } else {
                    eprintln!("error: option requires an argument: '{}'", args[i]);
                    eprintln!("For more information, try '--help'");
                    std::process::exit(2);
                }
            }
            arg if arg.starts_with('-') => {
                eprintln!("error: unrecognized option '{}'", arg);
                eprintln!("For more information, try '--help'");
                std::process::exit(2);
            }
            unknown => {
                eprintln!("error: unexpected argument '{}'", unknown);
                eprintln!("For more information, try '--help'");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    if calibrate_idle && !explicit_idle_baseline {
        match calibrate_idle_baseline(3, Duration::from_millis(200)) {
            Some(w) => {
                eprintln!("[ventop-ai] Calibrated idle power baseline: {:.2} W", w);
            }
            None => {
                eprintln!(
                    "[ventop-ai] Hardware power monitor unavailable; using default baseline ({:.1} W)",
                    get_idle_baseline_watts()
                );
            }
        }
    }

    if json_mode {
        print_json_snapshot(&tracked_procs, include_threads);
        return Ok(());
    }

    if oneshot_mode {
        print_oneshot_snapshot(&tracked_procs, include_threads);
        return Ok(());
    }

    if stream_ms.is_some()
        || watch_secs.is_some()
        || summary_mode
        || duration_secs.is_some()
        || mark_file.is_some()
    {
        let ms = stream_ms.unwrap_or_else(|| watch_secs.map(|s| s * 1000).unwrap_or(200));
        let opts = StreamOptions {
            interval_ms: ms,
            tracked_patterns: tracked_procs,
            include_threads,
            duration_secs,
            summary: summary_mode,
            mark_file,
            active_threshold_pct,
        };
        stream_json_snapshots(opts);
        return Ok(());
    }

    // Launch interactive TUI
    run_tui()
}

fn run_tui() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut sys = System::new_all();
    sys.refresh_cpu_all();
    sys.refresh_memory();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut ui_state = UiState::default();
    let tick_rate = Duration::from_millis(1000);
    let mut last_tick = Instant::now();

    let per_core_pct: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
    let mut clusters = get_cpu_clusters_with_load(&per_core_pct);
    let mut cpu_freqs = get_cpu_frequencies();
    let mut gpu = get_adreno_gpu_metrics();
    let mut power_fan = get_hardware_power_fan();
    let mut hexagon = collect_hexagon_snapshot_with_power(power_fan.board_power_watts);
    let mut action_brain = collect_action_brain_snapshot();
    let mut thermals = get_consolidated_thermals();
    let mut top_processes = get_top_processes(&sys, 100, ui_state.process_sort);

    loop {
        let uptime = System::uptime();

        terminal.draw(|f| {
            render_ui(
                f,
                &sys,
                &ui_state,
                &clusters,
                &cpu_freqs,
                &gpu,
                &hexagon,
                &action_brain,
                &thermals,
                &power_fan,
                &top_processes,
                uptime,
            );
        })?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_millis(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if ui_state.filter_mode {
                        match key.code {
                            KeyCode::Esc | KeyCode::Enter => {
                                ui_state.filter_mode = false;
                            }
                            KeyCode::Backspace => {
                                ui_state.process_filter.pop();
                                ui_state.scroll_offset = 0;
                            }
                            KeyCode::Char(c) => {
                                ui_state.process_filter.push(c);
                                ui_state.scroll_offset = 0;
                            }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                                break;
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                let matching_count = top_processes
                                    .iter()
                                    .filter(|p| {
                                        ui_state.process_filter.is_empty()
                                            || p.name
                                                .to_lowercase()
                                                .contains(&ui_state.process_filter.to_lowercase())
                                    })
                                    .count();
                                if ui_state.scroll_offset + 1 < matching_count {
                                    ui_state.scroll_offset += 1;
                                }
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                ui_state.scroll_offset = ui_state.scroll_offset.saturating_sub(1);
                            }
                            KeyCode::PageDown => {
                                let matching_count = top_processes
                                    .iter()
                                    .filter(|p| {
                                        ui_state.process_filter.is_empty()
                                            || p.name
                                                .to_lowercase()
                                                .contains(&ui_state.process_filter.to_lowercase())
                                    })
                                    .count();
                                ui_state.scroll_offset = (ui_state.scroll_offset + 10)
                                    .min(matching_count.saturating_sub(1));
                            }
                            KeyCode::PageUp => {
                                ui_state.scroll_offset = ui_state.scroll_offset.saturating_sub(10);
                            }
                            KeyCode::Home | KeyCode::Char('g') => {
                                ui_state.scroll_offset = 0;
                            }
                            KeyCode::Char('c') => {
                                ui_state.process_sort = ProcessSortMode::CpuDesc;
                                ui_state.scroll_offset = 0;
                                top_processes = get_top_processes(&sys, 100, ui_state.process_sort);
                            }
                            KeyCode::Char('m') => {
                                ui_state.process_sort = ProcessSortMode::MemoryDesc;
                                ui_state.scroll_offset = 0;
                                top_processes = get_top_processes(&sys, 100, ui_state.process_sort);
                            }
                            KeyCode::Char('p') => {
                                ui_state.process_sort = ProcessSortMode::PidAsc;
                                ui_state.scroll_offset = 0;
                                top_processes = get_top_processes(&sys, 100, ui_state.process_sort);
                            }
                            KeyCode::Char('n') => {
                                ui_state.process_sort = ProcessSortMode::NameAsc;
                                ui_state.scroll_offset = 0;
                                top_processes = get_top_processes(&sys, 100, ui_state.process_sort);
                            }
                            KeyCode::Char('/') => {
                                ui_state.filter_mode = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            sys.refresh_cpu_all();
            sys.refresh_memory();
            sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

            let per_core_pct: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
            clusters = get_cpu_clusters_with_load(&per_core_pct);
            cpu_freqs = get_cpu_frequencies();
            gpu = get_adreno_gpu_metrics();
            power_fan = get_hardware_power_fan();
            hexagon = collect_hexagon_snapshot_with_power(power_fan.board_power_watts);
            action_brain = collect_action_brain_snapshot();
            thermals = get_consolidated_thermals();
            top_processes = get_top_processes(&sys, 100, ui_state.process_sort);

            last_tick = Instant::now();
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
