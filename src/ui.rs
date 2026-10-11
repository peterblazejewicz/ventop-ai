// SPDX-License-Identifier: BSD-3-Clause AND MIT
// Portions Copyright (c) 2025 Alex Jokela and contributors (BSD-3-Clause)
// Portions Copyright (c) 2026 Piotr Błażejewicz and contributors (MIT)

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
    Frame,
};
use sysinfo::System;

use crate::action_brain::ActionBrainSnapshot;
use crate::hardware::{AdrenoGpuMetrics, CpuCluster, HardwarePowerFan, VentunoThermals};
use crate::hexagon::HexagonNpuSnapshot;
use crate::sysinfo_ext::{ProcessInfo, ProcessSortMode};

pub struct UiState {
    pub process_sort: ProcessSortMode,
    pub process_filter: String,
    pub filter_mode: bool,
    pub scroll_offset: usize,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            process_sort: ProcessSortMode::CpuDesc,
            process_filter: String::new(),
            filter_mode: false,
            scroll_offset: 0,
        }
    }
}

pub fn render_ui(
    frame: &mut Frame,
    sys: &System,
    ui_state: &UiState,
    clusters: &[CpuCluster],
    _cpu_freqs: &[u32],
    gpu: &AdrenoGpuMetrics,
    hexagon: &HexagonNpuSnapshot,
    action_brain: &ActionBrainSnapshot,
    thermals: &VentunoThermals,
    power_fan: &HardwarePowerFan,
    top_processes: &[ProcessInfo],
    uptime_secs: u64,
) {
    let size = frame.area();

    // Main layout: Header, Top Grid (CPU & Dual Brain), Mid Grid (Memory & Thermals/Power), Process Table
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Length(12), // Top Grid (CPU + Dual Brain)
            Constraint::Length(6),  // Mid Grid (Memory + Thermals/Power)
            Constraint::Min(8),     // Process table
            Constraint::Length(1),  // Status / key help
        ])
        .split(size);

    render_header(frame, chunks[0], uptime_secs);
    render_top_grid(frame, chunks[1], sys, clusters, gpu, hexagon, action_brain);
    render_mid_grid(frame, chunks[2], sys, thermals, power_fan);
    render_process_table(frame, chunks[3], top_processes, ui_state);
    render_footer(frame, chunks[4]);
}

fn render_header(frame: &mut Frame, area: Rect, uptime_secs: u64) {
    let hours = uptime_secs / 3600;
    let mins = (uptime_secs % 3600) / 60;
    let secs = uptime_secs % 60;

    let header_text = vec![Line::from(vec![
        Span::styled(
            " VENTOP-AI ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  Arduino Ventuno Q ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "[Qualcomm Snapdragon QCS8275 + STM32H5]",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" | "),
        Span::styled(
            format!("Uptime: {:02}:{:02}:{:02}", hours, mins, secs),
            Style::default().fg(Color::Green),
        ),
        Span::raw(" | "),
        Span::styled(
            "DUAL-BRAIN EDGE AI PLATFORM",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
    ])];

    let header = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(header, area);
}

fn render_top_grid(
    frame: &mut Frame,
    area: Rect,
    sys: &System,
    clusters: &[CpuCluster],
    gpu: &AdrenoGpuMetrics,
    hexagon: &HexagonNpuSnapshot,
    action_brain: &ActionBrainSnapshot,
) {
    let top_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(42), // CPU & Clusters
            Constraint::Percentage(58), // Dual-Brain (Hexagon NPU + STM32H5)
        ])
        .split(area);

    render_cpu_panel(frame, top_cols[0], sys, clusters);
    render_dual_brain_panel(frame, top_cols[1], gpu, hexagon, action_brain);
}

