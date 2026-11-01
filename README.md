# crc-gen

Generate symbolic CRC update expressions and simplify them into table-free XOR
equations or standalone Rust code. This Rust implementation replaces the original
C experiment and needs no dependencies or submodules.

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
| `word-rust` | An alternative Rust update grouping equal shifts into masked words |

`--stats` writes operation counts to stderr, keeping generated code on stdout.
Raw counts measure shared DAG nodes; canonical and factored counts measure binary
XOR operations. Expanding a shared DAG into canonical sums can increase that
intermediate count before factoring reduces it.

Factoring is greedy, not globally optimal. Word output groups masks and shifts;
it does not search all equivalent word-level programs. These methods do not
promise to recover the shortest hand-optimized CCITT equation or the fastest
code for a particular CPU.

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
