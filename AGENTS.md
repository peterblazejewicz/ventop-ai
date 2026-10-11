# AGENTS.md - Cross-Agent Development Guide for `ventop-ai`

## 1. Multi-Agent Collaboration Model (x-Agent Workflow)

`ventop-ai` utilizes a paired cross-agent (x-agent) workflow between **Antigravity (Agy)** and **Claude**, operating under the direction and supervision of the human maintainer:

```
+-------------------------------------------------------------------------+
|                  Antigravity (Agy) Session                              |
|  - Host Environment: Windows 11 / WSL2 (Ubuntu 24.04 Noble)             |
|  - Role: Codebase architect, core Rust development, cross-compiler     |
|  - Responsibilities:                                                    |
|      * Maintain low-overhead sysfs file cache (<0.5% CPU overhead)      |
|      * Develop Qualcomm Hexagon NPU & STM32H5 Action Brain telemetry   |
|      * Cross-compile release binaries for target aarch64-linux-gnu      |
|      * Run unit tests via WSL2 and manage git repositories              |
+-------------------------------------------------------------------------+
                                    │
                                    │ Artifact: target/aarch64-.../release/ventop-ai
                                    │ [CRITICAL SAFETY GATE: User Confirmation Required]
                                    ▼
+-------------------------------------------------------------------------+
|                    Claude Session (Board Target)                        |
|  - Target Board:    Arduino Ventuno Q (Snapdragon QCS8275 + STM32H5)    |
|  - Role: Live target profiling, hardware validation, board optimization |
|  - Responsibilities:                                                    |
|      * Validate sysfs paths, devfreq, FastRPC, and remote processors    |
|      * Profile CPU footprint (ensuring < 0.5% CPU overhead)             |
|      * Report board discrepancies back to codebase developer            |
+-------------------------------------------------------------------------+
```

---

## 2. Crucial Safety Gate (Deployment Policy)

> [!CAUTION]
> **SAFETY GATE: NEVER TRANSFER FILES, SCP BINARIES, OR MODIFY BOARD FILES WITHOUT EXPLICIT USER CONFIRMATION.**
> - Neither Antigravity nor Claude may automatically execute `scp`, `rsync`, `ssh`, or file deployment to the target board without asking for and receiving explicit user approval.
> - Always present the command, destination, and payload size to the user first.
> - Do not modify `/etc/systemd`, bootloaders, u-boot, or root partition configuration on the board without explicit confirmation.

---

## 3. Platform Architecture: Arduino Ventuno Q

### A. The AI Brain
- **SoC**: Qualcomm Snapdragon QCS8275 (Monaco-Monza / QCS8300)
- **CPU**: 8 Cores (2x A78C Gold @ 2.11 GHz + 2x A78C Prime @ 2.36 GHz + 4x A55 Silver @ 1.96 GHz)
- **GPU**: Qualcomm Adreno 623 (479–877 MHz devfreq)
- **NPU**: Qualcomm Hexagon v75 HTP (40 dense TOPS, FastRPC `/dev/fastrpc-cdsp`, DMA-BUF VRAM)
- **Memory**: 16 GB LPDDR5 Unified System RAM

### B. The Action Brain
- **Microcontroller**: STM32H5F5 Arm Cortex-M33 @ 250MHz
- **Serial Link**: Internal USB CDC-ACM `/dev/ttySTM0` @ 115200 baud
- **RPC Service**: `arduino-router.service` on `/var/run/arduino-router.sock`
- **CAN-FD**: 3x CAN-FD interfaces (`can0`, `can1`, `can2`)

### C. Power & Cooling
- **Power Sensor**: TI INA232 hardware monitor (`hwmon12`, board power in Watts)
- **Fan Controller**: Microchip EMC2305 PWM Fan Controller (`hwmon0`, live RPM)
- **Thermals**: 6-Zone consolidated thermals (filtering 48 hardware TSENS zones)

---

## 4. Toolchain and Cross-Compilation Runbook

Cross-compilation is executed on the Windows workstation using **WSL2 (Ubuntu 24.04 LTS Noble)**:

### Build Commands
From Windows PowerShell:
```powershell
# Run unit tests
wsl bash -c "cargo test"

# Build release binary for target aarch64
wsl bash -c "cargo build --target aarch64-unknown-linux-gnu --release"
```

Resulting binary:
- `target/aarch64-unknown-linux-gnu/release/ventop-ai` (~1.2 MB stripped)

---

## 5. Testing & Validation

All tests must be executed via WSL2 or on the target board:

```powershell
# Run unit tests from Windows via WSL2
wsl bash -c "cargo test"
```

Unit test coverage:
- DMA-BUF VRAM parser for FastRPC context banks
- Command line model extractor (`-m model.gguf`)
- STM32H5 Action Brain status defaults
- Qualcomm SoC model detection fallback
- NDJSON snapshot serialization & schema dictionary
- Task transition latching for terminal tokens
- Summary marker intervals & per-phase profiling
