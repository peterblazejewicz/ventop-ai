// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

use crate::file_cache::resolve_sysfs_path;
use std::fs;

/// Parse DMA-BUF string content to sum megabytes allocated to CDSP compute context banks
pub fn parse_cdsp_dma_buf_from_str(content: &str) -> Option<u64> {
    let mut total_bytes = 0u64;
    let mut current_size = 0u64;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Attached")
            || trimmed.starts_with("Total")
            || trimmed.starts_with("write fence")
            || trimmed.starts_with("Dma-buf")
            || trimmed.starts_with("size")
        {
            continue;
        }

        if trimmed.contains("compute-cb@") {
            let line_lower = trimmed.to_lowercase();
            let is_non_cdsp = line_lower.contains("adsp")
                || line_lower.contains("lpass")
                || line_lower.contains("audio")
                || line_lower.contains("mpss")
                || line_lower.contains("modem")
                || line_lower.contains("slpi");
            if !is_non_cdsp && current_size > 0 {
                total_bytes += current_size;
                current_size = 0;
            }
        } else if let Some(first_word) = trimmed.split_whitespace().next() {
            if first_word.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(size) = first_word.parse::<u64>() {
                    current_size = size;
                }
            }
        }
    }

    if total_bytes > 0 {
        Some(total_bytes / (1024 * 1024))
    } else {
        None
    }
}

/// Parse /sys/kernel/debug/dma_buf/bufinfo to sum bytes allocated to CDSP compute context banks
pub fn parse_cdsp_dma_buf_mb() -> Option<u64> {
    let path = resolve_sysfs_path("/sys/kernel/debug/dma_buf/bufinfo");
    let content = fs::read_to_string(&path).ok()?;
    parse_cdsp_dma_buf_from_str(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cdsp_dma_buf_format() {
        let sample = r#"
1062932480  00000002  00080007  00000003  system  00005653  <none>
    Attached Devices:
    26300000.remoteproc:glink-edge:fastrpc:compute-cb@2
524288000  00000002  00080007  00000003  system  00005654  <none>
    Attached Devices:
    26300000.remoteproc:glink-edge:fastrpc:compute-cb@1
"#;
        let mb = parse_cdsp_dma_buf_from_str(sample).expect("should parse");
        assert_eq!(mb, 1513);
    }
}
