# Native ARM64 CRC benchmark

This benchmark compares the generated scalar byte form, scalar 16-byte block
form, ARM64 PMULL, and safe automatic dispatch against `crc 3.4.0` Table1 and
Table16. Build-time code generation uses the exact generator in this checkout;
no search databases, downloaded generated code, or host-specific paths are needed.

Cases include CRC-8/SMBUS and MAXIM, CRC-16/ARC, XMODEM and KERMIT, CRC-32/IEEE
and CRC-32C, CRC-64/ECMA, WE, XZ, GO-ISO and NVMe, plus the sparse CRC-32 `0xe5`
and CRC-64 `0xf1` candidates. Common algorithms retain their standard parameters;
the candidates use zero initialization/final XOR. Every implementation is checked
at 268 lengths and against known standard check values before any timing.

On native little-endian ARM64 with AES/PMULL/NEON:

```sh
cargo build --release --locked --manifest-path benchmarks/arm64/Cargo.toml
bench_cpu=$(python3 -c 'import os; print(min(os.sched_getaffinity(0)))')
taskset -c "$bench_cpu" benchmarks/arm64/target/release/crc-gen-arm64-bench \
  --metadata metadata.json > timings.csv
```

To validate correctness without collecting timings, including on x86 or under
QEMU, pass `--check`. Direct PMULL is registered only after feature detection;
native timing refuses an unsupported architecture/CPU. Do not use emulated
throughput to make ARM performance claims.

The default benchmark uses nine interleaved 20-ms rounds for 0, 8, 16, 64, 512,
4096 and 65536-byte buffers, with 128 calls per timer check. CSV contains medians
and quartiles in ns/checksum; divide by the nonzero input size for ns/byte. JSON
also keeps every round, CPU information, compiler version, affinity and the
source revision when `CRC_GEN_BENCH_COMMIT` was set at build time. Results include
function-call and finalization overhead. Data and tables are hot. CRC32 dedicated
instructions, cold caches, and kernel SIMD-context overhead are not measured.
PMULL and dispatch use scalar code below 16 bytes; dispatch boundaries are not a
claim of the universally optimal crossover.

The workflow `.github/workflows/arm64-pmull.yml` runs tests and benchmarks on a
native GitHub `ubuntu-24.04-arm` runner and uploads all records as
`arm64-pmull-results`. Its public runner is virtualized ARM hardware, not an
emulator or a claim about all ARM microarchitectures. The workflow uses Rust
1.99.0 for reproducibility; the generator's library MSRV remains 1.89.
