mod common;

use common::reference;
use crc_gen::{BitOrder, CrcSpec};
use std::fmt::Write;
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "crc-gen-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
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

#[test]
fn generated_rust_compiles_and_matches_polynomial_division() {
    let cases = [
        (1, 1, 64),
        (4, 3, 4),
        (8, 7, 8),
        (16, 0x1021, 8),
        (16, 0x1021, 16),
        (32, 0x04c11db7, 32),
        (64, 0x42f0e1eba9ea3693, 64),
        (64, 0, 64),
    ];
    let mut source = String::new();
    let mut checks = String::new();
    for (i, (width, polynomial, bits)) in cases.into_iter().enumerate() {
        for (j, order) in [BitOrder::MsbFirst, BitOrder::LsbFirst]
            .into_iter()
            .enumerate()
        {
            let spec = CrcSpec::new(width, polynomial, bits, order).unwrap();
            let equations = spec.symbolic().simplify();
            for (kind, code) in [
                ("bits", equations.factor().rust()),
                ("word", equations.word_rust()),
                (
                    "rated_bits",
                    equations.factor().operation_graph().rust().unwrap(),
                ),
                (
                    "rated_word",
                    equations.word_operation_graph().rust().unwrap(),
                ),
            ] {
                let module = format!("case_{i}_{j}_{kind}");
                writeln!(source, "mod {module} {{\n{code}}}").unwrap();
                let mut inputs = vec![(0, 0), (u64::MAX, u64::MAX)];
                for bit in 0..width {
                    inputs.push((1u64 << bit, 0));
                }
                for bit in 0..bits {
                    inputs.push((0, 1u64 << bit));
                }
                // Include mixed assignments as a check of emitted masks, shifts and parentheses.
                let mut rng = 0x9e3779b97f4a7c15u64;
                for _ in 0..24 {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    inputs.push((rng, rng.rotate_left(23)));
                }
                for (crc, data) in inputs {
                    let expected = reference(spec, crc, data);
                    writeln!(checks, "assert_eq!({module}::crc_update({crc}u64, {data}u64), {expected}u64, \"{module}\");").unwrap();
                }
            }
        }
    }
    writeln!(source, "fn main() {{\n{checks}}}").unwrap();
    let scratch = Scratch::new();
    let input = scratch.0.join("generated.rs");
    let output = scratch
        .0
        .join(format!("generated{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&input, source).unwrap();
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compiled = Command::new(compiler)
        .arg("--edition=2021")
        .arg("-Dwarnings")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let executed = Command::new(&output).output().unwrap();
    assert!(
        executed.status.success(),
        "{}",
        String::from_utf8_lossy(&executed.stderr)
    );
}
