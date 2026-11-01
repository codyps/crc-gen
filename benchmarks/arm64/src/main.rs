use std::{
    error::Error,
    hint::black_box,
    io::{self, Write},
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

struct Method {
    name: &'static str,
    checksum: fn(&[u8]) -> u64,
}
struct Case {
    name: &'static str,
    width: u8,
    polynomial: u64,
    reflected: bool,
    check: Option<u64>,
    methods: Vec<Method>,
}
// Generated DAG code intentionally retains explicit intermediates and casts.
#[allow(clippy::all)]
mod generated {
    use super::{Case, Method};
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}
use generated::{hardware_available, make_cases};

fn command(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut check_only = false;
    let mut metadata_path = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check_only = true,
            "--metadata" => {
                metadata_path = Some(PathBuf::from(
                    args.next().ok_or("--metadata requires a path")?,
                ))
            }
            _ => {
                return Err(format!(
                "unknown argument {arg}; usage: crc-gen-arm64-bench [--check] [--metadata PATH]"
            )
                .into())
            }
        }
    }
    let cases = make_cases();
    let mut rng = 0x9e3779b97f4a7c15u64;
    let buffer: Vec<u8> = (0..65536)
        .map(|_| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng as u8
        })
        .collect();
    let mut checked_lengths = 0;
    for len in (0..=256).chain([
        511, 512, 513, 1023, 1024, 1025, 4095, 4096, 4097, 65535, 65536,
    ]) {
        let bytes = &buffer[..len];
        for case in &cases {
            let reference = (case
                .methods
                .iter()
                .find(|m| m.name == "table1")
                .unwrap()
                .checksum)(bytes);
            for method in &case.methods {
                assert_eq!(
                    (method.checksum)(bytes),
                    reference,
                    "{} {} len={len}",
                    case.name,
                    method.name
                );
            }
        }
        checked_lengths += 1;
    }
    for case in &cases {
        if let Some(check) = case.check {
            for method in &case.methods {
                assert_eq!(
                    (method.checksum)(b"123456789"),
                    check,
                    "{} {} check value",
                    case.name,
                    method.name
                );
            }
        }
    }
    let implementation_count: usize = cases.iter().map(|c| c.methods.len()).sum();
    eprintln!("Verified {} CRC configurations, {implementation_count} implementations, {checked_lengths} input lengths; PMULL={}",cases.len(),hardware_available());
    if check_only {
        return Ok(());
    }
    if !cfg!(all(target_arch = "aarch64", target_endian = "little")) || !hardware_available() {
        return Err("timing requires ARM64 LE with PMULL/AES/NEON; use --check for portable correctness checks".into());
    }
    let uname = command("uname", &["-m"]);
    if uname != "aarch64" && uname != "arm64" {
        return Err(format!(
            "refusing native timing: uname reports {uname:?}; emulators should use --check"
        )
        .into());
    }
    let functions: Vec<_> = cases
        .iter()
        .flat_map(|c| c.methods.iter().map(move |m| (c, m)))
        .collect();
    let mut output = io::BufWriter::new(io::stdout().lock());
    writeln!(
        output,
        "algorithm,method,input_bytes,median_ns,p25_ns,p75_ns"
    )?;
    let mut measurements = Vec::new();
    for len in [0, 8, 16, 64, 512, 4096, 65536] {
        let bytes = &buffer[..len];
        let mut samples = vec![Vec::new(); functions.len()];
        for round in 0..9 {
            for offset in 0..functions.len() {
                let idx = (offset + round * 17) % functions.len();
                let checksum = functions[idx].1.checksum;
                let start = Instant::now();
                let mut count = 0u64;
                while start.elapsed() < Duration::from_millis(20) {
                    for _ in 0..128 {
                        black_box(checksum(black_box(bytes)));
                    }
                    count += 128;
                }
                samples[idx].push(start.elapsed().as_nanos() as f64 / count as f64);
            }
        }
        for (idx, (case, method)) in functions.iter().enumerate() {
            let raw = samples[idx].clone();
            samples[idx].sort_by(f64::total_cmp);
            let (p25, median, p75) = (samples[idx][2], samples[idx][4], samples[idx][6]);
            writeln!(
                output,
                "{},{},{len},{median:.3},{p25:.3},{p75:.3}",
                case.name, method.name
            )?;
            measurements.push(serde_json::json!({"algorithm":case.name,"method":method.name,"input_bytes":len,"median_ns":median,"p25_ns":p25,"p75_ns":p75,"rounds_ns":raw}));
        }
        output.flush()?;
        eprintln!("Completed {len}-byte inputs");
    }
    if let Some(path) = metadata_path {
        let metadata = serde_json::json!({
            "architecture":std::env::consts::ARCH,"pmull_available":hardware_available(),"uname":uname,
            "compiler":env!("CRC_GEN_BENCH_RUSTC_VERSION"),"lscpu":command("lscpu",&[]),
            "cpuinfo":std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default(),
            "affinity":std::fs::read_to_string("/proc/self/status").unwrap_or_default().lines().filter(|l|l.starts_with("Cpus_allowed_list:")).collect::<Vec<_>>(),
            "commit":option_env!("CRC_GEN_BENCH_COMMIT"),"github_run_id":std::env::var("GITHUB_RUN_ID").ok(),"runner_arch":std::env::var("RUNNER_ARCH").ok(),
            "profile":"release opt-level=3, codegen-units=1, no LTO; no global CPU-specific flags; hardware features isolated to generated functions",
            "measurement":"median of nine interleaved 20-ms rounds; 128 calls per timer check; hot buffers/tables; black_box inputs/outputs; fixed per-algorithm initialization; function-pointer calls",
            "validation":{"configurations":cases.len(),"implementations":implementation_count,"input_lengths":checked_lengths,"known_check_values":true},
            "limits":"Userspace whole-buffer timings on the recorded ARM machine; kernel SIMD-context costs, cold caches and dedicated CRC32 instructions are not measured. Direct PMULL uses scalar below 16 bytes. Scalar DAG operation counts do not model instruction throughput.",
            "cases":cases.iter().map(|c|serde_json::json!({"name":c.name,"width":c.width,"polynomial":format!("{:#x}",c.polynomial),"reflected":c.reflected,"check":c.check,"table1_bytes":256*usize::from(c.width)/8,"table16_bytes":4096*usize::from(c.width)/8})).collect::<Vec<_>>(),
            "measurements":measurements
        });
        std::fs::write(path, serde_json::to_vec_pretty(&metadata)?)?;
    }
    Ok(())
}
