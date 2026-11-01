mod common;

use crc_gen::{rating::*, BitOrder, CrcSpec};
use std::process::Command;

#[test]
fn ccitt_rediscovers_a_shared_nibble_fold_in_both_orders() {
    for (order, fold, budget) in [
        (BitOrder::MsbFirst, Op::Shr(4), 12),
        (BitOrder::LsbFirst, Op::Shl(4), 13),
    ] {
        let equations = CrcSpec::new(16, 0x1021, 8, order)
            .unwrap()
            .symbolic()
            .simplify();
        let graph = equations.word_operation_graph();
        let cost = graph.measure(&CostModel::default()).unwrap();
        assert!(cost.weighted_cost <= budget, "{order:?}: {cost:?}");
        assert!(
            cost.weighted_cost
                < equations
                    .masked_word_operation_graph()
                    .measure(&CostModel::default())
                    .unwrap()
                    .weighted_cost
        );
        let shift = graph
            .nodes
            .iter()
            .position(|n| n.op == fold)
            .expect("nibble fold");
        let input = graph.nodes[shift].inputs[0];
        assert!(
            graph
                .nodes
                .iter()
                .any(|n| n.op == Op::Xor && n.inputs.contains(&shift) && n.inputs.contains(&input)),
            "fold must share its input word"
        );
        let evaluation = Evaluation::generate(CrcSpec::new(16, 0x1021, 8, order).unwrap()).unwrap();
        let (best, metrics) = evaluation.best(&CostModel::default()).unwrap();
        assert_eq!(evaluation.variants[best].name, "factored-words");
        assert!(metrics.weighted_cost <= budget);
    }
}

#[test]
fn factoring_is_generic_and_never_worsens_the_unit_model() {
    for width in [1, 4, 8] {
        for polynomial in 0..(1u64 << width) {
            for bits in [1, width] {
                for order in [BitOrder::MsbFirst, BitOrder::LsbFirst] {
                    let spec = CrcSpec::new(width, polynomial, bits, order).unwrap();
                    let equations = spec.symbolic().simplify();
                    let old = equations.masked_word_operation_graph();
                    let graph = equations.word_operation_graph();
                    assert!(
                        graph.measure(&CostModel::default()).unwrap().weighted_cost
                            <= old.measure(&CostModel::default()).unwrap().weighted_cost
                    );
                    for input in 0..64 {
                        assert_eq!(
                            graph.evaluate(1u64 << input, 0).unwrap(),
                            common::reference(spec, 1u64 << input, 0)
                        );
                        assert_eq!(
                            graph.evaluate(0, 1u64 << input).unwrap(),
                            common::reference(spec, 0, 1u64 << input)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn compiled_ccitt_forms_match_hand_equations_exhaustively() {
    let dir = std::env::temp_dir().join(format!("crc-gen-word-regression-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let msb = CrcSpec::new(16, 0x1021, 8, BitOrder::MsbFirst)
        .unwrap()
        .symbolic()
        .simplify()
        .word_rust();
    let lsb = CrcSpec::new(16, 0x1021, 8, BitOrder::LsbFirst)
        .unwrap()
        .symbolic()
        .simplify()
        .word_rust();
    let source = format!("mod msb {{ {msb} }}\nmod lsb {{ {lsb} }}\n")
        + r#"
fn main() {
    for crc in 0..=u16::MAX {
        for byte in 0..=u8::MAX {
            let x=((crc >> 8) as u8)^byte;
            let x=x^(x>>4);
            let expected_msb=(crc<<8)^((x as u16)<<12)^((x as u16)<<5)^x as u16;
            let y=(crc as u8)^byte;
            let y=y^(y<<4);
            let expected_lsb=(((y as u16)<<8)|(crc>>8))^(y>>4) as u16^((y as u16)<<3);
            assert_eq!(msb::crc_update(crc as u64,byte as u64),expected_msb as u64);
            assert_eq!(lsb::crc_update(crc as u64,byte as u64),expected_lsb as u64);
            assert_eq!(msb::crc_update(crc as u64|0xfedcba9876540000,byte as u64|0x9876543210abcd00),expected_msb as u64);
            assert_eq!(lsb::crc_update(crc as u64|0xfedcba9876540000,byte as u64|0x9876543210abcd00),expected_lsb as u64);
        }
    }
}
"#;
    let input = dir.join("ccitt.rs");
    let output = dir.join(format!("ccitt{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&input, source).unwrap();
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let built = Command::new(compiler)
        .args(["--edition=2021", "-O", "-Dwarnings"])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let executed = Command::new(&output).output().unwrap();
    assert!(
        executed.status.success(),
        "{}",
        String::from_utf8_lossy(&executed.stderr)
    );
    std::fs::remove_dir_all(dir).unwrap();
}
