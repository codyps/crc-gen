//! Polynomial folding code generation for x86-64 carry-less multiplication.
//!
//! The generator itself remains portable and contains no unsafe code. Generated
//! intrinsic functions are unsafe; the generated dispatcher checks CPU/OS features.
use crate::{CrcSpec, SpecError};

pub use crate::folding::FoldingConstants;

#[derive(Clone, Debug)]
pub struct Pclmul {
    spec: CrcSpec,
    constants: FoldingConstants,
}

impl CrcSpec {
    /// Emit scalar and x86-64 PCLMUL/VPCLMUL streaming updates for byte input.
    /// CRC widths 1..=64 and both bit orders are supported.
    pub fn pclmul(self) -> Result<Pclmul, SpecError> {
        if self.data_bits() != 8 {
            return Err(SpecError("PCLMUL byte streams require --data-bits 8"));
        }
        Ok(Pclmul {
            spec: self,
            constants: FoldingConstants::new(self),
        })
    }
}

impl Pclmul {
    pub fn constants(&self) -> &FoldingConstants {
        &self.constants
    }

    /// Emit standalone Rust, requiring std only for runtime feature detection.
    /// The safe dispatcher compiles on other targets and uses the scalar path.
    pub fn rust(&self) -> String {
        crate::folding::rust(self.spec, &self.constants, include_str!("pclmul.rs.in"))
    }
}
