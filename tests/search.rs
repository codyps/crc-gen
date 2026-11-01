use crc_gen::{catalogue::*, rating::CostModel, search::*};
use rusqlite::{params, Connection};
use std::path::PathBuf;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "crc-gen-search-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn db(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn plan() -> Manifest {
    Manifest {
        version: 1,
        engine: engine_fingerprint(),
        optimizer: crc_gen::rating::OPTIMIZER.into(),
        domain: "catalogue".into(),
        include_even: false,
        catalogue: None,
        candidates: (1..16)
            .step_by(2)
            .map(|poly| Candidate {
                width: 4,
                polynomial: format!("{poly:#x}"),
                koopman: None,
                orientation: "test".into(),
                hd: vec![],
                source: None,
                source_sha256: None,
                raw_row: None,
            })
            .collect(),
        data_bits: vec![4],
        orders: vec!["msb".into(), "lsb".into()],
    }
}

#[test]
fn partial_completion_resume_and_model_rerating_do_not_repeat_work() {
    let scratch = Scratch::new();
    let db = scratch.db("run.sqlite");
    let plan = plan();
    let first = run(
        &db,
        &plan,
        &RunOptions {
            workers: 3,
            max_jobs: Some(3),
            ..RunOptions::default()
        },
    )
    .unwrap();
    assert_eq!(first.done, 3);
    assert_eq!(first.pending, 13);
    let (id, stored) = manifest(&db, None).unwrap();
    assert_eq!(id, plan.id().unwrap());
    let finished = run(
        &db,
        &stored,
        &RunOptions {
            workers: 4,
            ..RunOptions::default()
        },
    )
    .unwrap();
    assert_eq!(finished.done, 16);
    assert_eq!(finished.running, 0);
    assert_eq!(finished.failed, 0);
    let conn = Connection::open(&db).unwrap();
    let attempts: u64 = conn
        .query_row("SELECT sum(attempts) FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(attempts, 16);
    let mut model = CostModel {
        id: "expensive-shifts".into(),
        ..CostModel::default()
    };
    model.costs.shr = 20;
    model.latencies.shr = 3;
    let ranking = rank(
        &db,
        None,
        &model,
        &RankOptions {
            limit: 3,
            ..RankOptions::default()
        },
    )
    .unwrap();
    assert_eq!(ranking.len(), 2);
    assert!(ranking
        .iter()
        .all(|g| g.entries.iter().all(|r| r.metrics.model == model.id)));
    let after: u64 = conn
        .query_row("SELECT sum(attempts) FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, attempts);
    assert_eq!(run(&db, &stored, &RunOptions::default()).unwrap().done, 16);
    let mut incompatible = plan;
    incompatible.engine = "different-engine".into();
    assert!(run(&db, &incompatible, &RunOptions::default()).is_err());
}

#[test]
fn independent_shards_merge_idempotently_and_snapshot() {
    let scratch = Scratch::new();
    let a = scratch.db("a.sqlite");
    let b = scratch.db("b.sqlite");
    let dest = scratch.db("merged.sqlite");
    let plan = plan();
    let opts = RunOptions {
        workers: 2,
        shards: 2,
        ..RunOptions::default()
    };
    assert_eq!(run(&a, &plan, &opts).unwrap().done, 8);
    assert_eq!(
        run(
            &b,
            &plan,
            &RunOptions {
                shard_index: 1,
                ..opts
            }
        )
        .unwrap()
        .done,
        8
    );
    assert_eq!(merge(&dest, &a).unwrap(), 8);
    assert_eq!(merge(&dest, &b).unwrap(), 8);
    assert_eq!(merge(&dest, &a).unwrap(), 0);
    assert_eq!(status(&dest, None).unwrap().done, 16);
    assert_eq!(verify(&dest, None).unwrap(), 16);
    let backup = scratch.db("backup.sqlite");
    snapshot(&dest, &backup).unwrap();
    assert_eq!(status(&backup, None).unwrap().done, 16);
    assert!(snapshot(&dest, &backup).is_err());
    assert_eq!(load(&backup, None, 0).unwrap().polynomial, "0x1");
}

#[test]
fn expired_work_recovers_and_active_leases_are_not_stolen() {
    let scratch = Scratch::new();
    let db = scratch.db("run.sqlite");
    let plan = plan();
    let id = prepare(&db, &plan).unwrap();
    let conn = Connection::open(&db).unwrap();
    conn.execute("UPDATE jobs SET state='running',owner='dead-worker',lease_until=0,attempts=1 WHERE ordinal=0",[]).unwrap();
    conn.execute("UPDATE jobs SET state='running',owner='live-worker',lease_until=9223372036854775807,attempts=1 WHERE ordinal=1",[]).unwrap();
    assert_eq!(status(&db, None).unwrap().expired, 1);
    let result = run(
        &db,
        &plan,
        &RunOptions {
            workers: 3,
            ..RunOptions::default()
        },
    )
    .unwrap();
    assert_eq!(result.done, 15);
    assert_eq!(result.running, 1);
    let owner: String = conn
        .query_row("SELECT owner FROM jobs WHERE ordinal=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(owner, "live-worker");
    conn.execute("UPDATE jobs SET lease_until=0 WHERE ordinal=1", [])
        .unwrap();
    assert_eq!(run(&db, &plan, &RunOptions::default()).unwrap().done, 16);
    let attempts: u64 = conn
        .query_row(
            "SELECT attempts FROM jobs WHERE run_id=?1 AND ordinal=0",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(attempts, 2);
}

#[test]
fn corrupted_results_and_inventories_fail_instead_of_becoming_complete() {
    let scratch = Scratch::new();
    let db = scratch.db("run.sqlite");
    let dest = scratch.db("dest.sqlite");
    let plan = plan();
    run(
        &db,
        &plan,
        &RunOptions {
            max_jobs: Some(1),
            ..RunOptions::default()
        },
    )
    .unwrap();
    let conn = Connection::open(&db).unwrap();
    conn.execute("UPDATE jobs SET result=x'000102' WHERE ordinal=0", [])
        .unwrap();
    assert!(load(&db, None, 0).is_err());
    assert!(verify(&db, None).is_err());
    assert!(merge(&dest, &db).is_err());
    conn.execute("DELETE FROM jobs WHERE ordinal=15", [])
        .unwrap();
    assert!(prepare(&db, &plan).is_err());
    let unrelated = scratch.db("unrelated.sqlite");
    let conn = Connection::open(&unrelated).unwrap();
    conn.execute("CREATE TABLE user_data(x)", []).unwrap();
    assert!(prepare(&unrelated, &plan).is_err());
    let still_there: usize = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='user_data'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(still_there, 1);
}

#[test]
fn multiple_process_equivalent_runners_share_the_queue() {
    let scratch = Scratch::new();
    let db = scratch.db("run.sqlite");
    let plan = plan();
    prepare(&db, &plan).unwrap();
    let (p, q) = (plan.clone(), plan.clone());
    let (a, b) = (db.clone(), db.clone());
    let first = std::thread::spawn(move || {
        run(
            &a,
            &p,
            &RunOptions {
                workers: 2,
                ..RunOptions::default()
            },
        )
    });
    let second = std::thread::spawn(move || {
        run(
            &b,
            &q,
            &RunOptions {
                workers: 2,
                ..RunOptions::default()
            },
        )
    });
    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
    assert_eq!(status(&db, None).unwrap().done, 16);
    let conn = Connection::open(&db).unwrap();
    let repeats: usize = conn
        .query_row("SELECT count(*) FROM jobs WHERE attempts<>1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(repeats, 0);
}

#[test]
fn exhaustive_domains_and_quality_filters_are_conservative() {
    assert_eq!(exhaustive(8, false, None).unwrap().len(), 128);
    assert_eq!(exhaustive(16, false, None).unwrap().len(), 32768);
    assert_eq!(exhaustive(8, true, None).unwrap().len(), 256);
    assert!(exhaustive(32, false, None).is_err());
    let mut candidate = plan().candidates.remove(0);
    candidate.hd = vec![
        HdBound {
            distance: 4,
            max_data_bits: Some("1024".into()),
            status: "lower_bound".into(),
            raw: ">=1024".into(),
        },
        HdBound {
            distance: 5,
            max_data_bits: None,
            status: "unknown".into(),
            raw: "?HD=5?>=2000?".into(),
        },
    ];
    assert!(candidate.meets(4, 1024));
    assert!(!candidate.meets(4, 1025));
    assert!(!candidate.meets(5, 1));
    let scratch = Scratch::new();
    let db = scratch.db("quality.sqlite");
    let mut p = plan();
    p.candidates = vec![candidate];
    run(&db, &p, &RunOptions::default()).unwrap();
    assert_eq!(
        rank(
            &db,
            None,
            &CostModel::default(),
            &RankOptions {
                limit: 1,
                min_hd: Some(5),
                min_length: Some(1),
                ..RankOptions::default()
            }
        )
        .unwrap()
        .len(),
        0
    );
    let conn = Connection::open(&db).unwrap();
    let id = p.id().unwrap();
    conn.execute("UPDATE jobs SET state='failed',result=NULL,result_sha256=NULL,summary=NULL,error='test failure' WHERE run_id=?1 AND ordinal=0",params![id]).unwrap();
    assert_eq!(run(&db, &p, &RunOptions::default()).unwrap().failed, 1);
    assert_eq!(
        run(
            &db,
            &p,
            &RunOptions {
                retry_failed: true,
                ..RunOptions::default()
            }
        )
        .unwrap()
        .failed,
        0
    );
}
