//! Transactional, leased search jobs. SQLite WAL is for a local filesystem;
//! distributed machines use separate shard databases and merge completed work.

use crate::catalogue::{sha256, Candidate, Source};
use crate::rating::{CostModel, Evaluation, Metrics, OPTIMIZER};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const APPLICATION_ID: i64 = 0x43524347;
const SCHEMA: i64 = 1;
const MAX_RESULT: u64 = 16 * 1024 * 1024;

pub fn engine_fingerprint() -> String {
    sha256(
        concat!(
            env!("CARGO_PKG_VERSION"),
            include_str!("lib.rs"),
            include_str!("rating.rs"),
            include_str!("block.rs"),
            include_str!("pclmul.rs"),
            include_str!("pclmul.rs.in"),
            include_str!("catalogue.rs"),
            include_str!("search.rs"),
            include_str!("../Cargo.lock")
        )
        .as_bytes(),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogueIdentity {
    pub sha256: String,
    pub sources: Vec<Source>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub engine: String,
    pub optimizer: String,
    pub domain: String,
    pub include_even: bool,
    pub catalogue: Option<CatalogueIdentity>,
    pub candidates: Vec<Candidate>,
    pub data_bits: Vec<u8>,
    pub orders: Vec<String>,
}

impl Manifest {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || self.optimizer.is_empty() {
            return Err("unsupported manifest version".into());
        }
        if self.candidates.is_empty() || self.data_bits.is_empty() || self.orders.is_empty() {
            return Err("search has no jobs".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for candidate in &self.candidates {
            candidate.validate()?;
            if !seen.insert((candidate.width, candidate.polynomial.clone())) {
                return Err("duplicate candidate".into());
            }
            for bits in &self.data_bits {
                for order in &self.orders {
                    candidate.spec(*bits, order)?;
                }
            }
        }
        if self
            .data_bits
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != self.data_bits.len()
            || self
                .orders
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.orders.len()
        {
            return Err("duplicate search dimension".into());
        }
        match self.domain.as_str() {
            "exhaustive" => {
                let mut counts = BTreeMap::<u8, usize>::new();
                for candidate in &self.candidates {
                    if !matches!(candidate.width, 8 | 16)
                        || (!self.include_even
                            && crate::catalogue::polynomial(&candidate.polynomial)? & 1 == 0)
                    {
                        return Err("invalid exhaustive domain".into());
                    }
                    *counts.entry(candidate.width).or_default() += 1;
                }
                for (width, count) in counts {
                    let expected = 1usize << (width - u8::from(!self.include_even));
                    if count != expected {
                        return Err(
                            "exhaustive manifest does not cover the entire polynomial domain"
                                .into(),
                        );
                    }
                }
            }
            "catalogue" if !self.include_even => (),
            _ => return Err("unsupported search domain".into()),
        }
        Ok(())
    }
    pub fn id(&self) -> Result<String> {
        Ok(sha256(&serde_json::to_vec(self)?))
    }
    pub fn total(&self) -> usize {
        self.candidates.len() * self.data_bits.len() * self.orders.len()
    }
    fn check_job(&self, ordinal: usize, candidate: &str, bits: u8, order: &str) -> Result<()> {
        if ordinal >= self.total() {
            return Err("job ordinal is outside the manifest".into());
        }
        let expected = &self.candidates[ordinal / (self.data_bits.len() * self.orders.len())];
        if serde_json::to_string(expected)? != candidate
            || self.data_bits[(ordinal / self.orders.len()) % self.data_bits.len()] != bits
            || self.orders[ordinal % self.orders.len()] != order
        {
            return Err("job identity disagrees with the manifest".into());
        }
        Ok(())
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn check_database(conn: &Connection) -> Result<()> {
    let app: i64 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if app != APPLICATION_ID || version != SCHEMA {
        return Err("not a supported crc-gen search database".into());
    }
    Ok(())
}

fn existing(path: &Path, writable: bool) -> Result<Connection> {
    let flags = if writable {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let conn = Connection::open_with_flags(path, flags)?;
    conn.busy_timeout(Duration::from_secs(20))?;
    check_database(&conn)?;
    if writable {
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
    }
    Ok(conn)
}

fn initialize(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(20))?;
    let app: i64 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let tables: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table'",
        [],
        |row| row.get(0),
    )?;
    if app != APPLICATION_ID && (app != 0 || tables != 0) {
        return Err("refusing to alter an unrelated database".into());
    }
    if app == APPLICATION_ID {
        check_database(&conn)?;
    }
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(&format!("BEGIN IMMEDIATE;
        PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version={SCHEMA};
        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY, manifest TEXT NOT NULL, created_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS jobs (
            run_id TEXT NOT NULL REFERENCES runs(id), ordinal INTEGER NOT NULL,
            candidate TEXT NOT NULL, data_bits INTEGER NOT NULL, bit_order TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('pending','running','done','failed')),
            owner TEXT, lease_until INTEGER, attempts INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL, elapsed_ms INTEGER,
            result BLOB, result_sha256 TEXT, summary TEXT, error TEXT,
            PRIMARY KEY(run_id, ordinal),
            CHECK(state != 'done' OR (result IS NOT NULL AND result_sha256 IS NOT NULL AND summary IS NOT NULL)));
        CREATE INDEX IF NOT EXISTS queue ON jobs(run_id,state,ordinal);
        COMMIT;"))?;
    Ok(conn)
}

pub fn prepare(path: &Path, manifest: &Manifest) -> Result<String> {
    manifest.validate()?;
    if manifest.engine != engine_fingerprint() || manifest.optimizer != OPTIMIZER {
        return Err(
            "manifest uses a different search engine; use the original binary to resume".into(),
        );
    }
    let json = serde_json::to_string(manifest)?;
    let id = manifest.id()?;
    let mut conn = initialize(path)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let previous: Option<String> = tx
        .query_row("SELECT manifest FROM runs WHERE id=?1", [&id], |row| {
            row.get(0)
        })
        .optional()?;
    if let Some(previous) = previous {
        if previous != json {
            return Err("manifest hash conflict".into());
        }
        let count: usize =
            tx.query_row("SELECT count(*) FROM jobs WHERE run_id=?1", [&id], |row| {
                row.get(0)
            })?;
        if count != manifest.total() {
            return Err("run has an incomplete or corrupt job inventory".into());
        }
    } else {
        tx.execute(
            "INSERT INTO runs VALUES (?1,?2,?3)",
            params![id, json, now()],
        )?;
        let mut insert = tx.prepare("INSERT INTO jobs (run_id,ordinal,candidate,data_bits,bit_order,state,updated_at) VALUES (?1,?2,?3,?4,?5,'pending',?6)")?;
        let mut ordinal = 0usize;
        for candidate in &manifest.candidates {
            for bits in &manifest.data_bits {
                for order in &manifest.orders {
                    insert.execute(params![
                        id,
                        ordinal,
                        serde_json::to_string(candidate)?,
                        bits,
                        order,
                        now()
                    ])?;
                    ordinal += 1;
                }
            }
        }
    }
    tx.commit()?;
    Ok(id)
}

pub fn manifest(path: &Path, id: Option<&str>) -> Result<(String, Manifest)> {
    let conn = existing(path, false)?;
    let id = choose_run(&conn, id)?;
    let text: String = conn.query_row("SELECT manifest FROM runs WHERE id=?1", [&id], |row| {
        row.get(0)
    })?;
    if sha256(text.as_bytes()) != id {
        return Err("manifest checksum mismatch".into());
    }
    let manifest: Manifest = serde_json::from_str(&text)?;
    manifest.validate()?;
    Ok((id, manifest))
}

fn choose_run(conn: &Connection, id: Option<&str>) -> Result<String> {
    if let Some(id) = id {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1)",
            [id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err("unknown run id".into());
        }
        return Ok(id.to_string());
    }
    let mut statement = conn.prepare("SELECT id FROM runs ORDER BY id LIMIT 2")?;
    let ids: Vec<String> = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if ids.len() != 1 {
        return Err("database needs exactly one run, or specify --run-id".into());
    }
    Ok(ids[0].clone())
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub workers: usize,
    pub shard_index: usize,
    pub shards: usize,
    pub max_jobs: Option<usize>,
    pub lease_seconds: u64,
    pub retry_failed: bool,
}
impl Default for RunOptions {
    fn default() -> Self {
        Self {
            workers: 1,
            shard_index: 0,
            shards: 1,
            max_jobs: None,
            lease_seconds: 30,
            retry_failed: false,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub run_id: String,
    pub total: usize,
    pub done: usize,
    pub pending: usize,
    pub running: usize,
    pub expired: usize,
    pub failed: usize,
    pub complete: bool,
    pub errors: Vec<(usize, String)>,
}

pub fn status(path: &Path, run: Option<&str>) -> Result<Status> {
    let conn = existing(path, false)?;
    conn.execute_batch("BEGIN")?;
    let id = choose_run(&conn, run)?;
    let json: String = conn.query_row("SELECT manifest FROM runs WHERE id=?1", [&id], |row| {
        row.get(0)
    })?;
    if sha256(json.as_bytes()) != id {
        return Err("manifest checksum mismatch".into());
    }
    let manifest: Manifest = serde_json::from_str(&json)?;
    let mut result = Status {
        run_id: id.clone(),
        total: 0,
        done: 0,
        pending: 0,
        running: 0,
        expired: 0,
        failed: 0,
        complete: false,
        errors: vec![],
    };
    let mut query =
        conn.prepare("SELECT state,count(*) FROM jobs WHERE run_id=?1 GROUP BY state")?;
    for row in query.query_map([&id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
    })? {
        let (state, count) = row?;
        result.total += count;
        match state.as_str() {
            "done" => result.done = count,
            "pending" => result.pending = count,
            "running" => result.running = count,
            "failed" => result.failed = count,
            _ => unreachable!(),
        }
    }
    if result.total != manifest.total() {
        return Err("run has an incomplete job inventory".into());
    }
    result.complete = result.total == result.done;
    result.expired = conn.query_row(
        "SELECT count(*) FROM jobs WHERE run_id=?1 AND state='running' AND lease_until<=?2",
        params![id, now()],
        |row| row.get(0),
    )?;
    let mut query = conn.prepare("SELECT ordinal,error FROM jobs WHERE run_id=?1 AND state='failed' ORDER BY ordinal LIMIT 10")?;
    result.errors = query
        .query_map([&id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(result)
}

struct Job {
    ordinal: usize,
    candidate: Candidate,
    bits: u8,
    order: String,
}

fn claim(conn: &mut Connection, id: &str, owner: &str, opts: &RunOptions) -> Result<Option<Job>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let columns = "SELECT ordinal,candidate,data_bits,bit_order FROM jobs WHERE run_id=?1 AND ordinal % ?2 = ?3 AND ";
    let expired = format!("{columns}state='running' AND lease_until<=?4 ORDER BY ordinal LIMIT 1");
    let pending = format!("{columns}state='pending' ORDER BY ordinal LIMIT 1");
    let read = |row: &rusqlite::Row<'_>| {
        Ok((
            row.get::<_, usize>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, u8>(2)?,
            row.get::<_, String>(3)?,
        ))
    };
    let row = tx
        .query_row(
            &expired,
            params![id, opts.shards, opts.shard_index, now()],
            read,
        )
        .optional()?;
    let row = if row.is_some() {
        row
    } else {
        tx.query_row(&pending, params![id, opts.shards, opts.shard_index], read)
            .optional()?
    };
    let Some((ordinal, candidate, bits, order)) = row else {
        tx.commit()?;
        return Ok(None);
    };
    let candidate: Candidate = serde_json::from_str(&candidate)?;
    candidate.spec(bits, &order)?;
    tx.execute("UPDATE jobs SET state='running',owner=?3,lease_until=?4,attempts=attempts+1,updated_at=?5,error=NULL WHERE run_id=?1 AND ordinal=?2",
        params![id,ordinal,owner,now()+opts.lease_seconds as i64 * 1000,now()])?;
    tx.commit()?;
    Ok(Some(Job {
        ordinal,
        candidate,
        bits,
        order,
    }))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Summary {
    variant: usize,
    metrics: Metrics,
    model: CostModel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    block16_metrics: Option<Metrics>,
}

fn encode(evaluation: &Evaluation) -> Result<(Vec<u8>, String, Summary)> {
    let bytes = serde_json::to_vec(evaluation)?;
    let hash = sha256(&bytes);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&bytes)?;
    let (variant, metrics) = evaluation.best(&evaluation.model)?;
    Ok((
        encoder.finish()?,
        hash,
        Summary {
            variant,
            metrics,
            model: evaluation.model.clone(),
            block16_metrics: evaluation.block16.as_ref().map(|v| v.metrics.clone()),
        },
    ))
}

fn decode(blob: &[u8], hash: &str, job: &Job) -> Result<Evaluation> {
    let mut bytes = Vec::new();
    GzDecoder::new(blob)
        .take(MAX_RESULT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RESULT || sha256(&bytes) != hash {
        return Err("result payload is oversized or has a checksum mismatch".into());
    }
    let evaluation: Evaluation = serde_json::from_slice(&bytes)?;
    evaluation.verify(job.candidate.spec(job.bits, &job.order)?)?;
    Ok(evaluation)
}

fn heartbeat(
    path: PathBuf,
    id: String,
    ordinal: usize,
    owner: String,
    seconds: u64,
    stop: mpsc::Receiver<()>,
) -> Result<()> {
    let conn = existing(&path, true)?;
    while matches!(
        stop.recv_timeout(Duration::from_millis(seconds * 1000 / 3)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ) {
        if conn.execute("UPDATE jobs SET lease_until=?4,updated_at=?5 WHERE run_id=?1 AND ordinal=?2 AND owner=?3 AND state='running'",
            params![id,ordinal,owner,now()+seconds as i64 * 1000,now()])? != 1 { return Ok(()); }
    }
    Ok(())
}

fn worker(
    path: PathBuf,
    id: String,
    owner: String,
    opts: RunOptions,
    budget: Arc<AtomicUsize>,
) -> Result<usize> {
    let mut conn = existing(&path, true)?;
    let mut completed = 0;
    while take_budget(&budget) {
        let Some(job) = claim(&mut conn, &id, &owner, &opts)? else {
            break;
        };
        let (stop, receiver) = mpsc::channel();
        let (beat_path, beat_id, beat_owner, ordinal, seconds) = (
            path.clone(),
            id.clone(),
            owner.clone(),
            job.ordinal,
            opts.lease_seconds,
        );
        let beat = std::thread::spawn(move || {
            heartbeat(beat_path, beat_id, ordinal, beat_owner, seconds, receiver)
        });
        let started = Instant::now();
        let evaluation = std::panic::catch_unwind(|| {
            Evaluation::generate(job.candidate.spec(job.bits, &job.order).unwrap())
        })
        .unwrap_or_else(|_| Err("evaluator panicked".into()));
        let elapsed = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
        let finished = finish(&mut conn, &id, &owner, &job, evaluation, elapsed);
        drop(stop);
        beat.join().map_err(|_| "heartbeat panicked")??;
        if finished? {
            completed += 1;
        }
    }
    Ok(completed)
}

fn finish(
    conn: &mut Connection,
    id: &str,
    owner: &str,
    job: &Job,
    evaluation: crate::rating::Result<Evaluation>,
    elapsed: i64,
) -> Result<bool> {
    // Compress and rate outside the write transaction; the heartbeat is still
    // active here. The owner check fences results from superseded leases.
    let prepared = match evaluation {
        Ok(evaluation) => Ok(encode(&evaluation)?),
        Err(error) => Err(error),
    };
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let updated = match prepared {
        Ok((blob,hash,summary)) => tx.execute("UPDATE jobs SET state='done',result=?4,result_sha256=?5,summary=?6,elapsed_ms=?7,owner=NULL,lease_until=NULL,updated_at=?8,error=NULL
            WHERE run_id=?1 AND ordinal=?2 AND owner=?3 AND state='running'",
            params![id,job.ordinal,owner,blob,hash,serde_json::to_string(&summary)?,elapsed,now()])?,
        Err(error) => tx.execute("UPDATE jobs SET state='failed',error=?4,elapsed_ms=?5,owner=NULL,lease_until=NULL,updated_at=?6
            WHERE run_id=?1 AND ordinal=?2 AND owner=?3 AND state='running'",params![id,job.ordinal,owner,error,elapsed,now()])?,
    };
    if updated != 1 {
        return Ok(false);
    }
    tx.commit()?;
    Ok(true)
}

fn take_budget(budget: &AtomicUsize) -> bool {
    loop {
        let remaining = budget.load(Ordering::Relaxed);
        if remaining == 0 {
            return false;
        }
        if budget
            .compare_exchange_weak(
                remaining,
                remaining - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            return true;
        }
    }
}

pub fn run(path: &Path, manifest: &Manifest, opts: &RunOptions) -> Result<Status> {
    if opts.workers == 0
        || opts.shards == 0
        || opts.shard_index >= opts.shards
        || !(1..=86400).contains(&opts.lease_seconds)
    {
        return Err("invalid worker count, shard, or lease duration".into());
    }
    let id = prepare(path, manifest)?;
    if opts.retry_failed {
        let conn = existing(path, true)?;
        conn.execute(
            "UPDATE jobs SET state='pending',updated_at=?2 WHERE run_id=?1 AND state='failed'",
            params![id, now()],
        )?;
    }
    let budget = Arc::new(AtomicUsize::new(opts.max_jobs.unwrap_or(usize::MAX)));
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let mut workers = Vec::new();
    for i in 0..opts.workers {
        let (path, id, opts, budget) =
            (path.to_path_buf(), id.clone(), opts.clone(), budget.clone());
        let owner = format!("{}:{epoch}:{i}", std::process::id());
        workers.push(std::thread::spawn(move || {
            worker(path, id, owner, opts, budget)
        }));
    }
    // Join every thread even after an error so none outlive the reported outcome.
    let mut first_error = None;
    for worker in workers {
        let result = worker
            .join()
            .map_err(|_| "search worker panicked".into())
            .and_then(|result| result);
        if let Err(error) = result {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    status(path, Some(&id))
}

#[derive(Default)]
pub struct RankOptions {
    pub block16: bool,
    pub width: Option<u8>,
    pub bits: Option<u8>,
    pub order: Option<String>,
    pub min_hd: Option<u32>,
    pub min_length: Option<u64>,
    pub limit: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Ranked {
    pub ordinal: usize,
    pub candidate: Candidate,
    pub data_bits: u8,
    pub order: String,
    pub variant: Option<usize>,
    pub method: String,
    pub variant_name: String,
    pub metrics: Metrics,
}

#[derive(Serialize)]
pub struct RankGroup {
    pub method: String,
    pub width: u8,
    pub data_bits: u8,
    pub order: String,
    pub entries: Vec<Ranked>,
}

pub fn rank(
    path: &Path,
    run: Option<&str>,
    model: &CostModel,
    opts: &RankOptions,
) -> Result<Vec<RankGroup>> {
    if opts.limit == 0
        || opts.min_hd.is_some() != opts.min_length.is_some()
        || opts.min_hd.is_some_and(|hd| hd < 3)
    {
        return Err(
            "ranking needs a positive limit and paired --min-hd (>=3) / --min-length arguments"
                .into(),
        );
    }
    let conn = existing(path, false)?;
    let id = choose_run(&conn, run)?;
    let method = if opts.block16 { "block16" } else { "scalar" };
    let mut groups = BTreeMap::<(u8, u8, String), Vec<Ranked>>::new();
    let mut statement = conn.prepare("SELECT ordinal,candidate,data_bits,bit_order,summary FROM jobs WHERE run_id=?1 AND state='done' ORDER BY ordinal")?;
    let mut rows = statement.query([&id])?;
    while let Some(row) = rows.next()? {
        let ordinal = row.get(0)?;
        let candidate: Candidate = serde_json::from_str(&row.get::<_, String>(1)?)?;
        let scalar_bits: u8 = row.get(2)?;
        let bits = if opts.block16 { 128 } else { scalar_bits };
        let order: String = row.get(3)?;
        if opts.width.is_some_and(|v| v != candidate.width)
            || opts.bits.is_some_and(|v| v != bits)
            || opts.order.as_ref().is_some_and(|v| v != &order)
            || opts
                .min_hd
                .is_some_and(|hd| !candidate.meets(hd, opts.min_length.unwrap()))
        {
            continue;
        }
        let summary: Summary = serde_json::from_str(&row.get::<_, String>(4)?)?;
        let (variant, metrics) = if opts.block16 {
            let Some(metrics) = summary.block16_metrics else {
                continue;
            };
            let metrics = if model == &summary.model {
                metrics
            } else {
                load_with(&conn, &id, ordinal)?
                    .block16
                    .ok_or("missing block graph")?
                    .graph
                    .measure(model)?
            };
            (None, metrics)
        } else {
            let (variant, metrics) = if model == &summary.model {
                (summary.variant, summary.metrics)
            } else {
                load_with(&conn, &id, ordinal)?.best(model)?
            };
            (Some(variant), metrics)
        };
        let entries = groups
            .entry((candidate.width, bits, order.clone()))
            .or_default();
        entries.push(Ranked {
            ordinal,
            candidate,
            data_bits: bits,
            order,
            variant,
            method: method.into(),
            variant_name: String::new(),
            metrics,
        });
        entries.sort_by_key(|r| (r.metrics.weighted_cost, r.metrics.critical_path, r.ordinal));
        entries.truncate(opts.limit);
    }
    let mut result = Vec::new();
    for ((width, data_bits, order), mut entries) in groups {
        for entry in &mut entries {
            let evaluation = load_with(&conn, &id, entry.ordinal)?;
            let (variant, metrics, name) = if opts.block16 {
                let block = evaluation.block16.ok_or("missing block graph")?;
                (None, block.graph.measure(model)?, block.name)
            } else {
                let (index, metrics) = evaluation.best(model)?;
                (
                    Some(index),
                    metrics,
                    evaluation.variants[index].name.clone(),
                )
            };
            if variant != entry.variant || metrics != entry.metrics {
                return Err("cached ranking disagrees with verified result".into());
            }
            entry.variant_name = name;
        }
        result.push(RankGroup {
            method: method.into(),
            width,
            data_bits,
            order,
            entries,
        });
    }
    Ok(result)
}

fn load_with(conn: &Connection, run: &str, ordinal: usize) -> Result<Evaluation> {
    let (candidate,bits,order,blob,hash): (String,u8,String,Vec<u8>,String) = conn.query_row(
        "SELECT candidate,data_bits,bit_order,result,result_sha256 FROM jobs WHERE run_id=?1 AND ordinal=?2 AND state='done'",
        params![run,ordinal],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?;
    decode(
        &blob,
        &hash,
        &Job {
            ordinal,
            candidate: serde_json::from_str(&candidate)?,
            bits,
            order,
        },
    )
}

pub fn load(path: &Path, run: Option<&str>, ordinal: usize) -> Result<Evaluation> {
    let conn = existing(path, false)?;
    let id = choose_run(&conn, run)?;
    load_with(&conn, &id, ordinal)
}

/// Audit the complete immutable inventory and every committed result in one
/// consistent read transaction. Running or pending jobs are not called verified.
pub fn verify(path: &Path, run: Option<&str>) -> Result<usize> {
    let (id, manifest) = manifest(path, run)?;
    let conn = existing(path, false)?;
    conn.execute_batch("BEGIN")?;
    let integrity: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(format!("SQLite integrity check failed: {integrity}").into());
    }
    let mut statement=conn.prepare("SELECT ordinal,candidate,data_bits,bit_order,state,summary FROM jobs WHERE run_id=?1 ORDER BY ordinal")?;
    let mut rows = statement.query([&id])?;
    let (mut total, mut verified) = (0, 0);
    while let Some(row) = rows.next()? {
        let ordinal = row.get(0)?;
        let candidate: String = row.get(1)?;
        let bits = row.get(2)?;
        let order: String = row.get(3)?;
        let state: String = row.get(4)?;
        manifest.check_job(ordinal, &candidate, bits, &order)?;
        total += 1;
        if state == "done" {
            let result = load_with(&conn, &id, ordinal)?;
            let summary: Summary = serde_json::from_str(&row.get::<_, String>(5)?)?;
            let (variant, metrics) = result.best(&result.model)?;
            if variant != summary.variant
                || metrics != summary.metrics
                || summary.model != result.model
                || summary.block16_metrics != result.block16.as_ref().map(|v| v.metrics.clone())
            {
                return Err("cached summary disagrees with result".into());
            }
            verified += 1;
        }
    }
    if total != manifest.total() {
        return Err("job inventory is incomplete".into());
    }
    Ok(verified)
}

pub fn snapshot(path: &Path, output: &Path) -> Result<()> {
    let conn = existing(path, false)?;
    conn.execute(
        "VACUUM INTO ?1",
        [output.to_str().ok_or("snapshot path must be UTF-8")?],
    )?;
    Ok(())
}

/// Merge independently computed shards without replacing conflicting results.
/// In-flight leases are never transferred to another machine.
pub fn merge(path: &Path, source: &Path) -> Result<usize> {
    if path.canonicalize().ok() == source.canonicalize().ok() && path.exists() {
        return Err("cannot merge a database into itself".into());
    }
    let source = existing(source, false)?;
    source.execute_batch("BEGIN")?;
    let mut dest = initialize(path)?;
    let tx = dest.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let active: usize = tx.query_row(
        "SELECT count(*) FROM jobs WHERE state='running' AND lease_until>?1",
        [now()],
        |row| row.get(0),
    )?;
    if active != 0 {
        return Err("stop destination workers and wait for their leases before merging".into());
    }
    let mut manifests = BTreeMap::new();
    let mut runs = source.prepare("SELECT id,manifest,created_at FROM runs ORDER BY id")?;
    for row in runs.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })? {
        let (id, json, created) = row?;
        let manifest: Manifest = serde_json::from_str(&json)?;
        manifest.validate()?;
        if sha256(json.as_bytes()) != id {
            return Err("source manifest checksum mismatch".into());
        }
        let previous: Option<String> = tx
            .query_row("SELECT manifest FROM runs WHERE id=?1", [&id], |row| {
                row.get(0)
            })
            .optional()?;
        if previous.is_some_and(|p| p != json) {
            return Err("manifest conflict".into());
        }
        tx.execute(
            "INSERT OR IGNORE INTO runs VALUES (?1,?2,?3)",
            params![id, json, created],
        )?;
        manifests.insert(id, manifest);
    }
    let mut jobs=source.prepare("SELECT run_id,ordinal,candidate,data_bits,bit_order,state,attempts,updated_at,elapsed_ms,result,result_sha256,summary,error FROM jobs ORDER BY run_id,ordinal")?;
    let mut rows = jobs.query([])?;
    let mut imported = 0;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let ordinal: usize = row.get(1)?;
        let candidate: String = row.get(2)?;
        let bits: u8 = row.get(3)?;
        let order: String = row.get(4)?;
        manifests
            .get(&id)
            .ok_or("job has no source manifest")?
            .check_job(ordinal, &candidate, bits, &order)?;
        let source_state: String = row.get(5)?;
        let attempts: usize = row.get(6)?;
        let updated: i64 = row.get(7)?;
        let elapsed: Option<i64> = row.get(8)?;
        let blob: Option<Vec<u8>> = row.get(9)?;
        let hash: Option<String> = row.get(10)?;
        let summary: Option<String> = row.get(11)?;
        let error: Option<String> = row.get(12)?;
        let state = if source_state == "running" {
            "pending"
        } else {
            &source_state
        };
        let job = Job {
            ordinal,
            candidate: serde_json::from_str(&candidate)?,
            bits,
            order: order.clone(),
        };
        job.candidate.spec(bits, &order)?;
        if state == "done" {
            let evaluation = decode(
                blob.as_deref().ok_or("missing result")?,
                hash.as_deref().ok_or("missing digest")?,
                &job,
            )?;
            let (variant, metrics) = evaluation.best(&evaluation.model)?;
            let cached: Summary =
                serde_json::from_str(summary.as_deref().ok_or("missing summary")?)?;
            if cached.variant != variant
                || cached.metrics != metrics
                || cached.model != evaluation.model
            {
                return Err("source summary mismatch".into());
            }
        }
        let previous:Option<(String,String,u8,String,Option<String>)>=tx.query_row(
            "SELECT state,candidate,data_bits,bit_order,result_sha256 FROM jobs WHERE run_id=?1 AND ordinal=?2",params![id,ordinal],
            |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
        if let Some((old_state, old_candidate, old_bits, old_order, old_hash)) = previous {
            if old_candidate != candidate || old_bits != bits || old_order != order {
                return Err("conflicting job identity".into());
            }
            if old_state == "done" {
                if state == "done" && old_hash != hash {
                    return Err("conflicting completed results".into());
                }
                continue;
            }
            if state != "done" {
                continue;
            }
            tx.execute(
                "DELETE FROM jobs WHERE run_id=?1 AND ordinal=?2",
                params![id, ordinal],
            )?;
        }
        tx.execute("INSERT INTO jobs (run_id,ordinal,candidate,data_bits,bit_order,state,attempts,updated_at,elapsed_ms,result,result_sha256,summary,error)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![id,ordinal,candidate,bits,order,state,attempts,updated,elapsed,blob,hash,summary,error])?;
        if state == "done" {
            imported += 1;
        }
    }
    // A partial inventory cannot be mistaken for a complete exhaustive run.
    let mut check = tx.prepare("SELECT id,manifest FROM runs")?;
    for row in check.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (id, json) = row?;
        let manifest: Manifest = serde_json::from_str(&json)?;
        let count: usize =
            tx.query_row("SELECT count(*) FROM jobs WHERE run_id=?1", [&id], |row| {
                row.get(0)
            })?;
        if count != manifest.total() {
            return Err("source job inventory is incomplete".into());
        }
    }
    drop(check);
    tx.commit()?;
    Ok(imported)
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    #[test]
    fn heartbeat_and_stale_completion_fencing() {
        let path = std::env::temp_dir().join(format!(
            "crc-gen-lease-{}-{}.sqlite",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let manifest = Manifest {
            version: 1,
            engine: engine_fingerprint(),
            optimizer: OPTIMIZER.into(),
            domain: "catalogue".into(),
            include_even: false,
            catalogue: None,
            candidates: vec![Candidate {
                width: 8,
                polynomial: "0x7".into(),
                koopman: None,
                orientation: "test".into(),
                hd: vec![],
                source: None,
                source_sha256: None,
                raw_row: None,
            }],
            data_bits: vec![8],
            orders: vec!["msb".into()],
        };
        let id = prepare(&path, &manifest).unwrap();
        let mut conn = existing(&path, true).unwrap();
        let opts = RunOptions {
            lease_seconds: 1,
            ..RunOptions::default()
        };
        let first = claim(&mut conn, &id, "first", &opts).unwrap().unwrap();
        let started = now();
        let (stop, receiver) = mpsc::channel();
        let (p, r) = (path.clone(), id.clone());
        let beat = std::thread::spawn(move || heartbeat(p, r, 0, "first".into(), 1, receiver));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let stamp: i64 = conn
                .query_row("SELECT updated_at FROM jobs", [], |row| row.get(0))
                .unwrap();
            if stamp > started {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "heartbeat failed to renew the lease"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(claim(&mut conn, &id, "second", &opts).unwrap().is_none());
        drop(stop);
        beat.join().unwrap().unwrap();
        conn.execute("UPDATE jobs SET lease_until=0", []).unwrap();
        let second = claim(&mut conn, &id, "second", &opts).unwrap().unwrap();
        let evaluation = Evaluation::generate(first.candidate.spec(8, "msb").unwrap()).unwrap();
        assert!(!finish(&mut conn, &id, "first", &first, Ok(evaluation.clone()), 0).unwrap());
        assert_eq!(status(&path, None).unwrap().done, 0);
        finish(&mut conn, &id, "second", &second, Ok(evaluation), 0).unwrap();
        assert_eq!(status(&path, None).unwrap().done, 1);
        drop(conn);
        std::fs::remove_file(&path).unwrap();
    }
}
