// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Evaluates NPU inference activity status and inference power delta based on multi-signal hardware inputs
pub fn evaluate_inference_activity(
    max_polling_cpu: f32,
    raw_power_delta: f64,
    peak_temp: i32,
    glink_interrupts_per_sec: u64,
    has_active_processes: bool,
) -> (&'static str, f64) {
    let power_above_idle_w = (raw_power_delta.max(0.0) * 10.0).round() / 10.0;

    if !has_active_processes {
        return ("Idle", power_above_idle_w);
    }

    let is_inferring = (max_polling_cpu >= 20.0)
        || (raw_power_delta >= 4.0)
        || (raw_power_delta >= 2.5 && peak_temp >= 40);

    let activity_status = if is_inferring {
        if raw_power_delta >= 8.2 {
            "Active (Prompt Processing)"
        } else {
            "Active (Token Generation)"
        }
    } else if glink_interrupts_per_sec > 50 {
        "Active (FastRPC RPC)"
    } else {
        "Idle (Model Loaded)"
    };

    (activity_status, power_above_idle_w)
}

/// Engine-level metrics directly queried from llama-server /slots
#[derive(Debug, Clone, serde::Serialize)]
pub struct LlamaEngineMetrics {
    pub slot_state: String, // "idle", "processing_prompt", "generating_tokens"
    pub prompt_tokens: u32,
    pub prompt_tokens_processed: u32,
    pub prompt_tokens_cached: u32,
    pub tokens_decoded: u32,
    pub prompt_tokens_per_sec: f32,
    pub decode_tokens_per_sec: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_task_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latched_decode_tps: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latched_prompt_tps: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds_since_active: Option<u64>,
}

/// Type alias for backward compatibility and slot telemetry naming
pub type LlamaSlotsMetrics = LlamaEngineMetrics;

#[derive(Debug, Clone)]
struct SlotTaskState {
    task_id: u64,
    was_processing: bool,
    initial_prompt_tokens: u32,
    last_decoded: u32,
}

#[derive(Debug, Clone)]
struct CachedSlotQuery {
    last_query_time: Instant,
    last_prompt_tokens: u32,
    last_decoded_tokens: u32,
    smoothed_prompt_tps: f32,
    smoothed_decode_tps: f32,
    last_metrics: Option<LlamaEngineMetrics>,
    last_success_time: Instant,
    slot_tasks: HashMap<u32, SlotTaskState>,
    latched_decode_tps: Option<f32>,
    latched_prompt_tps: Option<f32>,
    last_active_time: Option<Instant>,
}

static SLOT_CACHE: Mutex<Option<HashMap<u16, CachedSlotQuery>>> = Mutex::new(None);

/// Query llama-server /slots endpoint with non-blocking timeout and multi-slot aggregation
pub fn query_llama_slots(port: u16) -> Option<LlamaEngineMetrics> {
    let now = Instant::now();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    // Try a fresh HTTP query first
    if let Some(metrics) = do_http_query_slots(port, addr, now) {
        return Some(metrics);
    }

    // If query timed out or connection failed, check cache for sticky reuse (up to 1500ms)
    if let Ok(cache_guard) = SLOT_CACHE.lock() {
        if let Some(ref cache) = *cache_guard {
            if let Some(entry) = cache.get(&port) {
                let age = now.duration_since(entry.last_success_time);
                let age_ms = age.as_millis() as u64;
                if age_ms <= 1500 {
                    if let Some(ref prev) = entry.last_metrics {
                        let mut reused = prev.clone();
                        reused.age_ms = Some(age_ms);
                        return Some(reused);
                    }
                }
            }
        }
    }

    None
}

