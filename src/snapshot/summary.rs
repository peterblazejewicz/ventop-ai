// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use std::fmt::Write as _;

use serde::Serialize;

use super::types::{MarkerRecord, NpuPhase, SampleTelemetry, StatSummary, VentopSnapshot};

#[derive(Debug, Clone, Serialize)]
pub struct PhaseSummary {
    pub start_marker: String,
    pub end_marker: String,
    pub duration_secs: f64,
    pub samples_count: usize,
    pub median_power_w: Option<f64>,
    pub npu_active_pct: f64,
    pub median_cpu_big_pct: f32,
    pub median_cpu_little_pct: f32,
}

pub fn calc_f64_stats(mut values: Vec<f64>) -> Option<StatSummary<f64>> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = values.len();
    let min = values[0];
    let max = values[n - 1];
    let median = values[n * 50 / 100];
    let p90 = values[n * 90 / 100];
    Some(StatSummary {
        min,
        median,
        p90,
        max,
    })
}

pub fn calc_f32_stats(mut values: Vec<f32>) -> Option<StatSummary<f32>> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = values.len();
    let min = values[0];
    let max = values[n - 1];
    let median = values[n * 50 / 100];
    let p90 = values[n * 90 / 100];
    Some(StatSummary {
        min,
        median,
        p90,
        max,
    })
}

pub fn calc_u32_stats(mut values: Vec<u32>) -> Option<StatSummary<u32>> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let n = values.len();
    let min = values[0];
    let max = values[n - 1];
    let median = values[n * 50 / 100];
    let p90 = values[n * 90 / 100];
    Some(StatSummary {
        min,
        median,
        p90,
        max,
    })
}

/// Helper to identify end-of-phase markers (e.g., gen-end, req-end, done, stop)
pub fn is_end_marker(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    lower.ends_with("-end")
        || lower.ends_with("_end")
        || lower.ends_with(".end")
        || lower.ends_with(" end")
        || lower == "stop"
        || lower == "done"
        || lower == "finish"
        || lower == "exit"
        || lower == "complete"
}

#[derive(Default)]
pub struct SummaryTracker {
    pub samples: Vec<SampleTelemetry>,
    pub(crate) power_watts: Vec<f64>,
    pub(crate) fan_rpms: Vec<u32>,
    pub(crate) cpu_total: Vec<f32>,
    pub(crate) cpu_big: Vec<f32>,
    pub(crate) cpu_little: Vec<f32>,
    pub(crate) ram_available_gb: Vec<f64>,
    pub(crate) temp_gold: Vec<f32>,
    pub(crate) temp_silver: Vec<f32>,
    pub(crate) temp_gpu: Vec<f32>,
    pub(crate) temp_npu: Vec<f32>,
    pub(crate) temp_ram: Vec<f32>,
    pub(crate) npu_active_flags: Vec<bool>,
    pub markers: Vec<MarkerRecord>,
    pub total_samples: usize,
    pub npu_active_samples: usize,
    pub npu_active_and_tracked_busy_samples: usize,
}

