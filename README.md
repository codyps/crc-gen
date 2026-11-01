# crc-gen

Generate symbolic CRC update expressions and simplify them into table-free XOR
equations or standalone Rust code. This Rust implementation replaces the original
C experiment. It has no submodules. The search runner uses bundled SQLite,
compression, JSON and checksums for durable, resumable results.

See [SEARCH.md](SEARCH.md) for exhaustive CRC-8/16 searches, Koopman CRC-32/64
catalogues, parallel workers/shards, recovery, and versioned operation ratings.

```sh
cargo test
cargo run --release -- 0x1021 --width 16 --data-bits 8 --stats
cargo run --release -- 0x1021 --lsb-first --format rust > update.rs
cargo run --release -- 0x1021 --format word-rust > word_update.rs
cargo run -- --help
```

The command accepts a polynomial as a positional argument or with `--poly`.
Defaults are a 16-bit register, polynomial `0x1021`, an 8-bit input, and MSB-first
processing. Register and input widths can independently range from 1 to 64 bits.
Numbers can be decimal, hexadecimal (`0x`) or binary (`0b`).

## Symbolic model and simplification

Each register and input bit begins as a symbolic variable. The generator clocks
the polynomial's shift-register recurrence into a directed acyclic graph of XOR
expressions. Shared nodes stay shared, so the intermediate representation does
not expand into an exponential expression tree.

Simplification flattens XORs into canonical sums over GF(2): `x ^ x = 0`,
`x ^ 0 = x`, and constants fold modulo two. Factoring then repeatedly extracts
the most frequently shared pair of terms into a temporary. Temporary definitions
are topologically ordered, and ties are deterministic. Every extraction reduces
the total binary XOR count.

Output formats:

| `--format` | Output |
| --- | --- |
| `raw` | Reachable, unsimplified expression DAG using `eN` nodes |
| `simplified` | Canonical XOR sums for each output bit |
| `equations` | Factored bit equations with shared `tN` temporaries (default) |
| `rust` | A standalone `crc_update(crc: u64, data: u64) -> u64` using factored bits |
| `word-rust` | A Rust update using masked words and shared shift/XOR feedback folds |
| `block-rust` | Byte, 16-byte block, and arbitrary-slice Rust updates without tables |
| `pclmul-rust` | Scalar fallback, x86-64 PCLMUL/VPCLMUL updates, and safe runtime dispatch |
| `pmull-rust` | Scalar fallback, ARM64 PMULL updates, and safe runtime dispatch |

`--stats` writes operation counts to stderr, keeping generated code on stdout.
Raw counts measure shared DAG nodes; canonical and factored counts measure binary
XOR operations. Expanding a shared DAG into canonical sums can increase that
intermediate count before factoring reduces it.

Word simplification combines the incoming data with the feedback bits of the CRC,
then searches reversible changes of basis `t ^= t << k` and `t ^= t >> k`.
The inverse transform determines the remaining masked shifts; disjoint ranges
can be packed with OR. This rediscovers the four-bit CCITT fold for `0x1021`
in both bit orders, without polynomial-specific rules. Updates wider than the
CRC retain the masked-diagonal method.

The search retains the lowest unit operation count, breaking ties by dependency
depth, and keeps the masked-diagonal baseline if no candidate improves it.
Factoring is heuristic: it searches single folds, not every equivalent program,
and does not guarantee the shortest form or the fastest code for a CPU.

## Table-free 16-byte updates

```sh
cargo run --release -- 0xe5 --width 32 --msb-first --format block-rust > update.rs
```

`block-rust` supports CRC widths 8, 16, 32 and 64 with `--data-bits 8`.
It emits three alternatives sharing the same raw register conventions:

- `crc_update(crc, data)` consumes one byte, as in `word-rust`.
- `crc_update_16(crc, &[u8; 16])` consumes exactly 16 bytes.
- `crc_update_bytes(crc, &[u8])` uses 16-byte blocks plus byte-at-a-time tails.

Input byte order follows the slice order for both CRC bit orders. All updates
ignore high register bits; the slice helper also masks an empty-input seed.
Initialization, final reflection and final XOR are the caller's responsibility.
In the generator library, use `spec.block16()?.rust()`; `Block16::evaluate` and
`lane_graphs` expose the equivalent maps for verification and operation ratings.

