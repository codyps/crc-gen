mod common;
use crc_gen::{BitOrder, CrcSpec};
use std::process::Command;

const CASES: &[(u8, u64)] = &[
    (8, 0),
    (8, 0x25),
    (16, 0),
    (16, 7),
    (16, 0x1021),
    (32, 0),
    (32, 0xe5),
    (32, 0x04c11db7),
    (32, 0x1edc6f41),
    (64, 0),
    (64, 1),
    (64, 0xf1),
    (64, 0x42f0e1eba9ea3693),
    (64, u64::MAX),
];
fn oracle(spec: CrcSpec, mut crc: u64, bytes: &[u8]) -> u64 {
    crc &= common::mask(spec.width());
    for &byte in bytes {
        crc = common::reference(spec, crc, byte.into());
    }
    crc
}
#[test]
fn block_matrices_match_all_input_basis_vectors() {
    for &(width, poly) in CASES {
        for order in [BitOrder::MsbFirst, BitOrder::LsbFirst] {
            let spec = CrcSpec::new(width, poly, 8, order).unwrap();
            let block = spec.block16().unwrap();
            assert_eq!(block.lane_graphs().len(), 128 / usize::from(width));
            let graph = block.operation_graph();
            graph.validate().unwrap();
            let zero = [0; 16];
            assert_eq!(block.evaluate(0, &zero), 0);
            assert!(graph.evaluate(0, 0).is_err());
            for bit in 0..64 {
                assert_eq!(
                    graph.evaluate_block(1u64 << bit, &zero).unwrap(),
                    oracle(spec, 1u64 << bit, &zero)
                );
                assert_eq!(
                    block.evaluate(1u64 << bit, &zero),
                    oracle(spec, 1u64 << bit, &zero)
                );
            }
            for bit in 0..128 {
                let mut bytes = zero;
                bytes[bit / 8] = 1 << (bit % 8);
                assert_eq!(
                    graph.evaluate_block(0, &bytes).unwrap(),
                    oracle(spec, 0, &bytes)
                );
                assert_eq!(
                    block.evaluate(0, &bytes),
                    oracle(spec, 0, &bytes),
                    "{width} {poly:x} {order:?} bit {bit}"
                );
            }
            let bytes = std::array::from_fn(|i| (i as u8).wrapping_mul(37).wrapping_add(123));
            assert_eq!(
                block.evaluate(u64::MAX, &bytes),
                oracle(spec, u64::MAX, &bytes)
            );
        }
    }
}
#[test]
fn unsupported_block_dimensions_are_explicit_errors() {
    for width in [1, 7, 9, 24, 63] {
        assert!(CrcSpec::new(width, 1, 8, BitOrder::MsbFirst)
            .unwrap()
            .block16()
            .is_err());
    }
    assert!(CrcSpec::new(32, 7, 16, BitOrder::MsbFirst)
        .unwrap()
        .block16()
        .is_err());
}
#[test]
fn compiled_blocks_and_streaming_tails_match_independent_division() {
    let dir = std::env::temp_dir().join(format!("crc-gen-block-regression-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let mut source = String::new();
    let mut cases = Vec::new();
    for (i, &(width, poly)) in CASES.iter().enumerate() {
        for (j, order) in [BitOrder::MsbFirst, BitOrder::LsbFirst]
            .into_iter()
            .enumerate()
        {
            let spec = CrcSpec::new(width, poly, 8, order).unwrap();
            let module = format!("c{i}_{j}");
            source += &format!("mod {module} {{ {} }}\n", spec.block16().unwrap().rust());
            let rated = spec.block16().unwrap().operation_graph().rust().unwrap();
            source += &format!("mod r_{module} {{ {rated} }}\n");
            cases.push(format!("({width}, {poly}u64, {}, {module}::crc_update, {module}::crc_update_16, {module}::crc_update_bytes, r_{module}::crc_update_16)", order == BitOrder::LsbFirst));
        }
    }
    source += r#"
fn reference(width: u8, poly: u64, lsb: bool, mut crc: u64, bytes: &[u8]) -> u64 {
    let mask = u64::MAX >> (64-width);
    crc &= mask;
    for &byte in bytes {
        let mut data = byte as u64;
        if lsb {crc=crc.reverse_bits()>>(64-width); data=data.reverse_bits()>>56;}
        let mut numerator = ((crc as u128)<<8)^((data as u128)<<width);
        let divisor = (1u128<<width)|poly as u128;
        for bit in (u16::from(width)..u16::from(width)+8).rev() {
            if numerator & (1u128<<bit)!=0 {numerator^=divisor<<(bit-u16::from(width));}
        }
        crc=numerator as u64;
        if lsb {crc=crc.reverse_bits()>>(64-width);}
    }
    crc
}
fn main() {
    type Case=(u8,u64,bool,fn(u64,u64)->u64,fn(u64,&[u8;16])->u64,fn(u64,&[u8])->u64,fn(u64,&[u8;16])->u64);
    let cases:&[Case]=&[
"#;
    source += &cases.join(",\n");
    source += r#"];
    let mut rng=0x9e3779b97f4a7c15u64;
    let bytes:Vec<u8>=(0..511).map(|_|{rng^=rng<<13;rng^=rng>>7;rng^=rng<<17;rng as u8}).collect();
    for &(width,poly,lsb,byte,block,stream,rated) in cases {
        for seed in [0,1,u64::MAX,0xfedcba9876543210] {
            assert_eq!(rated(seed,bytes[..16].try_into().unwrap()),reference(width,poly,lsb,seed,&bytes[..16]));
            assert_eq!(block(seed,bytes[..16].try_into().unwrap()),reference(width,poly,lsb,seed,&bytes[..16]));
            for len in (0..=80).chain([127,128,129,255,256,257,511]) {
                let input=&bytes[..len];
                let expected=reference(width,poly,lsb,seed,input);
                assert_eq!(stream(seed,input),expected,"{width} {poly:x} {lsb} len={len}");
                let mut serial=seed & (u64::MAX>>(64-width));
                for &b in input {serial=byte(serial,b as u64);}
                assert_eq!(serial,expected);
                // Splits exercise every tail length and incremental raw seeds.
                for split in 0..=len.min(33) {
                    assert_eq!(stream(stream(seed,&input[..split]),&input[split..]),expected);
                }
            }
        }
    }
}
"#;
    let input = dir.join("block.rs");
    let output = dir.join(format!("block{}", std::env::consts::EXE_SUFFIX));
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