impl SummaryTracker {
    pub fn record_sample(&mut self, s: &VentopSnapshot, active_threshold_pct: f32) {
        let sample_idx = self.total_samples;
        self.total_samples += 1;

        let interval_secs = s.interval_ms.unwrap_or(200.0) / 1000.0;
        let phase = NpuPhase::from_activity_status(s.hexagon_npu.activity_status);
        let is_heuristic = s.hexagon_npu.activity_source != "engine_slots";

        let mut prompt_tokens_processed = 0u32;
        let mut tokens_decoded = 0u32;
        let mut completed_task_tokens = None;
        for p in &s.hexagon_npu.active_processes {
            if let Some(ref em) = p.engine_metrics {
                prompt_tokens_processed = prompt_tokens_processed.max(em.prompt_tokens_processed);
                tokens_decoded = tokens_decoded.max(em.tokens_decoded);
                if let Some(c) = em.completed_task_tokens {
                    completed_task_tokens = Some(completed_task_tokens.unwrap_or(0).max(c));
                }
            }
        }

        self.samples.push(SampleTelemetry {
            interval_secs,
            power_w: s.power_fan.board_power_watts,
            phase,
            is_heuristic,
            prompt_tokens_processed,
            tokens_decoded,
            completed_task_tokens,
            cpu_big: s.cpu.big_cluster_load_pct,
            cpu_little: s.cpu.little_cluster_load_pct,
        });

        if let Some(ref m) = s.marker {
            self.markers.push(MarkerRecord {
                label: m.clone(),
                sample_idx,
            });
        }

        if let Some(w) = s.power_fan.board_power_watts {
            self.power_watts.push(w);
        }
        if let Some(rpm) = s.power_fan.fan_rpm {
            self.fan_rpms.push(rpm);
        }
        self.cpu_total.push(s.cpu.total_load_pct);
        self.cpu_big.push(s.cpu.big_cluster_load_pct);
        self.cpu_little.push(s.cpu.little_cluster_load_pct);
        self.ram_available_gb
            .push(s.memory.available_bytes as f64 / 1_073_741_824.0);

        if let Some(t) = s.thermals.cpu_gold_celsius {
            self.temp_gold.push(t as f32);
        }
        if let Some(t) = s.thermals.cpu_silver_celsius {
            self.temp_silver.push(t as f32);
        }
        if let Some(t) = s.thermals.gpu_celsius {
            self.temp_gpu.push(t as f32);
        }
        if let Some(t) = s.thermals.npu_nsp_celsius {
            self.temp_npu.push(t as f32);
        }
        if let Some(t) = s.thermals.ram_celsius {
            self.temp_ram.push(t as f32);
        }

        let npu_active = !s.hexagon_npu.activity_status.starts_with("Idle")
            && !s.hexagon_npu.activity_status.starts_with("Offline");
        self.npu_active_flags.push(npu_active);
        if npu_active {
            self.npu_active_samples += 1;

            let tracked_busy = s
                .tracked_processes
                .as_ref()
                .map(|procs| procs.iter().any(|p| p.cpu_pct >= active_threshold_pct))
                .unwrap_or(false);

            if tracked_busy {
                self.npu_active_and_tracked_busy_samples += 1;
            }
        }
    }

