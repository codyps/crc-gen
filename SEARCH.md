# Searching and rating CRC polynomials

Requires Rust 1.89 or newer. Build once with `cargo build --release`. Commands below use `target/release/crc-gen`.
Searches need no network access. Importing new Koopman data is a separate,
explicit operation. SQLite is bundled; building it requires a C compiler.

## Candidate domains and error-detection quality

The checked-in `data/koopman/catalogue.json` contains CRC-8/16/32/64 Zoo snapshots,
including both reciprocal forms. The accompanying HTML, SHA-256 digests, original
rows, retrieval times and CC BY 4.0 attribution are retained. Rebuild from the
snapshots with `python3 scripts/import_koopman.py --offline`; refresh explicitly
with `--refresh`. The runner verifies the source files against their digests.

Koopman's implicit +1 notation is converted to the normal polynomial convention
used by this generator. Do not feed the implicit values directly to `--poly`.
The source HD lists begin at HD=3 and give **dataword bits**, excluding CRC bits.
`exact`, `lower_bound` and `unknown` are distinct. Question marks, including
tentative expressions like `?HD=8?>=1024`, stay unknown. Higher known HD bounds
can establish a lower requested HD, but unknown entries never establish one.
See `data/koopman/README.md` and the original Zoo notes.

HD describes detection: HD=d guarantees detection of up to d-1 bit flips at the
stated lengths. This search does not implement error correction or recompute HD.
Unlisted exhaustive candidates retain **unknown quality**, not an invented bound.
These source tables are not a complete census of all good 32/64-bit polynomials.

```sh
# All listed 32/64-bit candidates and reciprocal forms, both bit orders.
target/release/crc-gen search run --db results/koopman.sqlite \
  --catalogue data/koopman/catalogue.json --width 32,64 --workers 8

# All valid monic CRC-8/16 generators with constant coefficient one:
# 128 + 32,768 polynomials; both orders make 65,792 jobs at data-bits=8.
target/release/crc-gen search run --db results/exhaustive.sqlite \
  --exhaustive --width 8,16 --catalogue data/koopman/catalogue.json --workers 8

# Optional: also enumerate even and zero polynomials, including degeneracies.
# --include-even expands to all 256 / 65,536 possible low coefficient words.

# Independently compare wider update blocks; dimensions become separate tasks.
target/release/crc-gen search run --db results/blocks.sqlite \
  --catalogue data/koopman/catalogue.json --width 32,64 --data-bits 8,32,64 --workers 8
```

## Durability, resumption and parallelism

Every run has a content-addressed immutable manifest. Its identity includes the
candidate metadata, catalogue digest, dimensions, optimizer version, and a source
and dependency-lock fingerprint. Worker counts, budgets, leases and shard numbers
do not change that identity. Keep the engine binary for resuming an old run;
changed optimization code creates a new run instead of silently mixing results.
`--resume RUN_ID` refuses incompatible engine fingerprints.

The inventory is created in one transaction. Each completed job atomically saves
its compressed graphs, unit-model metrics, payload digest and timing. SQLite uses
WAL and `synchronous=FULL`; a reported completion has committed. Failures are
persisted separately and need `--retry-failed`; they are never counted as success.
Checksummed payloads and their metadata are validated when loaded. The `verify`
command checks every inventory entry and saved result in a consistent snapshot.

Workers claim jobs transactionally and renew a lease while computing. After a
process is killed, completed results remain, and unfinished jobs become eligible
when their leases expire (30 seconds by default). A stale owner cannot overwrite
another worker's result. Computation is at-least-once around a crash; committed
results are unique. Status distinguishes pending, running, expired, done and failed, and reports an
explicit `complete` flag. Lease timestamps use millisecond precision.

```sh
# Bounded work is useful for scheduled batches, tests, and initial experiments.
target/release/crc-gen search run --db results/exhaustive.sqlite \
  --exhaustive --width 8,16 --catalogue data/koopman/catalogue.json --max-jobs 1000
target/release/crc-gen search status --db results/exhaustive.sqlite

# Read RUN_ID from status. Increase/decrease concurrency on resume.
target/release/crc-gen search run --db results/exhaustive.sqlite --resume RUN_ID --workers 8
target/release/crc-gen search verify --db results/exhaustive.sqlite
```

Multiple processes can share a database **on a local filesystem**. Worker leases
and transactions coordinate them. Do not put a WAL database on NFS or a cloud
sync folder. Different machines use independent files with deterministic shards:

```sh
# Machine A and B use identical source/catalogue snapshots and engine binaries.
target/release/crc-gen search run --db shard0.sqlite --catalogue data/koopman/catalogue.json \
  --width 32,64 --shards 2 --shard-index 0 --workers 4
target/release/crc-gen search run --db shard1.sqlite --catalogue data/koopman/catalogue.json \
  --width 32,64 --shards 2 --shard-index 1 --workers 4

# Transfer consistent snapshots, then merge locally. Repeating a merge is safe.
target/release/crc-gen search snapshot --db shard0.sqlite --output shard0-backup.sqlite
target/release/crc-gen search merge --db combined.sqlite --from shard0-backup.sqlite
target/release/crc-gen search merge --db combined.sqlite --from shard1.sqlite
```

