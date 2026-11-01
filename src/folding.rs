//! Architecture-independent CRC folding constants and code-template rendering.
use crate::{BitOrder, CrcSpec};

/// Constants for normal-order folding using a degree-64 polynomial.
/// For a width w, D(x) = G(x) * x^(64-w). This also handles even/zero G.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FoldingConstants {
    /// Low coefficients of D, omitting its leading x^64.
    pub polynomial: u64,
    /// Low coefficients of floor(x^128 / D), omitting its leading x^64.
    pub reciprocal: u64,
    pub x64: u64,
    pub x128: u64,
    pub x192: u64,
    pub x256: u64,
    pub x320: u64,
    pub x512: u64,
    pub x576: u64,
}

fn power_mod(exponent: usize, polynomial: u64) -> u64 {
    let mut value = 1u64;
    for _ in 0..exponent {
        let carry = value >> 63;
        value <<= 1;
        if carry != 0 {
            value ^= polynomial;
        }
    }
    value
}

impl FoldingConstants {
    pub(crate) fn new(spec: CrcSpec) -> Self {
        let polynomial = spec.polynomial() << (64 - spec.width());
        // Cancel x^128 with D*x^64 first; the rest fits in u128.
        let divisor = (1u128 << 64) | u128::from(polynomial);
        let mut remainder = u128::from(polynomial) << 64;
        let mut reciprocal = 0u64;
        for bit in (64..128).rev() {
            if remainder & (1u128 << bit) != 0 {
                remainder ^= divisor << (bit - 64);
                reciprocal ^= 1u64 << (bit - 64);
            }
        }
        Self {
            polynomial,
            reciprocal,
            x64: power_mod(64, polynomial),
            x128: power_mod(128, polynomial),
            x192: power_mod(192, polynomial),
            x256: power_mod(256, polynomial),
            x320: power_mod(320, polynomial),
            x512: power_mod(512, polynomial),
            x576: power_mod(576, polynomial),
        }
    }
}

pub(crate) fn rust(spec: CrcSpec, constants: &FoldingConstants, template: &str) -> String {
    let c = constants;
    let mut code = spec.symbolic().simplify().word_rust();
    let values = [
        ("WIDTH", u64::from(spec.width())),
        ("POLYNOMIAL", c.polynomial),
        ("RECIPROCAL", c.reciprocal),
        ("X64", c.x64),
        ("X128", c.x128),
        ("X192", c.x192),
        ("X256", c.x256),
        ("X320", c.x320),
        ("X512", c.x512),
        ("X576", c.x576),
    ];
    let mut template = template.replace(
        "@REFLECTED@",
        if spec.order() == BitOrder::LsbFirst {
            "true"
        } else {
            "false"
        },
    );
    for (name, value) in values {
        template = template.replace(&format!("@{name}@"), &value.to_string());
    }
    code.push_str(&template);
    code
}