/// Pure helper to parse /slots JSON payload into slot state and token counters
pub fn parse_slots_json_payload(slots_json: &serde_json::Value) -> (String, u32, u32, u32, u32) {
    let mut any_processing = false;
    let mut any_prompt_phase = false;

    let mut total_prompt_tokens = 0u32;
    let mut total_prompt_processed = 0u32;
    let mut total_prompt_cached = 0u32;
    let mut total_tokens_decoded = 0u32;

    if let Some(slots_array) = slots_json.as_array() {
        for slot in slots_array {
            let is_processing = slot
                .get("is_processing")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let prompt_tokens = slot
                .get("n_prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let prompt_processed = slot
                .get("n_prompt_tokens_processed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let prompt_cached = slot
                .get("n_prompt_tokens_cache")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;

            total_prompt_tokens += prompt_tokens;
            total_prompt_processed += prompt_processed;
            total_prompt_cached += prompt_cached;

            let tokens_decoded = slot
                .get("next_token")
                .and_then(|v| {
                    if let Some(arr) = v.as_array() {
                        arr.first()
                            .and_then(|nt| nt.get("n_decoded"))
                            .and_then(|d| d.as_u64())
                    } else if let Some(obj) = v.as_object() {
                        obj.get("n_decoded").and_then(|d| d.as_u64())
                    } else {
                        None
                    }
                })
                .or_else(|| slot.get("n_decoded").and_then(|d| d.as_u64()))
                .unwrap_or(0) as u32;

            total_tokens_decoded += tokens_decoded;

            if is_processing {
                any_processing = true;
                // Strict measured rule from llama-server /slots telemetry:
                // - is_processing == true and next_token[0].n_decoded == 0 -> Prompt Processing
                // - is_processing == true and next_token[0].n_decoded > 0 -> Token Generation
                // - not is_processing -> Idle
                // NOTE: Do NOT use n_prompt_tokens (it grows during generation while n_prompt_tokens_processed stays constant).
                if tokens_decoded == 0 {
                    any_prompt_phase = true;
                }
            }
        }
    }

    let slot_state = if !any_processing {
        "idle".to_string()
    } else if any_prompt_phase {
        "processing_prompt".to_string()
    } else {
        "generating_tokens".to_string()
    };

    (
        slot_state,
        total_prompt_tokens,
        total_prompt_processed,
        total_prompt_cached,
        total_tokens_decoded,
    )
}

fn do_http_query_slots(port: u16, addr: SocketAddr, now: Instant) -> Option<LlamaEngineMetrics> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(15)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(25)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(15)))
        .ok()?;

    let request = format!(
        "GET /slots HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        port
    );
    stream.write_all(request.as_bytes()).ok()?;

    let mut response = Vec::with_capacity(8192);
    let mut buffer = [0u8; 8192];
    let mut content_length: Option<usize> = None;
    let mut body_start: Option<usize> = None;

    while let Ok(n) = stream.read(&mut buffer) {
        if n == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..n]);

        if body_start.is_none() {
            if let Some(pos) = response.windows(4).position(|w| w == b"\r\n\r\n") {
                body_start = Some(pos + 4);
                let header_str = String::from_utf8_lossy(&response[..pos]);
                for line in header_str.lines() {
                    if let Some(val) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        content_length = val.trim().parse::<usize>().ok();
                    }
                }
            }
        }

        if let (Some(start), Some(len)) = (body_start, content_length) {
            if response.len() >= start + len {
                break;
            }
        }

        if response.len() > 65536 {
            break;
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    let body_start = response_str.find("\r\n\r\n")? + 4;
    let body = &response_str[body_start..];
    let slots_json: serde_json::Value = serde_json::from_str(body).ok()?;

    let (
        slot_state,
        total_prompt_tokens,
        total_prompt_processed,
        total_prompt_cached,
        total_tokens_decoded,
    ) = parse_slots_json_payload(&slots_json);

    let mut completed_task_tokens: Option<u32> = None;

    if let Ok(mut cache_guard) = SLOT_CACHE.lock() {
        if cache_guard.is_none() {
            *cache_guard = Some(HashMap::new());
        }
        let cache = cache_guard.as_mut().unwrap();

        let entry = cache.entry(port).or_insert_with(|| CachedSlotQuery {
            last_query_time: now,
            last_prompt_tokens: total_prompt_processed,
            last_decoded_tokens: total_tokens_decoded,
            smoothed_prompt_tps: 0.0,
            smoothed_decode_tps: 0.0,
            last_metrics: None,
            last_success_time: now,
            slot_tasks: HashMap::new(),
            latched_decode_tps: None,
            latched_prompt_tps: None,
            last_active_time: None,
        });

        let elapsed = now.duration_since(entry.last_query_time).as_secs_f32();
        if elapsed > 0.05 {
            let delta_prompt = total_prompt_processed.saturating_sub(entry.last_prompt_tokens);
            let delta_decoded = total_tokens_decoded.saturating_sub(entry.last_decoded_tokens);

            let inst_prompt_tps = delta_prompt as f32 / elapsed;
            let inst_decode_tps = delta_decoded as f32 / elapsed;

            // EMA smoothing
            entry.smoothed_prompt_tps = entry.smoothed_prompt_tps * 0.7 + inst_prompt_tps * 0.3;
            entry.smoothed_decode_tps = entry.smoothed_decode_tps * 0.7 + inst_decode_tps * 0.3;

            entry.last_prompt_tokens = total_prompt_processed;
            entry.last_decoded_tokens = total_tokens_decoded;
            entry.last_query_time = now;

            if inst_decode_tps > 0.0 {
                let rounded_d = (inst_decode_tps * 10.0).round() / 10.0;
                entry.latched_decode_tps = Some(rounded_d);
            }
            if inst_prompt_tps > 0.0 {
                let rounded_p = (inst_prompt_tps * 10.0).round() / 10.0;
                entry.latched_prompt_tps = Some(rounded_p);
            }
        }
        let prompt_tps = (entry.smoothed_prompt_tps * 10.0).round() / 10.0;
        let decode_tps = (entry.smoothed_decode_tps * 10.0).round() / 10.0;

        let is_any_active = slot_state != "idle";
        if is_any_active {
            entry.last_active_time = Some(now);
            if decode_tps > 0.0 {
                entry.latched_decode_tps = Some(decode_tps);
            }
            if prompt_tps > 0.0 {
                entry.latched_prompt_tps = Some(prompt_tps);
            }
        }

        // Track per-slot task transitions to capture exact terminal completion tokens
        if let Some(slots_array) = slots_json.as_array() {
            for slot in slots_array {
                let slot_id = slot.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let task_id = slot.get("id_task").and_then(|v| v.as_u64()).unwrap_or(0);
                let is_proc = slot
                    .get("is_processing")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let p_tokens = slot
                    .get("n_prompt_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let p_proc = slot
                    .get("n_prompt_tokens_processed")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let p_cached = slot
                    .get("n_prompt_tokens_cache")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let decoded = slot
                    .get("next_token")
                    .and_then(|v| {
                        if let Some(arr) = v.as_array() {
                            arr.first()
                                .and_then(|nt| nt.get("n_decoded"))
                                .and_then(|d| d.as_u64())
                        } else if let Some(obj) = v.as_object() {
                            obj.get("n_decoded").and_then(|d| d.as_u64())
                        } else {
                            None
                        }
                    })
                    .or_else(|| slot.get("n_decoded").and_then(|d| d.as_u64()))
                    .unwrap_or(0) as u32;

                let initial_prompt = p_proc + p_cached;

                let task_state = entry.slot_tasks.entry(slot_id).or_insert(SlotTaskState {
                    task_id,
                    was_processing: is_proc,
                    initial_prompt_tokens: initial_prompt,
                    last_decoded: decoded,
                });

                if is_proc {
                    if task_state.task_id != task_id || !task_state.was_processing {
                        task_state.task_id = task_id;
                        task_state.initial_prompt_tokens = initial_prompt;
                    } else if initial_prompt > task_state.initial_prompt_tokens {
                        task_state.initial_prompt_tokens = initial_prompt;
                    }
                    task_state.last_decoded = decoded;
                    task_state.was_processing = true;
                } else {
                    // Slot transitioned from processing -> idle
                    if task_state.was_processing && task_state.task_id == task_id {
                        let final_decoded =
                            p_tokens.saturating_sub(task_state.initial_prompt_tokens);
                        let final_tokens = task_state.last_decoded.max(final_decoded);
                        if final_tokens > 0 {
                            completed_task_tokens =
                                Some(completed_task_tokens.unwrap_or(0).max(final_tokens));
                        }
                        task_state.was_processing = false;
                    }
                }
            }
        }

        let seconds_since_active = entry
            .last_active_time
            .map(|t| now.duration_since(t).as_secs());

        let metrics = LlamaEngineMetrics {
            slot_state,
            prompt_tokens: total_prompt_tokens,
            prompt_tokens_processed: total_prompt_processed,
            prompt_tokens_cached: total_prompt_cached,
            tokens_decoded: total_tokens_decoded,
            prompt_tokens_per_sec: prompt_tps,
            decode_tokens_per_sec: decode_tps,
            age_ms: None,
            completed_task_tokens,
            latched_decode_tps: entry.latched_decode_tps,
            latched_prompt_tps: entry.latched_prompt_tps,
            seconds_since_active,
        };

        entry.last_metrics = Some(metrics.clone());
        entry.last_success_time = now;

        Some(metrics)
    } else {
        Some(LlamaEngineMetrics {
            slot_state,
            prompt_tokens: total_prompt_tokens,
            prompt_tokens_processed: total_prompt_processed,
            prompt_tokens_cached: total_prompt_cached,
            tokens_decoded: total_tokens_decoded,
            prompt_tokens_per_sec: 0.0,
            decode_tokens_per_sec: 0.0,
            age_ms: None,
            completed_task_tokens: None,
            latched_decode_tps: None,
            latched_prompt_tps: None,
            seconds_since_active: None,
        })
    }
}

/// Helper to parse command line arguments into LLM offload metrics
#[allow(clippy::type_complexity)]
pub fn parse_llm_args_from_cmdline(
    cmdline: &str,
) -> (
    Option<String>,
    Option<String>,
    Option<u64>,
    String,
    Option<u32>,
    Option<u32>,
    bool,
    Option<u32>,
    Option<u16>,
) {
    let parts: Vec<&str> = cmdline.split_whitespace().collect();
    let mut model_name = None;
    let mut model_path = None;
    let mut model_size_mb = None;
    let mut target_device = "HTP0".to_string();
    let mut n_threads = None;
    let mut n_gpu_layers = None;
    let mut flash_attn = false;
    let mut context_size = None;
    let mut port = None;

    let mut i = 0;
    while i < parts.len() {
        match parts[i] {
            "-m" | "--model" => {
                if i + 1 < parts.len() {
                    let path_part = parts[i + 1];
                    model_path = Some(path_part.to_string());
                    let file_name = Path::new(path_part)
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| path_part.to_string());
                    model_name = Some(file_name);
                    if let Ok(meta) = fs::metadata(path_part) {
                        model_size_mb = Some(meta.len() / (1024 * 1024));
                    }
                    i += 1;
                }
            }
            "-dev" | "--device" => {
                if i + 1 < parts.len() {
                    target_device = parts[i + 1].to_uppercase();
                    i += 1;
                }
            }
            "-t" | "--threads" => {
                if i + 1 < parts.len() {
                    n_threads = parts[i + 1].parse().ok();
                    i += 1;
                }
            }
            "-ngl" | "--n-gpu-layers" => {
                if i + 1 < parts.len() {
                    n_gpu_layers = parts[i + 1].parse().ok();
                    i += 1;
                }
            }
            "-fa" | "--flash-attn" => {
                if i + 1 < parts.len() && parts[i + 1] == "on" {
                    flash_attn = true;
                    i += 1;
                } else {
                    flash_attn = true;
                }
            }
            "-c" | "--ctx-size" => {
                if i + 1 < parts.len() {
                    context_size = parts[i + 1].parse().ok();
                    i += 1;
                }
            }
            "--port" if i + 1 < parts.len() => {
                port = parts[i + 1].parse().ok();
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }

    (
        model_name,
        model_path,
        model_size_mb,
        target_device,
        n_threads,
        n_gpu_layers,
        flash_attn,
        context_size,
        port,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_llm_args_from_cmdline() {
        let cmd = "/opt/gemma/bin/llama-server -m /models/gemma-teacher-e2b-q4_k_m.gguf --host 127.0.0.1 --port 8000 -dev HTP0 -ngl 99 -t 4 -fa on -c 2048";
        let (model, path, size, dev, threads, ngl, fa, ctx, port) =
            parse_llm_args_from_cmdline(cmd);

        assert_eq!(model, Some("gemma-teacher-e2b-q4_k_m.gguf".to_string()));
        assert_eq!(
            path,
            Some("/models/gemma-teacher-e2b-q4_k_m.gguf".to_string())
        );
        let _ = size;
        assert_eq!(dev, "HTP0");
        assert_eq!(threads, Some(4));
        assert_eq!(ngl, Some(99));
        assert!(fa);
        assert_eq!(ctx, Some(2048));
        assert_eq!(port, Some(8000));
    }

    #[test]
    fn test_inference_detection_scenarios() {
        // Scenario 1: Clean idle with model loaded
        let (status, pwr) = evaluate_inference_activity(0.0, 0.5, 36, 0, true);
        assert_eq!(status, "Idle (Model Loaded)");
        assert_eq!(pwr, 0.5);

        // Scenario 2: Idle with normal board power jitter (1.6 - 3.3W delta, 37-38°C, low CPU)
        let (status, pwr) = evaluate_inference_activity(0.0, 1.8, 37, 0, true);
        assert_eq!(status, "Idle (Model Loaded)");
        assert_eq!(pwr, 1.8);

        let (status, pwr) = evaluate_inference_activity(2.5, 3.2, 38, 0, true);
        assert_eq!(status, "Idle (Model Loaded)");
        assert_eq!(pwr, 3.2);

        // Scenario 3: Real token generation (reviewer evidence: 6.1-7.8W above idle, 42-44°C, ~66% polling CPU)
        let (status, pwr) = evaluate_inference_activity(66.0, 6.8, 43, 0, true);
        assert_eq!(status, "Active (Token Generation)");
        assert_eq!(pwr, 6.8);

        // Scenario 4: Heavy prompt prefill (power delta >= 8.2W)
        let (status, pwr) = evaluate_inference_activity(70.0, 8.7, 43, 0, true);
        assert_eq!(status, "Active (Prompt Processing)");
        assert_eq!(pwr, 8.7);

        // Scenario 5: Early token generation sample (silicon not yet warm, 38°C, but polling CPU = 55% and power delta = 6.2W)
        let (status, pwr) = evaluate_inference_activity(55.0, 6.2, 38, 0, true);
        assert_eq!(status, "Active (Token Generation)");
        assert_eq!(pwr, 6.2);

        // Scenario 6: FastRPC RPC traffic without token generation
        let (status, pwr) = evaluate_inference_activity(0.0, 0.8, 37, 120, true);
        assert_eq!(status, "Active (FastRPC RPC)");
        assert_eq!(pwr, 0.8);

        // Scenario 7: System idle without any models loaded
        let (status, pwr) = evaluate_inference_activity(0.0, 0.0, 35, 0, false);
        assert_eq!(status, "Idle");
        assert_eq!(pwr, 0.0);
    }

    #[test]
    fn test_parse_slots_json_payload() {
        // Test 1: Idle slot
        let idle_json = serde_json::json!([
            {
                "id": 0,
                "is_processing": false,
                "n_prompt_tokens": 874,
                "n_prompt_tokens_processed": 0,
                "next_token": [{"n_decoded": 0, "has_next_token": false}]
            }
        ]);
        let (state, p_tokens, p_proc, _, decoded) = parse_slots_json_payload(&idle_json);
        assert_eq!(state, "idle");
        assert_eq!(p_tokens, 874);
        assert_eq!(p_proc, 0);
        assert_eq!(decoded, 0);

        // Test 2: Active Prompt Processing (is_processing == true, n_decoded == 0)
        let prompt_json = serde_json::json!([
            {
                "id": 0,
                "is_processing": true,
                "n_prompt_tokens": 1052,
                "n_prompt_tokens_processed": 512,
                "next_token": [{"n_decoded": 0, "n_remain": 128}]
            }
        ]);
        let (state, _, _, _, decoded) = parse_slots_json_payload(&prompt_json);
        assert_eq!(state, "processing_prompt");
        assert_eq!(decoded, 0);

        // Test 3: Active Token Generation (is_processing == true, n_decoded > 0)
        let decode_json = serde_json::json!([
            {
                "id": 0,
                "is_processing": true,
                "n_prompt_tokens": 67,
                "n_prompt_tokens_processed": 12,
                "next_token": [{"n_decoded": 55, "n_remain": 65, "has_next_token": true}]
            }
        ]);
        let (state, _, _, _, decoded) = parse_slots_json_payload(&decode_json);
        assert_eq!(state, "generating_tokens");
        assert_eq!(decoded, 55);

        // Test 4: Object format for next_token
        let obj_json = serde_json::json!([
            {
                "id": 0,
                "is_processing": true,
                "n_prompt_tokens": 50,
                "n_prompt_tokens_processed": 50,
                "next_token": {"n_decoded": 10, "has_next_token": true}
            }
        ]);
        let (state, _, _, _, decoded) = parse_slots_json_payload(&obj_json);
        assert_eq!(state, "generating_tokens");
        assert_eq!(decoded, 10);
    }

    #[test]
    fn test_inference_detection_gated_on_genuine_processes() {
        // High power delta (15.0W above idle) and high polling CPU (60%) with 0 active processes
        // MUST report Idle, NEVER Active (Token Generation) or Active (Prompt Processing)!
        let (status, pwr) = evaluate_inference_activity(60.0, 15.0, 50, 200, false);
        assert_eq!(status, "Idle");
        assert_eq!(pwr, 15.0);

        // Extreme power delta (25.0W) with 0 active processes
        let (status, pwr) = evaluate_inference_activity(0.0, 25.0, 35, 0, false);
        assert_eq!(status, "Idle");
        assert_eq!(pwr, 25.0);

        // High polling CPU (80%) with 0 active processes
        let (status, pwr) = evaluate_inference_activity(80.0, 1.0, 35, 0, false);
        assert_eq!(status, "Idle");
        assert_eq!(pwr, 1.0);

        // High GLINK interrupts (100/s) with 0 active processes
        let (status, pwr) = evaluate_inference_activity(0.0, 0.5, 35, 100, false);
        assert_eq!(status, "Idle");
        assert_eq!(pwr, 0.5);
    }
}
