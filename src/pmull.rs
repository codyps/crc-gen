//! ARM64 PMULL folding code generation with safe runtime dispatch.
use crate::folding::FoldingConstants;
use crate::{CrcSpec, SpecError};

#[derive(Clone, Debug)]
pub struct Pmull {
    spec: CrcSpec,
    constants: FoldingConstants,
}

impl CrcSpec {
    /// Generate little-endian ARM64 PMULL byte-stream updates, with a portable
    /// scalar fallback. Supports widths 1..=64 and both CRC bit orders.
    pub fn pmull(self) -> Result<Pmull, SpecError> {
        if self.data_bits() != 8 {
            return Err(SpecError("PMULL byte streams require --data-bits 8"));
        }
        Ok(Pmull {
            spec: self,
            constants: FoldingConstants::new(self),
        })
    }
}

impl Pmull {
    pub fn constants(&self) -> &FoldingConstants {
        &self.constants
    }

    /// Emit standalone Rust. Uses std for CPU/OS feature detection; code on
    /// unsupported CPUs, other architectures and big-endian targets is scalar.
    pub fn rust(&self) -> String {
        crate::folding::rust(self.spec, &self.constants, include_str!("pmull.rs.in"))
    }
}
