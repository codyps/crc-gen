use crc_gen::{BitOrder, CrcSpec};

mod common;
use common::{mask, reference};

#[test]
fn validate_specification() {
    for width in [0, 65, 255] {
        assert!(CrcSpec::new(width, 1, 8, BitOrder::MsbFirst).is_err());
    }
    for bits in [0, 65, 255] {
        assert!(CrcSpec::new(16, 1, bits, BitOrder::MsbFirst).is_err());
    }
    assert!(CrcSpec::new(16, 0x11021, 8, BitOrder::MsbFirst).is_err());
    assert!(CrcSpec::new(64, u64::MAX, 64, BitOrder::LsbFirst).is_ok());
}

#[test]
fn basis_vectors_prove_linear_equivalence() {
    for width in [1, 4, 8, 16, 32, 64] {
        for bits in [1, 8, 64] {
            for order in [BitOrder::MsbFirst, BitOrder::LsbFirst] {
                for polynomial in [0, 1, 0x42f0_e1eb_a9ea_3693 & mask(width)] {
                    let spec = CrcSpec::new(width, polynomial, bits, order).unwrap();
                    let equations = spec.symbolic().simplify();
                    let program = equations.factor();
                    assert!(program.xor_count() <= equations.xor_count());
                    let check = |crc, data| {
                        let expected = reference(spec, crc, data);
                        assert_eq!(
                            equations.evaluate(crc, data),
                            expected,
                            "{spec:?}, {crc:#x}, {data:#x}"
                        );
                        assert_eq!(
                            program.evaluate(crc, data),
                            expected,
                            "{spec:?}, {crc:#x}, {data:#x}"
                        );
                    };
                    // A linear map is uniquely determined by zero and its basis vectors.
                    check(0, 0);
                    for i in 0..width {
                        check(1u64 << i, 0);
                    }
                    for i in 0..bits {
                        check(0, 1u64 << i);
                    }
                    check(u64::MAX, u64::MAX); // Including ignored high input bits.
                }
            }
        }
    }
}

#[test]
fn exhaustive_small_registers() {
    for polynomial in 0..16 {
        for order in [BitOrder::MsbFirst, BitOrder::LsbFirst] {
            let spec = CrcSpec::new(4, polynomial, 4, order).unwrap();
            let equations = spec.symbolic().simplify();
            let program = equations.factor();
            for crc in 0..16 {
                for data in 0..16 {
                    let expected = reference(spec, crc, data);
                    assert_eq!(equations.evaluate(crc, data), expected);
                    assert_eq!(program.evaluate(crc, data), expected);
                }
            }
        }
    }
}

#[test]
fn known_crc_check_values() {
    let cases = [
        (8, 0x07, BitOrder::MsbFirst, 0, 0, 0xf4),
        (16, 0x1021, BitOrder::MsbFirst, 0, 0, 0x31c3),
        (16, 0x1021, BitOrder::MsbFirst, 0xffff, 0, 0x29b1),
        (16, 0x1021, BitOrder::MsbFirst, 0x1d0f, 0, 0xe5cc),
        (16, 0x1021, BitOrder::LsbFirst, 0, 0, 0x2189),
        (16, 0x1021, BitOrder::LsbFirst, 0xffff, 0xffff, 0x906e),
        (
            32,
            0x04c11db7,
            BitOrder::LsbFirst,
            0xffffffff,
            0xffffffff,
            0xcbf43926,
        ),
        (
            64,
            0x42f0e1eba9ea3693,
            BitOrder::MsbFirst,
            0,
            0,
            0x6c40df5f0b497347,
        ),
    ];
    for (width, poly, order, init, xorout, expected) in cases {
        let spec = CrcSpec::new(width, poly, 8, order).unwrap();
        let program = spec.symbolic().simplify().factor();
        let crc = b"123456789"
            .iter()
            .fold(init, |crc, byte| program.evaluate(crc, (*byte).into()));
        assert_eq!(crc ^ xorout, expected, "{spec:?}");
    }
}

#[test]
fn ccitt_factoring_saves_operations() {
    for order in [BitOrder::MsbFirst, BitOrder::LsbFirst] {
        let symbolic = CrcSpec::new(16, 0x1021, 8, order).unwrap().symbolic();
        let equations = symbolic.simplify();
        let program = equations.factor();
        assert!(program.xor_count() < equations.xor_count());
        assert!(!program.temporaries().is_empty());
        assert_eq!(program.equations(), equations.factor().equations());
        assert!(symbolic.raw_equations().contains("out[15] = e"));
    }
}