    pub fn format_report(&self, active_threshold_pct: f32, elapsed_secs: f64) -> String {
        if self.total_samples == 0 {
            return "No telemetry samples collected for summary.\n".to_string();
        }

        let mut out = String::new();
        let npu_share = (self.npu_active_samples as f64 / self.total_samples as f64) * 100.0;
        let coactive_share =
            (self.npu_active_and_tracked_busy_samples as f64 / self.total_samples as f64) * 100.0;

        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "================================================================================"
        );
        let _ = writeln!(out, "                       VENTOP-AI RUN SUMMARY REPORT");
        let _ = writeln!(
            out,
            "================================================================================"
        );
        let _ = writeln!(
            out,
            "Duration / Samples : {} samples (~{:.1}s)",
            self.total_samples, elapsed_secs
        );
        let _ = writeln!(
            out,
            "Active NPU Share   : {:.1}% ({}/{} samples)",
            npu_share, self.npu_active_samples, self.total_samples
        );
        let _ = writeln!(
            out,
            "Co-Activity Share  : {:.1}% (NPU active while tracked proc >= {:.1}% CPU)",
            coactive_share, active_threshold_pct
        );
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{:<28} {:>10} {:>10} {:>10} {:>10}",
            "METRIC", "MIN", "MEDIAN", "P90", "MAX"
        );
        let _ = writeln!(
            out,
            "────────────────────────────────────────────────────────────────────────────────"
        );

        let print_f64_row = |out: &mut String, name: &str, data: &[f64], unit: &str| {
            if let Some(st) = calc_f64_stats(data.to_vec()) {
                let label = if unit.is_empty() {
                    name.to_string()
                } else {
                    format!("{} ({})", name, unit)
                };
                let _ = writeln!(
                    out,
                    "{:<28} {:>10.2} {:>10.2} {:>10.2} {:>10.2}",
                    label, st.min, st.median, st.p90, st.max
                );
            }
        };

        let print_f32_row = |out: &mut String, name: &str, data: &[f32], unit: &str| {
            if let Some(st) = calc_f32_stats(data.to_vec()) {
                let label = if unit.is_empty() {
                    name.to_string()
                } else {
                    format!("{} ({})", name, unit)
                };
                let _ = writeln!(
                    out,
                    "{:<28} {:>10.1} {:>10.1} {:>10.1} {:>10.1}",
                    label, st.min, st.median, st.p90, st.max
                );
            }
        };

        let print_u32_row = |out: &mut String, name: &str, data: &[u32], unit: &str| {
            if let Some(st) = calc_u32_stats(data.to_vec()) {
                let label = if unit.is_empty() {
                    name.to_string()
                } else {
                    format!("{} ({})", name, unit)
                };
                let _ = writeln!(
                    out,
                    "{:<28} {:>10} {:>10} {:>10} {:>10}",
                    label, st.min, st.median, st.p90, st.max
                );
            }
        };

        print_f64_row(&mut out, "Board Power", &self.power_watts, "W");
        print_u32_row(&mut out, "Fan Speed", &self.fan_rpms, "RPM");
        print_f32_row(&mut out, "CPU Total Load", &self.cpu_total, "%");
        print_f32_row(&mut out, "  Big Cluster (A78C)", &self.cpu_big, "%");
        print_f32_row(&mut out, "  Little Cluster (A55)", &self.cpu_little, "%");
        print_f64_row(&mut out, "RAM Available", &self.ram_available_gb, "GB");
        let _ = writeln!(out, "Thermals:");
        print_f32_row(&mut out, "  CPU Gold", &self.temp_gold, "°C");
        print_f32_row(&mut out, "  CPU Silver", &self.temp_silver, "°C");
        print_f32_row(&mut out, "  GPU Adreno", &self.temp_gpu, "°C");
        print_f32_row(&mut out, "  NPU Hexagon", &self.temp_npu, "°C");
        print_f32_row(&mut out, "  RAM LPDDR5", &self.temp_ram, "°C");

        if !self.markers.is_empty() {
            let _ = writeln!(out);
            let _ = writeln!(out, "MARKER PHASES & INTERVAL PROFILING:");
            let _ = writeln!(
                out,
                "────────────────────────────────────────────────────────────────────────────────"
            );

            let mut cur_start = self.markers[0].sample_idx;

            for i in 0..self.markers.len() {
                let m_curr = &self.markers[i];
                let (next_label, next_idx, is_next_end) = if i + 1 < self.markers.len() {
                    let m_next = &self.markers[i + 1];
                    (
                        m_next.label.as_str(),
                        m_next.sample_idx,
                        is_end_marker(&m_next.label),
                    )
                } else {
                    ("STREAM_END", self.samples.len(), false)
                };

                let start_idx = cur_start.min(self.samples.len());
                let end_idx = if is_next_end
                    || (next_idx < self.samples.len()
                        && self.samples[next_idx].phase == NpuPhase::TokenGeneration
                        && (next_idx + 1 >= self.samples.len()
                            || self.samples[next_idx + 1].phase != NpuPhase::TokenGeneration))
                {
                    (next_idx + 1).min(self.samples.len())
                } else {
                    next_idx.min(self.samples.len())
                };

                cur_start = end_idx;

                let slice = if start_idx < self.samples.len() && end_idx > start_idx {
                    &self.samples[start_idx..end_idx]
                } else {
                    &[]
                };

                if slice.is_empty() {
                    continue;
                }

                let active_count = slice.iter().filter(|s| s.phase != NpuPhase::Idle).count();
                // If trailing interval after an end-marker is 100% idle, omit it to keep report concise
                if next_label == "STREAM_END" && is_end_marker(&m_curr.label) && active_count == 0 {
                    continue;
                }

                let window_duration: f64 = slice.iter().map(|s| s.interval_secs).sum();
                let slice_len = slice.len();
                let tick_res = slice.first().map(|s| s.interval_secs).unwrap_or(0.20);

                // Compute median power in slice
                let slice_power: Vec<f64> = slice.iter().filter_map(|s| s.power_w).collect();
                let med_power = calc_f64_stats(slice_power).map(|st| st.median);

                let npu_pct = if slice_len > 0 {
                    (active_count as f64 / slice_len as f64) * 100.0
                } else {
                    0.0
                };

                // Compute CPU Big & Little median
                let slice_big: Vec<f32> = slice.iter().map(|s| s.cpu_big).collect();
                let med_big = calc_f32_stats(slice_big).map(|st| st.median).unwrap_or(0.0);

                let slice_little: Vec<f32> = slice.iter().map(|s| s.cpu_little).collect();
                let med_little = calc_f32_stats(slice_little)
                    .map(|st| st.median)
                    .unwrap_or(0.0);

                let power_str = med_power
                    .map(|p| format!("{:.2} W", p))
                    .unwrap_or_else(|| "N/A".to_string());

                let _ = writeln!(
                    out,
                    "  \"{}\" → \"{}\" ({:.1}s, {} samples):",
                    m_curr.label, next_label, window_duration, slice_len
                );
                let _ = writeln!(out, "    Overall : Power: median {} | NPU Active: {:5.1}% | CPU: Big {:4.1}%, Little {:4.1}%",
                          power_str, npu_pct, med_big, med_little);

                // Partition slice into Prompt, Decode, and Idle sub-phases
                let prompt_samples: Vec<_> = slice
                    .iter()
                    .filter(|s| s.phase == NpuPhase::PromptProcessing)
                    .collect();
                let decode_samples: Vec<_> = slice
                    .iter()
                    .filter(|s| s.phase == NpuPhase::TokenGeneration)
                    .collect();
                let idle_samples: Vec<_> =
                    slice.iter().filter(|s| s.phase == NpuPhase::Idle).collect();

                let mut sub_lines: Vec<String> = Vec::new();

                // 1. Prompt Sub-Phase
                if !prompt_samples.is_empty() {
                    let dur: f64 = prompt_samples.iter().map(|s| s.interval_secs).sum();
                    let dur_pct = if window_duration > 0.0 {
                        (dur / window_duration) * 100.0
                    } else {
                        0.0
                    };
                    let pwr_vec: Vec<f64> =
                        prompt_samples.iter().filter_map(|s| s.power_w).collect();
                    let med_p = calc_f64_stats(pwr_vec)
                        .map(|st| format!("{:.2} W", st.median))
                        .unwrap_or_else(|| "N/A".to_string());

                    // TTFT: duration of prompt phase (+ any preceding initial idle)
                    let initial_idle: f64 = slice
                        .iter()
                        .take_while(|s| s.phase == NpuPhase::Idle)
                        .map(|s| s.interval_secs)
                        .sum();
                    let ttft = dur + initial_idle;

                    // Prompt tokens: in llama-server, n_prompt_tokens_processed latches at full prompt count during and after prefill.
                    // Search across all active samples (prompt + decode) to get the true completed prompt count.
                    let p_tokens = slice
                        .iter()
                        .filter(|s| {
                            s.phase == NpuPhase::PromptProcessing
                                || s.phase == NpuPhase::TokenGeneration
                        })
                        .map(|s| s.prompt_tokens_processed)
                        .max()
                        .unwrap_or(0);
                    let is_heur = prompt_samples.iter().all(|s| s.is_heuristic);

                    let detail = if is_heur {
                        "[hw heuristics]".to_string()
                    } else if p_tokens > 0 {
                        // Suppress noisy rate for short phases (< 5 samples)
                        if prompt_samples.len() >= 5 && dur > 0.0 {
                            let tps = p_tokens as f64 / dur;
                            format!(
                                "{} tok ({:.1} TPS) | TTFT: {:.2}s (±{:.2})",
                                p_tokens, tps, ttft, tick_res
                            )
                        } else {
                            format!("{} tok | TTFT: {:.2}s (±{:.2})", p_tokens, ttft, tick_res)
                        }
                    } else {
                        format!("TTFT: {:.2}s (±{:.2})", ttft, tick_res)
                    };

                    sub_lines.push(format!(
                        "Prompt: {:4.2}s ({:2.0}%) | {:>7} | {}",
                        dur, dur_pct, med_p, detail
                    ));
                }

                // 2. Decode Sub-Phase
                if !decode_samples.is_empty() {
                    let dur: f64 = decode_samples.iter().map(|s| s.interval_secs).sum();
                    let dur_pct = if window_duration > 0.0 {
                        (dur / window_duration) * 100.0
                    } else {
                        0.0
                    };
                    let pwr_vec: Vec<f64> =
                        decode_samples.iter().filter_map(|s| s.power_w).collect();
                    let med_p = calc_f64_stats(pwr_vec)
                        .map(|st| format!("{:.2} W", st.median))
                        .unwrap_or_else(|| "N/A".to_string());

                    let max_d = decode_samples
                        .iter()
                        .map(|s| s.tokens_decoded)
                        .max()
                        .unwrap_or(0);
                    let min_d = decode_samples
                        .iter()
                        .map(|s| s.tokens_decoded)
                        .min()
                        .unwrap_or(0);

                    // Check if any slot completed a task during or at the immediate boundary of this slice
                    let completed_in_slice = slice
                        .iter()
                        .filter_map(|s| s.completed_task_tokens)
                        .max()
                        .unwrap_or(0);

                    // If generation began within this slice (e.g. prompt is present, or slice started from idle, or min_d <= 10),
                    // baseline decoded tokens was 0, so delta is simply max_d without boundary subtraction!
                    let started_in_slice = !prompt_samples.is_empty()
                        || start_idx == 0
                        || (start_idx > 0 && self.samples[start_idx - 1].phase == NpuPhase::Idle)
                        || min_d <= 10;

                    let raw_delta = if started_in_slice {
                        max_d
                    } else {
                        max_d.saturating_sub(min_d)
                    };

                    let delta_d = raw_delta.max(completed_in_slice);

                    let is_heur = decode_samples.iter().all(|s| s.is_heuristic);

                    let detail = if is_heur {
                        "[hw heuristics]".to_string()
                    } else if delta_d > 0 {
                        // Suppress rate for short decode (< 5 samples)
                        if decode_samples.len() >= 5 && dur > 0.0 {
                            let tps = delta_d as f64 / dur;
                            format!("{} tok ({:.1} TPS)", delta_d, tps)
                        } else {
                            format!("{} tok", delta_d)
                        }
                    } else {
                        "".to_string()
                    };

                    let line = if detail.is_empty() {
                        format!("Decode: {:4.2}s ({:2.0}%) | {:>7}", dur, dur_pct, med_p)
                    } else {
                        format!(
                            "Decode: {:4.2}s ({:2.0}%) | {:>7} | {}",
                            dur, dur_pct, med_p, detail
                        )
                    };
                    sub_lines.push(line);
                }

                // 3. Idle / Overhead Sub-Phase (only report if >= 0.05s)
                let idle_dur: f64 = idle_samples.iter().map(|s| s.interval_secs).sum();
                if idle_dur >= 0.05 && (!prompt_samples.is_empty() || !decode_samples.is_empty()) {
                    let dur_pct = if window_duration > 0.0 {
                        (idle_dur / window_duration) * 100.0
                    } else {
                        0.0
                    };
                    let pwr_vec: Vec<f64> = idle_samples.iter().filter_map(|s| s.power_w).collect();
                    let med_p = calc_f64_stats(pwr_vec)
                        .map(|st| format!("{:.2} W", st.median))
                        .unwrap_or_else(|| "N/A".to_string());
                    sub_lines.push(format!(
                        "Idle  : {:4.2}s ({:2.0}%) | {:>7}",
                        idle_dur, dur_pct, med_p
                    ));
                }

                // Print tree-formatted sub-lines
                for (idx, line) in sub_lines.iter().enumerate() {
                    let is_last = idx + 1 == sub_lines.len();
                    let prefix = if is_last {
                        "    └─ "
                    } else {
                        "    ├─ "
                    };
                    let _ = writeln!(out, "{}{}", prefix, line);
                }
            }
        }

        let _ = writeln!(
            out,
            "================================================================================"
        );
        out
    }

    pub fn print_report(&self, active_threshold_pct: f32, elapsed_secs: f64) {
        eprint!("{}", self.format_report(active_threshold_pct, elapsed_secs));
    }
}