A block is decomposed into register-sized words (16 byte-sized words for CRC-8, eight
16-bit words, four 32-bit words, or two 64-bit words). Let A advance the register
by one register-sized zero word. For n words, the update is
`A^n(crc XOR word[0]) XOR A^(n-1)(word[1]) XOR ... XOR A(word[n-1])`.
Each map is derived by independent polynomial division, simplified with masked
shifts and reversible XOR folds, and checked on every input basis bit. Generated
code computes independent contributions with a balanced final XOR tree; it has
no lookup tables or hardware-specific instructions. This removes the serial
byte-update chain, but dense polynomials can produce larger, slower programs.
Benchmark both alternatives on the target CPU; the slice helper selects blocks
by input length, not by a claim that blocks always win.

Durable search jobs for native widths and 8-bit input updates also store the
complete 16-byte block DAG and its ratings. Use `search rank --block16` for a
separate 128-bit-update ranking and `search export --block16 --job N` to emit
its exact recorded graph. Scalar rankings remain the default. See [SEARCH.md](SEARCH.md).

## PCLMUL and VPCLMUL byte streams

```sh
cargo run --release -- 0xe5 --width 32 --msb-first --format pclmul-rust > update.rs
```

The generator API is `spec.pclmul()?.rust()`, with `constants()` available for
inspection. All widths 1 through 64, both bit orders, and arbitrary polynomials
(including even and zero polynomials) are supported. Input must be byte-sized
(`--data-bits 8`). The generated Rust requires Rust 1.89 or newer and `std` for
runtime feature detection; it has no crate dependencies.

- `crc_update_bytes(crc, bytes)` is safe. On x86-64 it checks CPU/OS features and
  selects VPCLMUL-512 from 64 bytes, VPCLMUL-256 from 32 bytes, or PCLMUL from
  16 bytes. Short inputs, unsupported CPUs and other targets use scalar code.
- `crc_update_bytes_scalar(crc, bytes)` always uses the word simplifier.
- On x86-64, `crc_update_bytes_pclmul`, `crc_update_bytes_vpclmul256`, and
  `crc_update_bytes_vpclmul512` are **unsafe** entry points for separate benchmarking
  or callers that already checked features. Their corresponding `*_available()`
  helpers check CPU/OS support. Calling an unsupported entry point is unsound,
  even when the input is short. Target features stay within those functions;
  callers need not compile their whole program for AVX or PCLMUL.

All functions update a raw register in the same convention as `word-rust`,
mask unused seed bits even for empty input, and accept arbitrary slice alignment.
They perform no overreads. Initialization, final reflection and final XOR remain
caller-managed. Direct hardware functions use narrower paths for short inputs;
all process trailing bytes with the scalar update. Minimum dispatch lengths are
implementation boundaries, not universal performance crossover claims. Benchmark
on the intended CPU and message sizes.

The PCLMUL path uses four independent 128-bit streams for long inputs. VPCLMUL
uses two or four 128-bit lanes in 256- or 512-bit vectors. Normal-order conversion
uses byte shuffles; LSB-first input additionally reverses bits within bytes using
register shuffles. Folding needs no data-dependent memory lookups.

Constants are derived by polynomial division. For width w, the internal modulus
is `D = G * x^(64-w)`, a monic degree-64 polynomial. The normal-order raw CRC is
shifted left by `64-w`. This lets all CRC widths share the same folding and exact
reduction sequence. A fold across k bits computes
`clmul(low64(A), x^k mod D) XOR clmul(high64(A), x^(k+64) mod D) XOR B`.
The final stage advances by 64 bits and uses the reciprocal `floor(x^128 / D)`
to reduce to a 64-bit remainder, then converts back to the original register.
The generator contains no unsafe code; only the emitted, feature-gated intrinsic
functions use unsafe operations.

Hardware programs are not included in the portable word-operation DAG ratings.
Existing search results remain readable; source fingerprints include both the
backend and its template so runs from different generators cannot be mixed.

## ARM64 PMULL byte streams

```sh
cargo run --release -- 0x42f0e1eba9ea3693 --width 64 --msb-first --format pmull-rust > ecma_update.rs
```