Sharding is `job ordinal % shards == shard-index`. Run every shard to cover the
domain; stopping a shard leaves honest pending counts. Changing the partitioning
on resume is safe because completed jobs are skipped. Stop destination workers
and let active leases expire before merging. Source reads use a consistent
transaction; in-flight source jobs become pending in the destination. Conflicting
completed results, manifests, or job identities abort the entire merge.

Use `snapshot`, not a copy of the main `.sqlite` file while WAL writers are active:
committed data may still be in the `-wal` file. Snapshot refuses to overwrite an
existing file. Backups protect against disk loss; transactions alone do not.

## Ratings and later CPU models

Each job retains factored-bit, masked-word, and feedback-factored-word variants as topologically
ordered, hash-consed word-operation DAGs. Exact constants, masks, shifts, edges
and the execution schedule survive in the database. The exported DAG's Rust
code describes the same operations that are rated. DAGs must be affine over
GF(2); OR is allowed only for disjoint bit ranges. Zero and basis-vector checks
against independent polynomial long division verify every result before saving.

The `gf2-shift-fold-words-v2` optimizer searches single reversible shift/XOR
folds of the shared feedback word, selecting by unit cost and dependency depth.
The masked-word baseline remains a separate variant for comparison and re-rating.
The new optimizer/source fingerprint creates a new run: historical results stay
readable, but cannot be resumed as if they had used the new simplifier.

Native-width jobs (CRC-8/16/32/64 with `data-bits=8`) also retain a complete
`block16` graph and rating, under optimizer `gf2-shift-fold-words-and-block16-v3`.
The graph includes word inputs, seed masking/mixing, all advance maps, and the
balanced final XOR tree, with shared operations interned across the whole block.
The single graph's schedule gives dependency depth and live-value pressure;
these are not sums of lane depths or register-pressure estimates.

```sh
target/release/crc-gen search rank --db results/koopman.sqlite --block16 --limit 10
target/release/crc-gen search rank --db results/koopman.sqlite --block16 --min-hd 6 --min-length 1024
target/release/crc-gen search export --db results/koopman.sqlite --job 0 --block16 --format rust
```

Block rankings are explicitly marked `method=block16`, `data_bits=128`, and
`variant=null` (the block is a separate result field, not a scalar variant).
Scalar rankings use `method=scalar` and retain their actual input dimension.
Costs are per update: divide block cost by 16 for operations per byte. Do not
compare raw 128-bit-update costs with raw 8-bit-update costs. Custom models
re-rate the retained block DAG without regenerating it; HD filters are shared.

Block graphs use version 2 with explicit native-word input nodes recording byte
offset, width and endian order. Word loads and endian conversion have zero cost
in the initial word-operation model, as do scalar inputs; a later ISA model must
account for their actual instruction costs. Graph validation enforces bounds,
shape and affine ORs. Verification covers zero, all 64 register basis bits, all
128 block input basis bits, and all ones against independent polynomial division.
The exported Rust consumes exactly `&[u8; 16]` and describes the rated DAG.

Historical scalar-only results remain readable and rankable, with no block rows
invented for them. The new optimizer and source fingerprint require a fresh run;
existing results are never silently filled or mixed with new evaluations.

Initial metrics are:

- Counts of XOR, AND, OR, left shift and right shift (input/constant nodes cost zero).
- A weighted operation count, initially one per operation.
- Longest data-dependency path, initially unit latency per operation.
- Maximum simultaneously live DAG values in its recorded execution order,
  including inputs/constants. This is a pressure indicator, not register allocation.
- Raw/canonical/factored bit-XOR counts, kept distinct from word-operation counts.

Ranking minimizes weighted cost, then critical path, then job ordinal as a stable
tie-break. Results are grouped by CRC width, update width, and bit order so an
8-bit CRC or a 1-bit update does not appear to beat a larger problem. Error quality
is a separate filter and is included in every ranked candidate's metadata.

```sh
target/release/crc-gen search rank --db results/koopman.sqlite --limit 5 \
  --min-hd 4 --min-length 1024 > ranking.json
target/release/crc-gen search rank --db results/koopman.sqlite \
  --model data/unit-model.json --limit 5
target/release/crc-gen search export --db results/koopman.sqlite --job 0 --format rust > candidate.rs
target/release/crc-gen search export --db results/koopman.sqlite --job 0 --format graph > candidate-dag.json
```

A model JSON has `id`, `costs`, and `latencies`; each operation object explicitly
contains `xor`, `and`, `or`, `shl`, `shr` integer weights. Supply a new model ID
when changing weights. Re-rating reads the existing DAGs without rerunning the
simplifier. Saved evaluations retain their full original model; ranking JSON
retains the requested model, its digest, run ID and grouped results. Zero weights are allowed; arithmetic overflow and unknown fields fail.

These are algebraic ratings, not machine-instruction or cycle predictions. ISA
lowering will need to account for register width, immediate encoding, folded
operands, fused instructions, scheduling and throughput; masks and dependencies
are retained for that work. Greedy factoring, diagonal grouping and single-fold searches do not
prove optimal forms. A later optimizer uses a new fingerprint/run and can be
compared with the retained older results.
