# AGENTS.md

This file provides guidance for AI coding assistants working with code in this repository.

## Commands

```bash
cargo build [--release]          # build all workspace members
cargo test --verbose             # run all tests
cargo fmt -- --check             # check formatting (CI enforces this)
cargo clippy --all-targets --all-features  # lint (CI treats warnings as errors: RUSTFLAGS="-Dwarnings")
cargo doc --open                 # generate and open rustdoc
cargo run --bin qdl-rs -- <args> # run the CLI tool directly
cargo run --bin qramdump -- <args>
```

CI runs `fmt`, `clippy`, `build`, and `test` on Linux, macOS, and Windows (x86 and ARM). All warnings are fatal.

**Every commit must build and pass all CI checks on its own, not just the branch tip.** `git bisect` must never land on a commit that fails to build *or that is functionally broken* — each commit has to leave the tree in a working state, not merely a compiling one. So a fix split across multiple commits has to keep each intermediate state both green and functional. When authoring or splitting commits, verify each one individually (`cargo build`, `cargo clippy --all-targets --all-features` with `RUSTFLAGS="-Dwarnings"`, `cargo fmt -- --check`, `cargo test`) before moving on.

Commits must be signed off with `git commit -s` per the DCO. **The AI agent must never add a `Signed-off-by` tag** — sign-off attests to the Developer Certificate of Origin and must be applied by a human.

AI-assisted contributions must never be submitted autonomously — a human must review, sign off, and submit every change.

When an assistant helps with a commit, identify its involvement with an `Assisted-by` trailer following the [Linux kernel coding-assistants convention](https://docs.kernel.org/process/coding-assistants.html). The format is `AGENT_NAME:MODEL_VERSION` — substitute the name and model of whichever assistant actually helped, e.g.:

```
Assisted-by: Claude:claude-sonnet-4-6
```

Append any static-analysis tools that meaningfully contributed (e.g. `coccinelle`, `sparse`), but do not list standard tools like `git` or the compiler.

## Architecture

This is a Cargo workspace with three members:

### `qdl/` — core library
The protocol implementation. Exposes everything the CLI and ramdump tools need.

- **`src/types.rs`** — shared traits and structs. `QdlReadWrite` is the raw transport trait. `QdlDevice<T>` wraps it with a `FirehoseConfiguration` and implements `QdlChan`. `QdlChan: BufRead + Write` is what all Firehose functions accept. `QdlDevice` resets the board to EDL on `Drop` if an operation failed mid-flight.
- **`src/lib.rs`** — all Firehose XML protocol functions (`firehose_configure`, `firehose_program_storage`, `firehose_read_storage`, `firehose_reset`, etc.). Also contains `load_programmer_images`, which handles both plain ELF loaders and CPIO `newc` multi-image archives (used on newer platforms like SM8750 with `xbl_s_devprg_ns.melf`).
- **`src/sahara.rs`** — Sahara binary protocol (the first stage before Firehose). `sahara_run` drives the full state machine; `sahara_dump_regions` handles crash ramdumps.
- **`src/parsers.rs`** — XML response parsers for Firehose ACK/NAK and configure responses.
- **`src/usb.rs`** / **`src/serial.rs`** — transport backends, compiled in via `features = ["usb", "serial"]`. USB uses `nusb`; serial uses `serial2`. On Windows, serial is the default backend.

### `cli/` — `qdl-rs` binary
The main user-facing tool. Parses args with `clap`, constructs a `QdlDevice`, runs Sahara to load the programmer, then dispatches Firehose commands.

- **`src/main.rs`** — CLI entry point and subcommand dispatch.
- **`src/flasher.rs`** — `flasher` subcommand: reads `rawprogramN.xml` and `patchN.xml`, drives `firehose_program_storage` and `firehose_patch` in sequence.
- **`src/programfile.rs`** — parses rawprogram/patch XML files into operation structs.
- **`src/util.rs`** — GPT helpers (`read_gpt_from_storage`, `find_part`, `print_partition_table`).

### `qramdump/` — `qramdump` binary
Connects to a crashed device, runs Sahara in memory-debug mode, and dumps selected regions via `sahara_dump_regions`. Simpler than `cli/`; no Firehose stage.

## Key design notes

- **Two-phase boot**: every operation starts with a Sahara handshake to upload a programmer ELF (or CPIO archive) into device RAM, then switches to Firehose XML over the same transport.
- **LUN/physical partition index**: UFS devices require explicit `--phys-part-idx` for all storage operations. The `flasher` command reads this from the XML files automatically; all other commands need it set explicitly.
- **Sector size matters**: wrong `--sector-size` causes NAK on `<program>` operations. Defaults vary by storage type (eMMC/NVMe: 512, UFS/NAND/SPINOR: 4096).
- **XML framing on serial**: Firehose XML packets aren't naturally delimited over serial; `firehose_read` scans for `</data>` to find message boundaries rather than relying on packet framing.

## Keeping this file current

Update this file whenever the codebase evolves in ways that affect the guidance here: new workspace members, renamed modules, changed CLI flags, new protocol stages, or revised CI requirements. Stale guidance is worse than no guidance.
