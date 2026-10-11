# Attribution, Prior Art & Third-Party Notices

`ventop-ai` is released by Piotr Błażejewicz and contributors under a composite license structure:
- **Original Work**: [MIT License](LICENSE) (Copyright © 2026 Piotr Błażejewicz and contributors).
- **Ported & Derived Portions**: [BSD 3-Clause License](LICENSE) (Copyright © 2025 Alex Jokela and contributors).

This document details project lineage, TASL attribution, component provenance, upstream license texts, third-party crate dependencies, and binary distribution requirements.

---

## 1. Project Lineage & Upstream Prior Art

`ventop-ai` originated as an evolution of [`rktop`](https://github.com/ajokela/rktop), an open-source terminal system monitor for Rockchip SoCs created by Alex Jokela and contributors under the BSD 3-Clause License.

### Upstream Metadata (TASL)
* **Title**: `rktop` (Rockchip SoC Monitor)
* **Author / Copyright Holder**: Alex Jokela and contributors
* **Source Repository**: <https://github.com/ajokela/rktop>
* **Upstream License**: BSD 3-Clause License (reproduced in Section 3)

### Architectural Divergence
While `rktop` was built for Rockchip SoCs (such as the RK3588 and RK3399) monitoring ARM Mali GPUs and Rockchip NPUs via vendor sysfs nodes, `ventop-ai` was re-architected for the heterogeneous **Arduino Ventuno Q** dual-brain architecture:

1. **Qualcomm Snapdragon QCS8275 AI Brain**:
   - Replaced Rockchip NPU sysfs monitoring with Qualcomm Hexagon v75 HTP NPU telemetry: FastRPC descriptor scanning, background daemon filtering (`cdsprpcd`, `adsprpcd`), DMA-BUF VRAM accounting (`compute-cb@*` via debugfs), GLINK mailbox IRQ rate analysis, and on-die NSP silicon thermals.
   - Replaced Mali GPU monitoring with dynamic Adreno 623 devfreq scaling and governor discovery.
   - Implemented dynamic 3-tier CPU cluster discovery across Gold (A78C), Prime (A78C), and Silver (A55) cores via `/sys/devices/system/cpu/cpufreq/policy*`.
2. **STMicroelectronics STM32H5F5 Action Brain**:
   - Added MCU telemetry over internal USB CDC-ACM serial (`/dev/ttySTM0` @ 115200 baud).
   - Added `arduino-router.service` MsgPack RPC Unix socket monitoring (`/var/run/arduino-router.sock`).
   - Added link state and byte counters for 3x CAN-FD interfaces (`can0`, `can1`, `can2`).
3. **Power, Thermals & Telemetry Engine**:
   - Added dynamic TI INA232 hardware monitor power telemetry with baseline subtraction.
   - Added dynamic Microchip EMC2305 PWM cooling fan monitor live tachometer RPM.
   - Aggregated 6 consolidated thermal zones from 48 hardware TSENS sensors dynamically by sensor type.
   - Added high-frequency NDJSON telemetry streaming (`--stream`), interval delta CPU percentage calculation, dynamic phase marker injection (`--mark-file`), process thread core affinity analysis (`--proc`), and machine-readable schema discovery (`--schema`).

---

## 2. Component Provenance & Lineage Inventory

All legacy Rockchip-specific monitoring code was removed or rewritten for the Qualcomm Snapdragon QCS8275 and STMicroelectronics STM32H5F5 platform. Retained prior-art code from `ajokela/rktop` is confined to ~115 lines of standard TUI terminal initialization and layout scaffolding in `ui.rs` and `main.rs` (~1.5% of `src/`):

| Component | Provenance Status | Origin / Prior Art | Derived Lines | Scope & Implementation Details |
|---|---|---|---|---|
| `src/action_brain.rs` | **Original Work** | None (New) | 0 (0%) | STM32H5 CDC-ACM serial detection, `arduino-router` Unix socket state, and 3x CAN-FD interface telemetry. |
| `src/hexagon/` | **Original Work** | None (New) | 0 (0%) | Qualcomm Hexagon v75 HTP NPU monitoring: submodules for inference evaluation, FastRPC scanning, DMA-BUF VRAM accounting, GLINK IRQ rate, and silicon thermals. |
| `src/snapshot/` | **Original Work** | None (New) | 0 (0%) | NDJSON telemetry engine: submodules for snapshot types, streaming with delta tick math, statistical summaries, and schema discovery (`--schema`). |
| `src/file_cache.rs` | **Original Work** | None (Rewritten) | 0 (0%) | File handle cache with error eviction, descriptor rewinding, `resolve_sysfs_path` abstraction, and `EnvGuard` test harness. |
| `src/hardware/` | **Original Work** | None (Rewritten) | 0 (0%) | Dynamic platform discovery: submodules for CPU topology & cpufreq, Adreno GPU devfreq, INA232/EMC2305 power/fan monitoring, 48-TSENS thermals, and platform facts. |
| `src/sysinfo_ext.rs` | **Original Work** | None (Rewritten) | 0 (0%) | Single-pass `/proc/<pid>/stat` and `/proc/<pid>/status` parsing, `CpuTicks` delta calculations, Big vs Little CPU cluster attribution for QCS8275, and process thread tracking. |
| `src/ui.rs` | **Original with Scaffolding** | `ajokela/rktop` (Derived) | ~80 (~6.5%) | Dual-brain dashboard layout, NPU, MCU, power, and fan panels, vertical table scrolling. Retains ~80 lines of ratatui table widget layout and column formatting scaffolding. |
| `src/main.rs` | **Original with Scaffolding** | `ajokela/rktop` (Derived) | ~35 (~7.4%) | CLI entry point, NDJSON streaming, phase markers, `--proc`, `--schema`, and duration controls. Retains ~35 lines of crossterm terminal initialization, raw mode, and event loop dispatch. |
| `src/lib.rs` | **Original Work** | None (New) | 0 (0%) | Module declarations and public API surface for integration and fixture testing. |
| `build.rs` | **Original Work** | None (New) | 0 (0%) | Git commit hash embedding and rebuild triggers. |
| `tests/cli_tests.rs` | **Original Work** | None (New) | 0 (0%) | CLI integration test suite verifying `--help`, `--version`, `--schema`, `--json`, and `--oneshot`. |
| `tests/fixture_tests.rs` | **Original Work** | None (New) | 0 (0%) | Synthetic fixture test suite verifying alternate sysfs layouts, renumbered zones/hwmon, daemon filtering, high power baseline, and delta CPU accounting. |
| **Total Codebase** | **Composite (MIT AND BSD-3-Clause)** | `ajokela/rktop` + Original | **~115 (~1.5%)** | **Retained prior-art lines reduced from ~760 (12.7%) to ~115 (~1.5% of `src/` across 7,486 lines)**, confined to standard ratatui/crossterm terminal scaffolding. |

---

## 3. Upstream BSD 3-Clause License Text

The following license applies to the portions of `ventop-ai` ported or derived from `ajokela/rktop` (retained in git history and standard TUI terminal scaffolding):

```text
Copyright (c) 2025, Alex Jokela and contributors
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

---

## 4. Third-Party Crate Dependencies

`ventop-ai` compiles with the following direct third-party Rust dependencies:

| Crate | Version | License | Primary Role |
|---|---|---|---|
| `ratatui` | 0.28 | MIT | Terminal user interface rendering |
| `crossterm` | 0.28 | MIT | Terminal raw mode and input handling |
| `sysinfo` | 0.32 | MIT | Procfs/sysfs system metrics and process enumeration |
| `anyhow` | 1.0 | MIT OR Apache-2.0 | Application error handling |
| `nix` | 0.31 | MIT | Unix system call abstractions |
| `libc` | 0.2 | MIT OR Apache-2.0 | Low-level POSIX and C library bindings |
| `serde` | 1.0 | MIT OR Apache-2.0 | High-frequency telemetry serialization |
| `serde_json` | 1.0 | MIT OR Apache-2.0 | JSON snapshot and schema dictionary generation |
| `tempfile` *(dev)* | 3 | MIT OR Apache-2.0 | Mock filesystem fixture isolation in test suite |

All direct and transitive dependencies are governed by permissive open-source licenses (`MIT`, `Apache-2.0`, `BSD-3-Clause`, `Zlib`, `Unicode-3.0`, `BSL-1.0`) and are continuously audited via `cargo deny check`.

---

## 5. Binary Distribution & Packaging Instructions (BSD-3-Clause Clause 2)

Clause 2 of the upstream BSD 3-Clause License stipulates:
> *"Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution."*

To satisfy this legal requirement when distributing precompiled binaries (`ventop-ai` compiled for `aarch64-unknown-linux-gnu` or `x86_64-unknown-linux-gnu`):
1. **Never distribute bare executable binaries** in release artifacts without accompanying license documentation.
2. **Bundle `LICENSE` and `ATTRIBUTION.md`** inside every distribution archive.

### Release Packaging Command
```bash
# Example packaging for aarch64 release from repository root:
tar -czvf ventop-ai-v0.3.8-aarch64-unknown-linux-gnu.tar.gz \
  LICENSE ATTRIBUTION.md \
  -C target/aarch64-unknown-linux-gnu/release ventop-ai
```

Archive contents:
- `ventop-ai` (executable binary)
- `LICENSE` (composite MIT + BSD-3-Clause text)
- `ATTRIBUTION.md` (TASL provenance, upstream notices, component inventory, and dependency licenses)