/// Print summary report directly from a SummaryTracker
pub fn print_summary(tracker: &SummaryTracker, active_threshold_pct: f32, elapsed_secs: f64) {
    tracker.print_report(active_threshold_pct, elapsed_secs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::stream::collect_snapshot_continuous;
    use crate::snapshot::types::NpuPhase;
    use sysinfo::System;

    #[test]
    fn test_stat_calculations() {
        let values = vec![1.0, 5.0, 10.0, 20.0, 50.0];
        let stats = calc_f64_stats(values).unwrap();
        assert_eq!(stats.min, 1.0);
        assert_eq!(stats.max, 50.0);
        assert_eq!(stats.median, 10.0);
        assert_eq!(stats.p90, 50.0);
    }

    #[test]
    fn test_calc_u32_stats() {
        assert!(calc_u32_stats(vec![]).is_none());

        let single = calc_u32_stats(vec![2818]).unwrap();
        assert_eq!(single.min, 2818);
        assert_eq!(single.median, 2818);
        assert_eq!(single.p90, 2818);
        assert_eq!(single.max, 2818);

        let multi = calc_u32_stats(vec![3300, 2818, 3250, 3000, 3150, 2900, 3200, 3100, 3220, 3150])
            .unwrap();
        // sorted (10 elements): [2818, 2900, 3000, 3100, 3150, 3150, 3200, 3220, 3250, 3300]
        // n = 10 -> median = idx 5 (3150), p90 = idx 9 (3300)
        assert_eq!(multi.min, 2818);
        assert_eq!(multi.median, 3150);
        assert_eq!(multi.p90, 3300);
        assert_eq!(multi.max, 3300);
    }

    #[test]
    fn test_summary_tracker_fan_speed_report_and_omission() {
        let sys = System::new();

        // 1. Tracker with fan_rpm populated across samples (and one None sample)
        let mut tracker = SummaryTracker::default();
        let rpms = [
            Some(3250),
            Some(2818),
            None,
            Some(3150),
            Some(3300),
            Some(2950),
            Some(3100),
            Some(3150),
            Some(3200),
            Some(3050),
            Some(3250),
        ];
        // 10 valid RPMs sorted: [2818, 2950, 3050, 3100, 3150, 3150, 3200, 3250, 3250, 3300]
        // min = 2818, median (idx 5) = 3150, p90 (idx 9) = 3300, max = 3300
        for rpm in rpms {
            let mut s = collect_snapshot_continuous(&sys, &[], false);
            s.power_fan.board_power_watts = Some(14.50);
            s.power_fan.fan_rpm = rpm;
            tracker.record_sample(&s, 50.0);
        }

        assert_eq!(tracker.fan_rpms.len(), 10);
        let report = tracker.format_report(50.0, 2.2);
        let expected_fan_row = format!(
            "{:<28} {:>10} {:>10} {:>10} {:>10}",
            "Fan Speed (RPM)", 2818, 3150, 3300, 3300
        );
        assert!(
            report.contains(&expected_fan_row),
            "Expected report to contain exact Fan Speed row:\n{}\nActual report:\n{}",
            expected_fan_row,
            report
        );

        // Verify Fan Speed (RPM) appears immediately after Board Power (W)
        let power_pos = report.find("Board Power (W)").expect("Board Power row missing");
        let fan_pos = report.find("Fan Speed (RPM)").expect("Fan Speed row missing");
        let cpu_pos = report.find("CPU Total Load (%)").expect("CPU Total Load row missing");
        assert!(power_pos < fan_pos && fan_pos < cpu_pos);

        // 2. Tracker where all samples have fan_rpm: None -> Fan Speed (RPM) row must be omitted
        let mut tracker_no_fan = SummaryTracker::default();
        for _ in 0..3 {
            let mut s = collect_snapshot_continuous(&sys, &[], false);
            s.power_fan.board_power_watts = Some(12.00);
            s.power_fan.fan_rpm = None;
            tracker_no_fan.record_sample(&s, 50.0);
        }
        assert!(tracker_no_fan.fan_rpms.is_empty());
        let report_no_fan = tracker_no_fan.format_report(50.0, 0.6);
        assert!(
            !report_no_fan.contains("Fan Speed (RPM)"),
            "Expected Fan Speed (RPM) row to be omitted when fan_rpms is empty, got:\n{}",
            report_no_fan
        );
    }

    #[test]
    fn test_is_end_marker() {
        assert!(is_end_marker("gen-end"));
        assert!(is_end_marker("req_end"));
        assert!(is_end_marker("turn.end"));
        assert!(is_end_marker("benchmark end"));
        assert!(is_end_marker("stop"));
        assert!(is_end_marker("done"));
        assert!(is_end_marker("exit"));
        assert!(is_end_marker("finish"));
        assert!(is_end_marker("complete"));

        assert!(!is_end_marker("gen-start"));
        assert!(!is_end_marker("req_start"));
        assert!(!is_end_marker("phase1"));
        assert!(!is_end_marker("prefill"));
    }

    #[test]
    fn test_summary_marker_phase_intervals() {
        let sys = System::new();
        let mut tracker = SummaryTracker::default();

        let mut s1 = collect_snapshot_continuous(&sys, &[], false);
        s1.marker = Some("prompt-start".to_string());
        s1.hexagon_npu.activity_status = "Active (Prompt Processing)";
        s1.hexagon_npu.activity_source = "engine_slots";
        s1.power_fan.board_power_watts = Some(19.8);
        s1.interval_ms = Some(200.0);
        tracker.record_sample(&s1, 50.0);

        let mut s2 = collect_snapshot_continuous(&sys, &[], false);
        s2.marker = None;
        s2.hexagon_npu.activity_status = "Active (Token Generation)";
        s2.hexagon_npu.activity_source = "engine_slots";
        s2.power_fan.board_power_watts = Some(18.2);
        s2.interval_ms = Some(200.0);
        tracker.record_sample(&s2, 50.0);

        let mut s3 = collect_snapshot_continuous(&sys, &[], false);
        s3.marker = Some("decode-start".to_string());
        s3.hexagon_npu.activity_status = "Active (Token Generation)";
        s3.hexagon_npu.activity_source = "engine_slots";
        s3.power_fan.board_power_watts = Some(17.9);
        s3.interval_ms = Some(200.0);
        tracker.record_sample(&s3, 50.0);

        assert_eq!(tracker.markers.len(), 2);
        assert_eq!(tracker.markers[0].label, "prompt-start");
        assert_eq!(tracker.markers[0].sample_idx, 0);
        assert_eq!(tracker.markers[1].label, "decode-start");
        assert_eq!(tracker.markers[1].sample_idx, 2);

        assert_eq!(tracker.samples.len(), 3);
        assert_eq!(tracker.samples[0].phase, NpuPhase::PromptProcessing);
        assert_eq!(tracker.samples[1].phase, NpuPhase::TokenGeneration);
        assert_eq!(tracker.samples[2].phase, NpuPhase::TokenGeneration);

        // Verify print_report prints without panic
        tracker.print_report(50.0, 0.6);
    }
}