fn render_cpu_panel(frame: &mut Frame, area: Rect, sys: &System, clusters: &[CpuCluster]) {
    let cpus = sys.cpus();
    let total_cpu = if !cpus.is_empty() {
        cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
    } else {
        0.0
    };

    let mut lines = Vec::new();

    // Total CPU bar
    let bar_width: usize = 20;
    let filled = ((total_cpu / 100.0) * bar_width as f32).round() as usize;
    let empty = bar_width.saturating_sub(filled);
    let bar = format!("[{}{}]", "#".repeat(filled), "-".repeat(empty));

    lines.push(Line::from(vec![
        Span::styled(
            "Total CPU: ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:5.1}% ", total_cpu),
            Style::default().fg(if total_cpu > 80.0 {
                Color::Red
            } else {
                Color::Green
            }),
        ),
        Span::styled(bar, Style::default().fg(Color::Cyan)),
    ]));

    // Cluster frequencies
    for cluster in clusters {
        let (label, col) = match cluster.name {
            name if name.contains("Gold") => ("Gold (A78C) : ", Color::LightYellow),
            name if name.contains("Prime") => ("Prime (A78C): ", Color::LightRed),
            _ => ("Silver (A55): ", Color::LightGreen),
        };

        lines.push(Line::from(vec![
            Span::styled(label, Style::default().fg(col)),
            Span::styled(
                format!("{:2.0}% load", cluster.load_pct),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" @ "),
            Span::styled(
                format!("{:4} MHz", cluster.cur_freq_mhz),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " [{:4}-{:4} MHz, {}]",
                    cluster.min_freq_mhz, cluster.max_freq_mhz, cluster.governor
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    lines.push(Line::from(Span::styled(
        "─────────────────────────────────────────────────────────",
        Style::default().fg(Color::DarkGray),
    )));

    // Cores grouped by cluster:
    if !clusters.is_empty() {
        for cluster in clusters {
            let (prefix, col) = match cluster.name {
                name if name.contains("Gold") => ("Gold", Color::LightYellow),
                name if name.contains("Prime") => ("Prime", Color::LightRed),
                name if name.contains("Silver") => ("Silver", Color::LightGreen),
                _ => ("Cluster", Color::Cyan),
            };
            let core_range =
                if let (Some(first), Some(last)) = (cluster.cores.first(), cluster.cores.last()) {
                    if first == last {
                        format!("C{}", first)
                    } else {
                        format!("C{}-C{}", first, last)
                    }
                } else {
                    "C?".to_string()
                };
            let label = format!("{:<6} ({:<5}): ", prefix, core_range);
            let mut spans = vec![Span::styled(label, Style::default().fg(col))];
            for &core_idx in &cluster.cores {
                if let Some(cpu) = cpus.get(core_idx as usize) {
                    let u = cpu.cpu_usage();
                    let col = if u > 80.0 {
                        Color::Red
                    } else if u > 50.0 {
                        Color::Yellow
                    } else {
                        Color::Green
                    };
                    spans.push(Span::styled(
                        format!("C{}:{:2.0}% ", core_idx, u),
                        Style::default().fg(col),
                    ));
                }
            }
            lines.push(Line::from(spans));
        }
    } else {
        let mut spans = vec![Span::styled(
            "Cores         : ",
            Style::default().fg(Color::White),
        )];
        for (i, cpu) in cpus.iter().enumerate() {
            let u = cpu.cpu_usage();
            let col = if u > 80.0 {
                Color::Red
            } else if u > 50.0 {
                Color::Yellow
            } else {
                Color::Green
            };
            spans.push(Span::styled(
                format!("C{}:{:2.0}% ", i, u),
                Style::default().fg(col),
            ));
        }
        lines.push(Line::from(spans));
    }

    let cpu_block = Paragraph::new(lines).block(
        Block::default()
            .title(format!(" CPU Subsystem ({} Cores) ", cpus.len()))
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(cpu_block, area);
}

fn render_dual_brain_panel(
    frame: &mut Frame,
    area: Rect,
    gpu: &AdrenoGpuMetrics,
    hexagon: &HexagonNpuSnapshot,
    action_brain: &ActionBrainSnapshot,
) {
    let mut lines = Vec::new();

    // 1. The AI Brain Header
    let state_color = if hexagon.is_available {
        Color::Green
    } else {
        Color::Red
    };

    // Detect live or latched speed across active processes for the header
    let mut header_activity: Option<(String, Color)> = None;
    for p in &hexagon.active_processes {
        if let Some(ref em) = p.engine_metrics {
            if em.slot_state == "generating_tokens" && em.decode_tokens_per_sec > 0.0 {
                header_activity = Some((
                    format!("Active ({:.1} tok/s)", em.decode_tokens_per_sec),
                    Color::LightGreen,
                ));
                break;
            } else if em.slot_state == "processing_prompt" && em.prompt_tokens_per_sec > 0.0 {
                header_activity = Some((
                    format!("Active (Prefill: {:.0} T/s)", em.prompt_tokens_per_sec),
                    Color::LightYellow,
                ));
                break;
            } else if em.slot_state == "idle" {
                if let (Some(tps), Some(age)) = (em.latched_decode_tps, em.seconds_since_active) {
                    if age < 5 && header_activity.is_none() {
                        header_activity =
                            Some((format!("Idle (last: {:.1} tok/s)", tps), Color::Cyan));
                    }
                }
            }
        }
    }

    let (status_text, act_color) = if let Some((custom, col)) = header_activity {
        (custom, col)
    } else {
        let col = match hexagon.activity_status {
            s if s.contains("High") || s.contains("Prompt") => Color::LightRed,
            s if s.contains("Active") || s.contains("Token") => Color::LightGreen,
            s if s.contains("Light") => Color::LightCyan,
            _ => Color::DarkGray,
        };
        (hexagon.activity_status.to_string(), col)
    };

    lines.push(Line::from(vec![
        Span::styled(
            "◆ THE AI BRAIN: ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "Qualcomm Hexagon {} ({} TOPS)",
                hexagon.core_id, hexagon.tops_rating
            ),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::styled(&hexagon.remoteproc_state, Style::default().fg(state_color)),
        Span::raw(" ["),
        Span::styled(
            status_text,
            Style::default().fg(act_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("]"),
    ]));

    // 2. NPU Core Silicon Geometry
    lines.push(Line::from(vec![
        Span::styled("  Core Geometry: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}x Threads", hexagon.hw_threads),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}x HVX (1024b)", hexagon.hvx_units),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}x HMX (Matrix)", hexagon.hmx_units),
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}MB VTCM", hexagon.vtcm_sram_mb),
            Style::default().fg(Color::LightCyan),
        ),
    ]));

    // 3. Silicon Multi-Zone Thermals & Throttling & Interrupts
    let fmt_temp = |val: Option<i32>| -> (String, Color) {
        match val {
            Some(t) => {
                let color = if t >= 80 {
                    Color::Red
                } else if t >= 65 {
                    Color::Yellow
                } else {
                    Color::Green
                };
                (format!("{}°C", t), color)
            }
            None => ("-".to_string(), Color::DarkGray),
        }
    };

    let (c_str, c_col) = fmt_temp(hexagon.silicon_thermals.core_vector_celsius);
    let (m_str, m_col) = fmt_temp(hexagon.silicon_thermals.matrix_hmx_celsius);
    let (v_str, v_col) = fmt_temp(hexagon.silicon_thermals.vtcm_sram_celsius);

    let (throttle_str, throttle_col) = match hexagon.throttle_level {
        Some(val) if val > 0 => (format!("{}/4", val), Color::Red),
        Some(val) => (format!("{}/4", val), Color::Green),
        None => ("-".to_string(), Color::DarkGray),
    };
    let irq_str = hexagon
        .glink_interrupts_per_sec
        .map(|i| format!(" | IRQs: {}/s", i))
        .unwrap_or_default();

    lines.push(Line::from(vec![
        Span::styled("  Thermals: ", Style::default().fg(Color::DarkGray)),
        Span::styled("Core:", Style::default().fg(Color::DarkGray)),
        Span::styled(c_str, Style::default().fg(c_col)),
        Span::raw(" "),
        Span::styled("Mat:", Style::default().fg(Color::DarkGray)),
        Span::styled(m_str, Style::default().fg(m_col)),
        Span::raw(" "),
        Span::styled("VTCM:", Style::default().fg(Color::DarkGray)),
        Span::styled(v_str, Style::default().fg(v_col)),
        Span::styled(" | Throt: ", Style::default().fg(Color::DarkGray)),
        Span::styled(throttle_str, Style::default().fg(throttle_col)),
        Span::styled(irq_str, Style::default().fg(Color::Cyan)),
    ]));

    // 4 & 5. Active LLM Workloads & HTP Offload Parameters
    let vram_label = if hexagon.vram_is_estimated {
        "VRAM Alloc(est): "
    } else {
        "VRAM Alloc(DMA): "
    };
    let vram_val = hexagon
        .vram_allocated_mb
        .map(|m| format!("{}MB", m))
        .unwrap_or_else(|| "Dynamic".to_string());
    let inner_width = area.width.saturating_sub(2);

    if !hexagon.active_processes.is_empty() {
        if hexagon.active_processes.len() == 1 {
            let p = &hexagon.active_processes[0];
            let raw_name = p.model_name.as_deref().unwrap_or(&p.name);
            let model_title = raw_name.strip_suffix(".gguf").unwrap_or(raw_name);
            let port_str = p.port.map(|pt| format!(":{}", pt)).unwrap_or_default();

            let label = if p.model_name.is_some() {
                "  LLM Model: "
            } else {
                "  Workload: "
            };

            let mut title_spans = vec![
                Span::styled(label, Style::default().fg(Color::DarkGray)),
                Span::styled(
                    model_title,
                    Style::default()
                        .fg(Color::LightGreen)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" [PID {}{}]", p.pid, port_str),
                    Style::default().fg(Color::Yellow),
                ),
            ];
            if p.polling_cpu_pct > 5.0 {
                title_spans.push(Span::styled(
                    format!(" (Poll CPU: {:.0}%)", p.polling_cpu_pct),
                    Style::default()
                        .fg(Color::LightRed)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if let Some(ref em) = p.engine_metrics {
                if em.slot_state == "generating_tokens" {
                    title_spans.push(Span::styled(
                        format!(" ⚡ {:.1} tok/s", em.decode_tokens_per_sec),
                        Style::default()
                            .fg(Color::LightGreen)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else if em.slot_state == "processing_prompt" {
                    title_spans.push(Span::styled(
                        format!(" ⚡ Prefill: {:.0} T/s", em.prompt_tokens_per_sec),
                        Style::default()
                            .fg(Color::LightYellow)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else if let (Some(tps), Some(age)) =
                    (em.latched_decode_tps, em.seconds_since_active)
                {
                    let col = if age < 5 {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    };
                    let s = if let Some(ptps) = em.latched_prompt_tps {
                        format!(" [last: {:.1} T/s (P:{:.0})]", tps, ptps)
                    } else {
                        format!(" [last: {:.1} T/s]", tps)
                    };
                    title_spans.push(Span::styled(s, Style::default().fg(col)));
                } else {
                    title_spans.push(Span::styled(
                        " [idle]",
                        Style::default().fg(Color::DarkGray),
                    ));
                }
            }
            lines.push(Line::from(title_spans));

            let mut offload_spans = vec![
                Span::styled("  Offload: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("dev:{}", p.target_device),
                    Style::default().fg(Color::Cyan),
                ),
            ];
            if let Some(ngl) = p.n_gpu_layers {
                offload_spans.push(Span::styled(
                    format!(" ngl:{}", ngl),
                    Style::default().fg(Color::LightMagenta),
                ));
            }
            if let Some(thr) = p.n_threads {
                offload_spans.push(Span::styled(
                    format!(" thr:{}", thr),
                    Style::default().fg(Color::Yellow),
                ));
            }
            if p.flash_attn {
                offload_spans.push(Span::styled(" fa:on", Style::default().fg(Color::Green)));
            }
            if let Some(ctx) = p.context_size {
                offload_spans.push(Span::styled(
                    format!(" ctx:{}", ctx),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            offload_spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
            offload_spans.push(Span::styled(
                vram_label,
                Style::default().fg(Color::DarkGray),
            ));
            offload_spans.push(Span::styled(
                vram_val,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::from(offload_spans));
        } else {
            // 2 or more processes
            for (idx, p) in hexagon.active_processes.iter().take(2).enumerate() {
                let raw_name = p.model_name.as_deref().unwrap_or(&p.name);
                let clean_name = raw_name.strip_suffix(".gguf").unwrap_or(raw_name);
                let port_str = p.port.map(|pt| format!(":{}", pt)).unwrap_or_default();
                let poll_str = if p.polling_cpu_pct > 5.0 {
                    format!(" poll:{:.0}%", p.polling_cpu_pct)
                } else {
                    String::new()
                };

                // Determine TPS and styling
                let (tps_span, tps_len) = match p.engine_metrics.as_ref() {
                    Some(em) if em.slot_state == "generating_tokens" => {
                        let text = if inner_width < 55 {
                            format!("⚡{:.1}T/s", em.decode_tokens_per_sec)
                        } else {
                            format!("⚡ {:.1} tok/s", em.decode_tokens_per_sec)
                        };
                        let len = text.chars().count();
                        (
                            Span::styled(
                                text,
                                Style::default()
                                    .fg(Color::LightGreen)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            len,
                        )
                    }
                    Some(em) if em.slot_state == "processing_prompt" => {
                        let text = if inner_width < 55 {
                            format!("⚡P:{:.0}T/s", em.prompt_tokens_per_sec)
                        } else {
                            format!("⚡ Prefill: {:.0} T/s", em.prompt_tokens_per_sec)
                        };
                        let len = text.chars().count();
                        (
                            Span::styled(
                                text,
                                Style::default()
                                    .fg(Color::LightYellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            len,
                        )
                    }
                    Some(em) => {
                        if let (Some(tps), Some(age)) =
                            (em.latched_decode_tps, em.seconds_since_active)
                        {
                            let col = if age < 5 {
                                Color::Cyan
                            } else {
                                Color::DarkGray
                            };
                            let text = if inner_width < 55 {
                                format!("[{:.1}T/s]", tps)
                            } else if inner_width >= 80 {
                                if let Some(ptps) = em.latched_prompt_tps {
                                    format!("[last: {:.1} T/s (P:{:.0})]", tps, ptps)
                                } else {
                                    format!("[last: {:.1} T/s]", tps)
                                }
                            } else {
                                format!("[last: {:.1} T/s]", tps)
                            };
                            let len = text.chars().count();
                            (Span::styled(text, Style::default().fg(col)), len)
                        } else {
                            let text = "[idle]".to_string();
                            let len = text.len();
                            (
                                Span::styled(text, Style::default().fg(Color::DarkGray)),
                                len,
                            )
                        }
                    }
                    None => {
                        let text = "[idle]".to_string();
                        let len = text.len();
                        (
                            Span::styled(text, Style::default().fg(Color::DarkGray)),
                            len,
                        )
                    }
                };

                let prefix = if inner_width < 55 {
                    format!("  #{}{}: ", idx + 1, port_str)
                } else if p.model_name.is_some() {
                    format!("  LLM #{}{}: ", idx + 1, port_str)
                } else {
                    format!("  Proc #{}{}: ", idx + 1, port_str)
                };

                let vram_str = if inner_width < 55 {
                    format!("[{:.1}G]", p.rss_mb as f32 / 1024.0)
                } else {
                    format!("[{}MB]", p.rss_mb)
                };

                let ngl_str = if inner_width >= 75 {
                    p.n_gpu_layers
                        .map(|l| format!(" ngl:{}", l))
                        .unwrap_or_default()
                } else {
                    String::new()
                };

                // Compute budget for model name to prevent line wrap
                let overhead = prefix.chars().count()
                    + 1
                    + tps_len
                    + 1
                    + poll_str.chars().count()
                    + ngl_str.chars().count()
                    + 1
                    + vram_str.chars().count();
                let max_model_len = (inner_width as usize).saturating_sub(overhead).max(8);
                let display_name = if clean_name.chars().count() > max_model_len {
                    let mut s: String = clean_name
                        .chars()
                        .take(max_model_len.saturating_sub(1))
                        .collect();
                    s.push('…');
                    s
                } else {
                    clean_name.to_string()
                };

                let mut row_spans = vec![
                    Span::styled(prefix, Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        display_name,
                        Style::default()
                            .fg(Color::LightGreen)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    tps_span,
                ];
                if !poll_str.is_empty() {
                    row_spans.push(Span::styled(poll_str, Style::default().fg(Color::LightRed)));
                }
                if !ngl_str.is_empty() {
                    row_spans.push(Span::styled(
                        ngl_str,
                        Style::default().fg(Color::LightMagenta),
                    ));
                }
                row_spans.push(Span::raw(" "));
                row_spans.push(Span::styled(vram_str, Style::default().fg(Color::Yellow)));

                lines.push(Line::from(row_spans));
            }
        }
    } else {
        let fastrpc_label = if hexagon.is_available && hexagon.remoteproc_state == "running" {
            Span::styled("/dev/fastrpc-cdsp ready", Style::default().fg(Color::Green))
        } else {
            Span::styled(
                "/dev/fastrpc-cdsp offline",
                Style::default().fg(Color::DarkGray),
            )
        };
        lines.push(Line::from(vec![
            Span::styled("  Workloads: ", Style::default().fg(Color::DarkGray)),
            Span::styled("Idle", Style::default().fg(Color::DarkGray)),
            Span::styled(" | FastRPC: ", Style::default().fg(Color::DarkGray)),
            fastrpc_label,
        ]));
        let vram_display = if hexagon.is_available && hexagon.remoteproc_state == "running" {
            Span::styled(
                "Dynamic Zero-Copy Unified LPDDR5",
                Style::default().fg(Color::Cyan),
            )
        } else {
            Span::styled("Unavailable", Style::default().fg(Color::DarkGray))
        };
        lines.push(Line::from(vec![
            Span::styled("  VRAM Allocation: ", Style::default().fg(Color::DarkGray)),
            vram_display,
        ]));
    }

    // 6. Adreno GPU
    let gpu_freq_str = gpu
        .cur_freq_mhz
        .map(|f| format!("{} MHz", f))
        .unwrap_or_else(|| "N/A".to_string());
    lines.push(Line::from(vec![
        Span::styled("  Adreno 623 GPU: ", Style::default().fg(Color::DarkGray)),
        Span::styled(gpu_freq_str, Style::default().fg(Color::White)),
        Span::styled(
            gpu.governor
                .as_deref()
                .map(|g| format!(" [{}]", g))
                .unwrap_or_default(),
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    // 7. Divider
    lines.push(Line::from(Span::styled(
        "─────────────────────────────────────────────────────────",
        Style::default().fg(Color::DarkGray),
    )));

    // 8. The Action Brain
    let link_color = if action_brain.is_available {
        Color::Green
    } else {
        Color::Red
    };
    let sock_color = if action_brain.router_socket_active {
        Color::Green
    } else {
        Color::DarkGray
    };

    lines.push(Line::from(vec![
        Span::styled(
            "◆ THE ACTION BRAIN: ",
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "STM32H5F5 Cortex-M33 @ 250MHz",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::styled(&action_brain.serial_device, Style::default().fg(link_color)),
        Span::styled(
            format!(" ({} baud)", action_brain.baudrate),
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    // 9. Action Brain Status & CAN
    let mut can_spans = vec![
        Span::styled("  RPC router.sock: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            if action_brain.router_socket_active {
                "CONNECTED"
            } else {
                "INACTIVE"
            },
            Style::default().fg(sock_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | CAN Buses: ", Style::default().fg(Color::DarkGray)),
    ];

    if !action_brain.can_interfaces.is_empty() {
        for can in &action_brain.can_interfaces {
            let color = if can.is_up {
                Color::Green
            } else {
                Color::DarkGray
            };
            can_spans.push(Span::styled(
                format!("{}:{} ", can.name, if can.is_up { "UP" } else { "DN" }),
                Style::default().fg(color),
            ));
        }
    } else {
        can_spans.push(Span::styled("None", Style::default().fg(Color::DarkGray)));
    }
    lines.push(Line::from(can_spans));

    let brain_block = Paragraph::new(lines).block(
        Block::default()
            .title(" Dual-Brain Telemetry (AI + Action) ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Magenta)),
    );
    frame.render_widget(brain_block, area);
}

fn render_mid_grid(
    frame: &mut Frame,
    area: Rect,
    sys: &System,
    thermals: &VentunoThermals,
    power_fan: &HardwarePowerFan,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50), // Memory & ZRAM
            Constraint::Percentage(50), // Hardware Power & Consolidated Thermals
        ])
        .split(area);

    // Memory panel
    let total_ram_gb = sys.total_memory() as f64 / 1_073_741_824.0;
    let used_ram_gb = sys.used_memory() as f64 / 1_073_741_824.0;
    let avail_ram_gb = sys.available_memory() as f64 / 1_073_741_824.0;
    let ram_pct = (used_ram_gb / total_ram_gb.max(0.1)) * 100.0;

    let bar_width: usize = 20;
    let filled = ((ram_pct / 100.0) * bar_width as f64).round() as usize;
    let empty = bar_width.saturating_sub(filled);
    let bar = format!("[{}{}]", "#".repeat(filled), "-".repeat(empty));

    let mem_lines = vec![
        Line::from(vec![
            Span::styled(
                "LPDDR5 RAM: ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "{:.2} / {:.2} GB ({:4.1}%) ",
                    used_ram_gb, total_ram_gb, ram_pct
                ),
                Style::default().fg(Color::White),
            ),
            Span::styled(bar, Style::default().fg(Color::Green)),
        ]),
        Line::from(vec![
            Span::styled("Available : ", Style::default().fg(Color::LightGreen)),
            Span::styled(
                format!("{:.2} GB", avail_ram_gb),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" | Swap: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                if sys.total_swap() > 0 {
                    format!(
                        "{:.2} / {:.2} GB",
                        sys.used_swap() as f64 / 1_073_741_824.0,
                        sys.total_swap() as f64 / 1_073_741_824.0
                    )
                } else {
                    "None (16GB Physical)".to_string()
                },
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ];

    let mem_block = Paragraph::new(mem_lines).block(
        Block::default()
            .title(" Memory Subsystem ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Green)),
    );
    frame.render_widget(mem_block, cols[0]);

    // Power & Thermals panel
    let mut power_lines = Vec::new();

    let power_w_str = power_fan
        .board_power_watts
        .map(|w| format!("{:.2} W", w))
        .unwrap_or_else(|| "N/A".to_string());
    let volt_v_str = power_fan
        .board_voltage_v
        .map(|v| format!("{:.2} V", v))
        .unwrap_or_default();
    let fan_rpm_str = power_fan
        .fan_rpm
        .map(|r| format!("{} RPM", r))
        .unwrap_or_else(|| "N/A".to_string());

    power_lines.push(Line::from(vec![
        Span::styled(
            "Board Power: ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            power_w_str,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                " [{} @ {}]",
                volt_v_str,
                power_fan.power_device.as_deref().unwrap_or("INA232")
            ),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            " | Fan: ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            fan_rpm_str,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Thermals: CPU Gold, CPU Silver, GPU, NPU, RAM, Board
    let t_gold = thermals
        .cpu_gold_celsius
        .map(|t| format!("{}°C", t))
        .unwrap_or_else(|| "-".to_string());
    let t_silver = thermals
        .cpu_silver_celsius
        .map(|t| format!("{}°C", t))
        .unwrap_or_else(|| "-".to_string());
    let t_gpu = thermals
        .gpu_celsius
        .map(|t| format!("{}°C", t))
        .unwrap_or_else(|| "-".to_string());
    let t_npu = thermals
        .npu_nsp_celsius
        .map(|t| format!("{}°C", t))
        .unwrap_or_else(|| "-".to_string());
    let t_ram = thermals
        .ram_celsius
        .map(|t| format!("{}°C", t))
        .unwrap_or_else(|| "-".to_string());

    power_lines.push(Line::from(vec![
        Span::styled("Thermals: ", Style::default().fg(Color::LightRed)),
        Span::styled(
            format!("Gold:{} ", t_gold),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            format!("Silv:{} ", t_silver),
            Style::default().fg(Color::White),
        ),
        Span::styled(format!("GPU:{} ", t_gpu), Style::default().fg(Color::White)),
        Span::styled(
            format!("NPU:{} ", t_npu),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("RAM:{}", t_ram), Style::default().fg(Color::White)),
    ]));

    let power_block = Paragraph::new(power_lines).block(
        Block::default()
            .title(" Hardware Power & Thermals ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(power_block, cols[1]);
}

fn render_process_table(
    frame: &mut Frame,
    area: Rect,
    processes: &[ProcessInfo],
    ui_state: &UiState,
) {
    let header_cells = [
        "PID", "Name", "User", "CPU %", "Mem %", "Nice", "Core", "Th", "State",
    ]
    .iter()
    .map(|h| {
        Span::styled(
            *h,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header = Row::new(header_cells).height(1).bottom_margin(0);

    let matching_count = processes
        .iter()
        .filter(|p| {
            if ui_state.process_filter.is_empty() {
                true
            } else {
                p.name
                    .to_lowercase()
                    .contains(&ui_state.process_filter.to_lowercase())
            }
        })
        .count();

    let rows: Vec<Row> = processes
        .iter()
        .filter(|p| {
            if ui_state.process_filter.is_empty() {
                true
            } else {
                p.name
                    .to_lowercase()
                    .contains(&ui_state.process_filter.to_lowercase())
            }
        })
        .skip(ui_state.scroll_offset)
        .map(|p| {
            let is_ai = p.name.contains("llama")
                || p.name.contains("qnn")
                || p.name.contains("fastrpc")
                || p.name.contains("arduino");
            let name_style = if is_ai {
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let cluster_label = crate::hardware::get_core_cluster_label(p.cpu_core);
            let core_label = format!("{} ({})", p.cpu_core, cluster_label);

            Row::new(vec![
                Span::raw(p.pid.to_string()),
                Span::styled(&p.name, name_style),
                Span::raw(&p.user),
                Span::styled(
                    format!("{:5.1}", p.cpu),
                    if p.cpu > 50.0 {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default()
                    },
                ),
                Span::raw(format!("{:5.1}", p.mem)),
                Span::raw(p.nice.to_string()),
                Span::styled(
                    core_label,
                    if crate::hardware::is_big_core(p.cpu_core) {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::raw(p.num_threads.to_string()),
                Span::raw(p.state.to_string()),
            ])
        })
        .collect();

    let title = format_process_table_title(ui_state, matching_count);

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(24),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(12),
            Constraint::Length(5),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::DarkGray)),
    );

    frame.render_widget(table, area);
}

pub fn format_process_table_title(ui_state: &UiState, total_matching: usize) -> String {
    if ui_state.filter_mode {
        format!(" Process List [Filter: {}_] ", ui_state.process_filter)
    } else if !ui_state.process_filter.is_empty() && ui_state.scroll_offset > 0 {
        format!(
            " Process List [Filter: {}] [Offset: +{}/{}] ",
            ui_state.process_filter, ui_state.scroll_offset, total_matching
        )
    } else if !ui_state.process_filter.is_empty() {
        format!(" Process List [Filter: {}] ", ui_state.process_filter)
    } else if ui_state.scroll_offset > 0 {
        format!(
            " Process List [Offset: +{}/{}] ",
            ui_state.scroll_offset, total_matching
        )
    } else {
        " Process List ".to_string()
    }
}

fn render_footer(frame: &mut Frame, area: Rect) {
    let footer_text = Line::from(vec![
        Span::styled(
            " Q / Esc",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Quit | "),
        Span::styled(
            "▲/▼ or j/k",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Scroll | "),
        Span::styled(
            "c/m/p/n",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Sort | "),
        Span::styled(
            "/",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Filter | "),
        Span::styled(
            "Big: 0-3 (A78C) | Little: 4-7 (A55)",
            Style::default().fg(Color::Yellow),
        ),
    ]);

    let footer = Paragraph::new(footer_text);
    frame.render_widget(footer, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_state_defaults() {
        let state = UiState::default();
        assert_eq!(state.process_sort, ProcessSortMode::CpuDesc);
        assert!(state.process_filter.is_empty());
        assert!(!state.filter_mode);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_format_process_table_title() {
        let mut state = UiState::default();
        assert_eq!(format_process_table_title(&state, 20), " Process List ");

        state.scroll_offset = 5;
        assert_eq!(
            format_process_table_title(&state, 20),
            " Process List [Offset: +5/20] "
        );

        state.process_filter = "llama".to_string();
        assert_eq!(
            format_process_table_title(&state, 2),
            " Process List [Filter: llama] [Offset: +5/2] "
        );

        state.scroll_offset = 0;
        assert_eq!(
            format_process_table_title(&state, 2),
            " Process List [Filter: llama] "
        );

        state.filter_mode = true;
        assert_eq!(
            format_process_table_title(&state, 2),
            " Process List [Filter: llama_] "
        );
    }
}
