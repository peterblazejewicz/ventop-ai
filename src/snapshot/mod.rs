// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Piotr Błażejewicz and contributors

pub mod schema;
pub mod stream;
pub mod summary;
pub mod types;

pub use schema::{get_schema_json, print_schema};
pub use stream::{
    collect_snapshot, collect_snapshot_continuous, compute_cpu_pct_from_stats, print_json_snapshot,
    print_oneshot_snapshot, reset_prev_cpu_stats, stream_json_snapshots, StreamOptions,
};
pub use summary::{
    calc_f32_stats, calc_f64_stats, calc_u32_stats, is_end_marker, print_summary, PhaseSummary,
    SummaryTracker,
};
pub use types::{
    CpuSnapshot, HostInfo, MarkerRecord, MemorySnapshot, NpuPhase, ProcessLifecycleEvent,
    ProcessSnapshot, SampleTelemetry, StatSummary, StatsSnapshot, VentopSnapshot,
};
