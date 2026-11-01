use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "crc-gen-cli-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn command(db: &Path, verb: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_crc-gen"));
    command.args(["search", verb, "--db"]).arg(db);
    command
}
fn json(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cli_resume_rank_and_export() {
    let scratch = Scratch::new();
    let db = scratch.0.join("results.sqlite");
    let first = json(
        command(&db, "run")
            .args([
                "--exhaustive",
                "--width",
                "8",
                "--orders",
                "msb",
                "--workers",
                "2",
                "--max-jobs",
                "2",
            ])
            .output()
            .unwrap(),
    );
    let id = first["run_id"].as_str().unwrap();
    assert_eq!(first["done"], 2);
    let next = json(
        command(&db, "run")
            .args(["--resume", id, "--max-jobs", "3", "--workers", "3"])
            .output()
            .unwrap(),
    );
    assert_eq!(next["done"], 5);
    let ranked = json(
        command(&db, "rank")
            .args(["--limit", "1"])
            .output()
            .unwrap(),
    );
    assert_eq!(ranked["groups"][0]["entries"].as_array().unwrap().len(), 1);
    let exported = command(&db, "export")
        .args(["--job", "0"])
        .output()
        .unwrap();
    assert!(exported.status.success());
    assert!(String::from_utf8_lossy(&exported.stdout).contains("pub fn crc_update"));
    let verified = command(&db, "verify").output().unwrap();
    assert!(verified.status.success());
    assert!(String::from_utf8_lossy(&verified.stdout).contains("Verified 5"));
}

#[test]
fn process_kill_preserves_committed_results_and_resume_recovers() {
    let scratch = Scratch::new();
    let db = scratch.0.join("crash.sqlite");
    let prepared = json(
        command(&db, "run")
            .args([
                "--exhaustive",
                "--width",
                "8",
                "--data-bits",
                "64",
                "--orders",
                "msb",
                "--max-jobs",
                "0",
            ])
            .output()
            .unwrap(),
    );
    let id = prepared["run_id"].as_str().unwrap();
    let mut child = command(&db, "run")
        .args(["--resume", id, "--workers", "2", "--lease-seconds", "1"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let conn = Connection::open(&db).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let (done, running): (usize, usize) = conn
            .query_row(
                "SELECT sum(state='done'),sum(state='running') FROM jobs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        if done > 0 && running > 0 {
            break;
        }
        if let Some(exit) = child.try_wait().unwrap() {
            panic!("search exited before interruption: {exit}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "search did not commit within deadline"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let saved: Vec<(usize, String)> = conn
        .prepare("SELECT ordinal,result_sha256 FROM jobs WHERE state='done'")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(!saved.is_empty());
    std::thread::sleep(std::time::Duration::from_secs(2));
    let resumed = json(
        command(&db, "run")
            .args(["--resume", id, "--workers", "3", "--lease-seconds", "1"])
            .output()
            .unwrap(),
    );
    assert_eq!(resumed["done"], 128);
    assert_eq!(resumed["running"], 0);
    assert_eq!(resumed["failed"], 0);
    for (ordinal, hash) in saved {
        let (attempts, current): (usize, String) = conn
            .query_row(
                "SELECT attempts,result_sha256 FROM jobs WHERE ordinal=?1",
                [ordinal],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(attempts, 1);
        assert_eq!(current, hash);
    }
    assert!(command(&db, "verify").output().unwrap().status.success());
}