Use `spec.pmull()?.rust()` in the generator API. It supports all widths 1..=64,
both CRC bit orders, and byte input (`--data-bits 8`). The degree-64 folding
constants and exact reciprocal reduction are shared with PCLMUL through
`folding::FoldingConstants`; the existing `pclmul::FoldingConstants` path remains
available. Neither backend adds production dependencies or executes unsafe code
inside the generator.

Generated functions use the same raw-register convention as the scalar forms:

- `crc_update_bytes(crc, bytes)` safely dispatches to PMULL from 16 bytes when
  little-endian ARM64 and CPU/OS support NEON, AES and PMULL. Otherwise it uses
  the portable scalar word form, including on non-ARM and big-endian targets.
- `crc_update_bytes_scalar(crc, bytes)` is always portable.
- On little-endian ARM64, `unsafe crc_update_bytes_pmull(crc, bytes)` is the
  direct hardware entry point. Call `pmull_available()` first, or use safe
  dispatch. Its target feature requirements apply even for short inputs.

Rust groups PMULL intrinsics under the `aes` target feature; runtime dispatch
checks the distinct AES and PMULL capabilities plus NEON. Hardware features stay
inside the generated functions. Four independent 128-bit accumulators process
bulk buffers, with narrower folds and scalar tails. RBIT and byte shuffles
convert reflected input to the common polynomial representation. No overreads,
alignment assumptions, lookup tables, initialization or final XOR are introduced.
The code uses `std` for runtime detection and is intended for userspace; kernel
integration must use the kernel's own SIMD context and feature APIs.

[The ARM64 benchmark](benchmarks/arm64/README.md) compares CRC-64/ECMA and common
CRCs against scalar byte/block forms and `crc` single-table/slicing-by-16 methods.
It refuses timing on non-ARM64 or PMULL-less hosts. `--check` supports portable
and emulated correctness checks. The native GitHub workflow records the CPU,
compiler, exact source, all timing rounds, and the execution architecture.

The PMULL regression compiles standalone generated programs, checks an independent
polynomial-division oracle, every data/seed basis bit, arbitrary seeds, tails,
unaligned buffers and incremental chunks. Linux guard-page checks use the runtime
page size, including ARM systems with larger pages. To cross-check generated
programs locally, `CRC_GEN_PMULL_TARGET`, `CRC_GEN_PMULL_RUSTFLAGS_JSON`,
`CRC_GEN_PMULL_RUNNER` and `CRC_GEN_PMULL_RUNNER_ARGS_JSON` configure compilation
and execution without shell evaluation. Set `CRC_GEN_REQUIRE_PMULL=1` to fail
rather than silently skip hardware when the test environment promises PMULL.

## Bit and polynomial conventions

- The polynomial is always in normal form, with the leading `x^width` coefficient
  omitted. Pass `0x1021` for CCITT in either bit order, not `0x11021` or `0x8408`.
- Bit indices count from the least significant bit: `crc[0]`, `data[0]`, `out[0]`.
- `--msb-first` consumes `data[data_bits - 1]` first. `--lsb-first` consumes
  `data[0]` first and reverses the polynomial internally.
- Only the specified low bits of `crc` and `data` are used. Output is restricted
  to the register width, including for widths smaller than a byte.
- The generator emits a streaming register update, not a complete named CRC
  checksum. It does not apply initialization, augmentation, final reflection,
  or final XOR.

For a catalogue CRC whose `refin` matches the selected bit order, initialize the
register with `init` for MSB-first, or with `init` bit-reversed across the register
width for LSB-first. After the final update, reverse the register if
`refin != refout`, then apply `xorout`. The pre-existing CRC reference document
is retained in `doc/`.

## Library API

```rust
use crc_gen::{BitOrder, CrcSpec};

let spec = CrcSpec::new(16, 0x1021, 8, BitOrder::MsbFirst).unwrap();
let symbolic = spec.symbolic();
let equations = symbolic.simplify();
let program = equations.factor();

assert_eq!(program.evaluate(0, b'1'.into()), 0x2672);
println!("{}", program.equations());
println!("{}", program.rust());
```

Tests cover XOR cancellation and constant folding, nested shared expressions,
exhaustive small CRCs, known CRC-8/16/32/64 check values, and zero/basis-vector
equivalence against independent polynomial long division. For linear maps the
basis-vector checks establish equivalence for every input assignment. Tests also
compile and execute both Rust output forms, including 1- and 64-bit edge cases,
and check CLI validation and all output formats.
