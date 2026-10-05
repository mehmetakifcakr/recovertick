# Performance and verification report

Measured on 5 October 2026 with Rust/Cargo **1.85.1** on
`aarch64-apple-darwin` (macOS 27.0). CPU model was unavailable inside the sandbox.
Release configuration: LTO, one codegen unit, default optimization level 3.
These numbers describe this model, compiler and ABI; they are not portable limits.

## RAM layouts measured with `size_of`

```text
target_arch=aarch64
record_bytes=144
engine_capacity_1_bytes=232
engine_capacity_16_bytes=2392
engine_capacity_64_bytes=9304
```

The values include the complete `Engine<demo::Motor, N>` value and its journal;
they exclude the caller's stack, executor, driver, exported segments and archive.
The library makes zero heap allocations. Generic model/state/event/fault/effect
types can be larger than these example types. `Option<Record>` layout and ABI
alignment are compiler/target details, not a stable binary interface.

When `take_journal()` transfers a segment, the caller owns another fixed array.
Both arrays and any temporary move/initialization space must fit the application
memory budget. This API does not claim zero-copy archive extraction.

## Host timing observation

The dependency-free benchmark was executed by the release all-targets test run.
It evaluates a pure counter reducer and records/returns full decision values.
There are 10 warmup batches and 1,000 measured batches of 256 dispatches.
Initialization occurs before the timer; `black_box` protects inputs and outputs.
No driver I/O, asynchronous source wait, archive storage or recovery fault path
is included in this successful-dispatch measurement.

```text
host=aarch64 samples=1000 batch=256 (pure Counter + full record copies)
batch_average_ns_per_dispatch p50=16.93 p99=39.88
```

These percentiles describe **per-batch average time divided by 256**, not a
distribution of individual dispatch latencies. OS scheduling, cache state,
thermal behavior and measurement overhead affect the result. This single run
does not establish worst-case execution time or an MCU percentile guarantee.

Reproduce with:

```sh
cargo run --release --example footprint --locked
cargo bench --bench dispatch --locked
```

## MCU targets awaiting hardware measurements

For the bundled small model on a Cortex-M4F at 120 MHz, the design targets are:

| Metric | Target | Status |
|---|---|---|
| Engine with 16 records | At most 4 KiB static RAM | Host value 2,392 B; target ABI value not measured |
| Incremental linked engine flash | Below 8 KiB | Not measured |
| Individual `dispatch + record` p99 | Below 10 microseconds | Not measured |
| Heap allocated by core | 0 B | Source uses only `core`, fixed arrays and by-value types |
| Third-party Cargo dependencies | 0 | Manifest, lockfile and `cargo tree` verified |

Cross-compilation passed for `thumbv7em-none-eabihf`. It confirms that the library
compiles without the host standard library, not that it meets timing or flash
targets. Compare linked firmware with/without the engine under identical compiler,
linker, panic and optimization settings to measure incremental flash. Use a hardware
cycle counter and report instrumentation overhead, interrupts, clock and sample
count for dispatch percentiles, including fault and journal-full paths.

For fixed application type sizes, dispatch bookkeeping is O(1) in capacity N
plus one application reducer evaluation and record copying. Storage is O(N).
`take_journal` may cost O(N). Streaming replay is O(K) for K records plus reducer
cost, with O(N) workspace and O(N) initialization; use N=1 for verification.

## Local verification completed

* Debug and release: **25 passing tests** per configuration (2 internal unit,
  17 public API boundaries, 6 async input/cancellation tests).
* Rust documentation example: **1 passing doctest**.
* `cargo fmt --all -- --check`: passed.
* `cargo clippy --all-targets --locked -- -D warnings`: passed.
* `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --locked`: passed.
* `cargo check --lib --target thumbv7em-none-eabihf --locked`: passed.
* Packaged source was rebuilt successfully with `cargo package --locked --allow-dirty`.
* `cargo publish --dry-run --locked --allow-dirty --registry crates-io`: passed;
  Cargo aborted the upload because this was a dry run.
* Workflow/Dependabot YAML parsed; 21 workflow shell blocks passed `bash -n`.
* Release metadata accepted `v0.1.0` and rejected a mismatched `v0.2.0` tag.

The package metadata warning about missing repository/homepage/documentation URLs
is expected before a real repository exists. Replace those with actual project
URLs before public distribution; no placeholder URLs were inserted.

GitHub Actions, Linux/Windows builds on remote runners, hardware-in-the-loop,
formal verification and a real registry upload were not performed. The workflow
defines those host CI checks for the eventual repository.
