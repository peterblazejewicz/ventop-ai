// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use crate::file_cache::resolve_sysfs_path;
use std::fs;
use std::sync::Mutex;
use std::time::Instant;

/// State for tracking GLINK interrupt delta rate
struct GlinkRateTracker {
    last_count: u64,
    last_time: Instant,
}

static GLINK_TRACKER: Mutex<Option<GlinkRateTracker>> = Mutex::new(None);

/// Parse total GLINK mailbox interrupt count specifically associated with CDSP from /proc/interrupts
pub fn read_glink_interrupt_count() -> Option<u64> {
    let path = resolve_sysfs_path("/proc/interrupts");
    let content = fs::read_to_string(&path).ok()?;
    parse_glink_interrupt_count_from_str(&content)
}

/// Parse total GLINK mailbox interrupt count specifically associated with CDSP
pub fn parse_glink_interrupt_count_from_str(content: &str) -> Option<u64> {
    for line in content.lines() {
        let line_lower = line.to_lowercase();
        if line_lower.contains("glink")
            && (line_lower.contains("cdsp") || line_lower.contains("compute"))
        {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let mut total = 0u64;
            for part in parts.iter().skip(1) {
                if let Ok(count) = part.parse::<u64>() {
                    total += count;
                } else {
                    break;
                }
            }
            return Some(total);
        }
    }
    None
}

/// Compute delta GLINK mailbox interrupts per second
pub fn get_glink_interrupt_rate() -> u64 {
    let now = Instant::now();
    let current_count = read_glink_interrupt_count().unwrap_or(0);

    let mut tracker_lock = GLINK_TRACKER.lock().unwrap();

    if let Some(ref mut tracker) = *tracker_lock {
        let elapsed = now.duration_since(tracker.last_time).as_secs_f64();
        let delta = current_count.saturating_sub(tracker.last_count);

        tracker.last_count = current_count;
        tracker.last_time = now;

        if elapsed > 0.05 {
            return (delta as f64 / elapsed).round() as u64;
        }
        0
    } else {
        *tracker_lock = Some(GlinkRateTracker {
            last_count: current_count,
            last_time: now,
        });
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glink_interrupt_parsing_filters_adsp() {
        let content = "\
 12:       1000       2000       GICv3  12  glink-smem-adsp\n\
 24:        150        350       GICv3  24  glink-smem-cdsp\n";
        let count = parse_glink_interrupt_count_from_str(content).expect("should find CDSP line");
        assert_eq!(count, 500); // 150 + 350, correctly ignoring 3000 from ADSP!

        let adsp_only = " 12: 1000 2000 GICv3 12 glink-smem-adsp\n";
        assert_eq!(parse_glink_interrupt_count_from_str(adsp_only), None);
    }
}
