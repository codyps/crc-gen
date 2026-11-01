//! Table-free block updates using independent register-sized linear transforms.

use crate::rating::{linear_word_graph, reference, Graph};
use crate::{mask, BitOrder, CrcSpec, SpecError};
use std::fmt::Write;

/// A 16-byte update decomposed into independent word contributions.
/// Graph k advances a register by k+1 register-sized zero words. These are
/// computed once by the generator; generated updates contain no lookup tables.
#[derive(Clone, Debug)]
pub struct Block16 {
    spec: CrcSpec,
    lanes: Vec<Graph>,
}

impl CrcSpec {
    /// Generate an alternate 16-byte update for byte streams and native widths.
    /// The existing byte-at-a-time update remains available for short inputs/tails.
    pub fn block16(self) -> Result<Block16, SpecError> {
        if self.data_bits() != 8 {
            return Err(SpecError("16-byte blocks require --data-bits 8"));
        }
        if !matches!(self.width(), 8 | 16 | 32 | 64) {
            return Err(SpecError(
                "16-byte blocks require CRC width 8, 16, 32 or 64",
            ));
        }
        let word_spec = CrcSpec::new(self.width(), self.polynomial(), self.width(), self.order())?;
        let mut columns: Vec<_> = (0..self.width()).map(|bit| 1u64 << bit).collect();
        let mut lanes = Vec::new();
        for _ in 0..128 / self.width() {
            for column in &mut columns {
                *column = reference(word_spec, *column, 0);
            }
            let graph = linear_word_graph(self.width(), self.width(), &columns);
            // All supported operations are affine; basis-vector verification
            // proves the complete map, including ignored high input bits.
            assert_eq!(graph.evaluate(0, 0).expect("linear graph"), 0);
            for bit in 0..64 {
                let expected = columns.get(bit).copied().unwrap_or(0);
                assert_eq!(
                    graph.evaluate(0, 1u64 << bit).expect("linear graph"),
                    expected
                );
            }
            lanes.push(graph);
        }
        Ok(Block16 { spec: self, lanes })
    }
}

impl Block16 {
    /// Advance matrices in increasing powers, for inspection and word-op ratings.
    pub fn lane_graphs(&self) -> &[Graph] {
        &self.lanes
    }

    /// Evaluate the block's linear maps without compiling generated Rust.
    pub fn evaluate(&self, crc: u64, block: &[u8; 16]) -> u64 {
        let bytes = usize::from(self.spec.width() / 8);
        let mut output = 0;
        for (i, chunk) in block.chunks_exact(bytes).enumerate() {
            let mut word = 0;
            for (j, byte) in chunk.iter().enumerate() {
                let shift = match self.spec.order() {
                    BitOrder::LsbFirst => j * 8,
                    BitOrder::MsbFirst => (bytes - 1 - j) * 8,
                };
                word |= u64::from(*byte) << shift;
            }
            if i == 0 {
                word ^= crc & mask(self.spec.width());
            }
            output ^= self.lanes[self.lanes.len() - 1 - i]
                .evaluate(0, word)
                .expect("generated lane graph");
        }
        output
    }

    /// Emit byte, fixed 16-byte, and arbitrary-slice streaming updates.
    /// Initialization, final reflection and final XOR remain the caller's job.
    pub fn rust(&self) -> String {
        let width = self.spec.width();
        let bytes = usize::from(width / 8);
        let mut code = self.spec.symbolic().simplify().word_rust();
        for (i, graph) in self.lanes.iter().enumerate() {
            let lane = graph.rust().expect("generated lane graph").replace(
                "pub fn crc_update(crc: u64, data: u64)",
                &format!("fn crc_block_lane_{i}(crc: u64, data: u64)"),
            );
            writeln!(code, "\n#[inline(always)]\n{lane}").unwrap();
        }
        code.push_str("\n/// Update the raw register with exactly 16 bytes, without tables.\n");
        code.push_str("pub fn crc_update_16(crc: u64, block: &[u8; 16]) -> u64 {\n");
        for i in 0..self.lanes.len() {
            let mut word = if width == 8 {
                format!("block[{i}] as u64")
            } else {
                let endian = match self.spec.order() {
                    BitOrder::MsbFirst => "be",
                    BitOrder::LsbFirst => "le",
                };
                let indexes = (i * bytes..(i + 1) * bytes)
                    .map(|j| format!("block[{j}]"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("u{width}::from_{endian}_bytes([{indexes}]) as u64")
            };
            if i == 0 {
                word = format!("({word}) ^ (crc & {}u64)", mask(width));
            }
            let lane = self.lanes.len() - 1 - i;
            writeln!(code, "    let p{i} = crc_block_lane_{lane}(0, {word});").unwrap();
        }
        // Explicit balanced XOR tree; each word contribution can run independently.
        let mut values: Vec<_> = (0..self.lanes.len()).map(|i| format!("p{i}")).collect();
        while values.len() > 1 {
            values = values
                .chunks(2)
                .map(|pair| format!("({} ^ {})", pair[0], pair[1]))
                .collect();
        }
        writeln!(code, "    {}\n}}", &values[0][1..values[0].len() - 1]).unwrap();
        code.push_str(
            "\n/// Update a raw register using 16-byte blocks and byte-at-a-time tails.\n",
        );
        code.push_str("pub fn crc_update_bytes(mut crc: u64, bytes: &[u8]) -> u64 {\n");
        writeln!(code, "    crc &= {}u64;", mask(width)).unwrap();
        code.push_str("    let mut blocks = bytes.chunks_exact(16);\n    for block in &mut blocks {\n        crc = crc_update_16(crc, block.try_into().unwrap());\n    }\n    for &byte in blocks.remainder() {\n        crc = crc_update(crc, byte as u64);\n    }\n    crc\n}\n");
        code
    }
}
