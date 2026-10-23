# Contributing to ventop-ai

Thank you for your interest in contributing to `ventop-ai`!

`ventop-ai` is an open-source dual-brain system and Hexagon NPU monitor built specifically for the **Arduino Ventuno Q** platform (Qualcomm Snapdragon QCS8275 + STM32H5).

---

## 1. Licensing of Contributions (`inbound = outbound`)

- **Original Contributions**: All new code, tests, and documentation contributed to `ventop-ai` are licensed under the project's [MIT License](LICENSE) (`inbound = outbound`).
- **Composite Files**: Modifications to files retaining upstream scaffolding from `ajokela/rktop` (`src/ui.rs`, `src/main.rs`) are accepted under MIT, resulting in composite files governed jointly by the MIT and BSD 3-Clause licenses (see [ATTRIBUTION.md](ATTRIBUTION.md) for full provenance details). All other source modules are 100% original work.
- **Copyright Notices**: Upstream copyright notices and SPDX file headers must never be altered or removed.
- **Dependency License Policy**: Third-party dependencies must use permissive open-source licenses (`MIT`, `Apache-2.0`, `BSD-3-Clause`, `ISC`, `Zlib`, `Unicode-3.0`, `BSL-1.0`). Copyleft licenses (GPL, AGPL, LGPL, SSPL) are strictly disallowed.

---

## 2. Target Board Safety Gate (Strict Deployment Policy)

> [!CAUTION]
> **CRITICAL SAFETY GATE: NEVER TRANSFER FILES, SCP BINARIES, OR SSH INTO THE TARGET BOARD WITHOUT EXPLICIT USER CONFIRMATION.**
> - Automated tools, CI scripts, and AI agents must **never** automatically deploy binaries or execute commands on the physical Arduino Ventuno Q board.
> - Always present the command, destination path, and binary payload size to the user first.
> - Do not alter `/etc/systemd`, bootloaders, u-boot, or kernel device tree configurations on the board without human confirmation.
> - **Git Remote Safety**: Do not push to remote git repositories without explicit user instruction (`git push` is blocked locally during reviews).

---

## 3. Development Setup & Toolchain

### Prerequisites
- **Workstation Environment**: Windows 11 with WSL2 (Ubuntu 24.04 LTS Noble) or native Linux x86_64.
- **Rust Toolchain**: Rust 1.80+ is required (due to RFC 3389 lint table stabilization and `check-cfg` support). Install via `rustup`:
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  rustup default stable
  ```
- **Cross-Compilation Toolchain**:
  ```bash
  rustup target add aarch64-unknown-linux-gnu
  sudo apt-get update
  sudo apt-get install -y gcc-aarch64-linux-gnu libc6-dev-arm64-cross
  ```

---

## 4. Build & Verification Runbook

All builds and tests must pass cleanly before submitting changes:

### From Linux / WSL2:
```bash
# 1. Check code formatting
cargo fmt --check

# 2. Run unit and CLI integration tests
cargo test

# 3. Static analysis (zero-warning policy)
cargo clippy --all-targets

# 4. Cross-compile release binary for aarch64
cargo build --target aarch64-unknown-linux-gnu --release
```

### From Windows PowerShell (via WSL2):
```powershell
# Run tests
wsl bash -c "cargo test"

# Run clippy
wsl bash -c "cargo clippy --all-targets"

# Cross-compile for Ventuno Q
wsl bash -c "cargo build --target aarch64-unknown-linux-gnu --release"
```

---

## 5. Architectural Standards & Coding Conventions

### A. Low-Overhead Sysfs Caching (<0.5% CPU Overhead)
Target hardware has strict CPU budgets for monitoring agents. Repeated polling must never repeatedly open and close file descriptors:
- Use the `FileCache` mechanism in `src/file_cache.rs` (`read_cached_file`, `read_cached_i32`, `read_cached_u32`) which holds open file handles and uses `seek(SeekFrom::Start(0))`.
- Avoid dynamic allocations in hot polling loops.

### B. Hardware Tolerance & Graceful Fallbacks
The Arduino Ventuno Q has dynamic subsystems (e.g. FastRPC sessions only exist during active inference; CAN interfaces may be unconfigured):
- **Never panic or call `.unwrap()`** on missing sysfs paths or device nodes.
- Always fall back to `None`, default zeroed metrics, or non-blocking fallbacks.

### C. Big vs Little Core Affinity
Telemetry should preserve the architectural distinction between:
- Gold Cores (Cortex-A78C, Cores 0–1)
- Prime Cores (Cortex-A78C, Cores 2–3)
- Silver Cores (Cortex-A55, Cores 4–7)

---

## 6. Binary Release Packaging (BSD-3-Clause Clause 2 Compliance)

When creating release distributions for the compiled binary, Clause 2 of the BSD 3-Clause License requires bundling the license notices:
```bash
tar -czvf ventop-ai-v0.3.9-aarch64-unknown-linux-gnu.tar.gz \
  -C target/aarch64-unknown-linux-gnu/release ventop-ai \
  -C ../../../ LICENSE ATTRIBUTION.md
```

---

## 7. Pull Request Checklist

Before submitting a pull request, ensure:
- [ ] `cargo fmt --check` passes with zero formatting diffs.
- [ ] `cargo test` passes all unit and integration tests.
- [ ] `cargo clippy --all-targets` generates zero warnings.
- [ ] `cargo build --target aarch64-unknown-linux-gnu --release` compiles successfully.
- [ ] New CLI options or metrics are reflected in `--help` and `--schema`.
- [ ] SPDX headers are retained on all modified files.
