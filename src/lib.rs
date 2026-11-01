#![forbid(unsafe_code)]

//! Symbolic CRC updates over GF(2).
//!
//! The polynomial omits its leading coefficient: use `0x1021`, not `0x11021`,
//! for CRC-16/CCITT. This convention is the same for both bit orders.
//! Input and output bit indices always count from the least significant bit.
//! The generated update accepts an arbitrary register; initialization, final
//! reflection and final XOR are the caller's responsibility.
//!
//! ```
//! use crc_gen::{BitOrder, CrcSpec};
//! let spec = CrcSpec::new(16, 0x1021, 8, BitOrder::MsbFirst)?;
//! let equations = spec.symbolic().simplify();
//! let program = equations.factor();
//! assert_eq!(program.evaluate(0, b'1'.into()), 0x2672);
//! assert!(program.xor_count() <= equations.xor_count());
//! # Ok::<(), crc_gen::SpecError>(())
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write};

pub mod rating;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitOrder {
    MsbFirst,
    LsbFirst,
}

/// A streaming update consuming `data_bits` from the low bits of `data`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CrcSpec {
    width: u8,
    polynomial: u64,
    data_bits: u8,
    order: BitOrder,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpecError(&'static str);

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for SpecError {}

fn mask(bits: u8) -> u64 {
    u64::MAX >> (64 - bits)
}

impl CrcSpec {
    pub fn new(
        width: u8,
        polynomial: u64,
        data_bits: u8,
        order: BitOrder,
    ) -> Result<Self, SpecError> {
        if !(1..=64).contains(&width) {
            return Err(SpecError("CRC width must be between 1 and 64"));
        }
        if !(1..=64).contains(&data_bits) {
            return Err(SpecError("data width must be between 1 and 64"));
        }
        if polynomial & !mask(width) != 0 {
            return Err(SpecError(
                "polynomial does not fit the CRC width; omit its leading coefficient",
            ));
        }
        Ok(Self {
            width,
            polynomial,
            data_bits,
            order,
        })
    }

    pub fn width(self) -> u8 {
        self.width
    }
    pub fn polynomial(self) -> u64 {
        self.polynomial
    }
    pub fn data_bits(self) -> u8 {
        self.data_bits
    }
    pub fn order(self) -> BitOrder {
        self.order
    }

    /// Build a DAG of unsimplified XOR operations, without expanding shared nodes.
    pub fn symbolic(self) -> SymbolicCrc {
        let mut arena = Arena::default();
        let zero = arena.push(Node::Constant(false));
        let mut register: Vec<_> = (0..self.width)
            .map(|i| arena.push(Node::Input(Term::Crc(i))))
            .collect();
        let data: Vec<_> = (0..self.data_bits)
            .map(|i| arena.push(Node::Input(Term::Data(i))))
            .collect();
        let polynomial = match self.order {
            BitOrder::MsbFirst => self.polynomial,
            BitOrder::LsbFirst => self.polynomial.reverse_bits() >> (64 - self.width),
        };
        for step in 0..self.data_bits {
            let (feedback_index, data_index) = match self.order {
                BitOrder::MsbFirst => (self.width - 1, self.data_bits - 1 - step),
                BitOrder::LsbFirst => (0, step),
            };
            let feedback = arena.xor(register[feedback_index as usize], data[data_index as usize]);
            let mut next = Vec::with_capacity(self.width as usize);
            for i in 0..self.width {
                let shifted = match self.order {
                    BitOrder::MsbFirst if i > 0 => register[(i - 1) as usize],
                    BitOrder::LsbFirst if i + 1 < self.width => register[(i + 1) as usize],
                    _ => zero,
                };
                next.push(if polynomial & (1u64 << i) != 0 {
                    arena.xor(shifted, feedback)
                } else {
                    shifted
                });
            }
            register = next;
        }
        SymbolicCrc {
            spec: self,
            arena,
            outputs: register,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Term {
    Crc(u8),
    Data(u8),
    Temporary(usize),
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crc(i) => write!(f, "crc[{i}]"),
            Self::Data(i) => write!(f, "data[{i}]"),
            Self::Temporary(i) => write!(f, "t{i}"),
        }
    }
}

/// A canonical XOR sum: equal terms cancel, and constants fold modulo two.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LinearExpr {
    constant: bool,
    terms: BTreeSet<Term>,
}

impl LinearExpr {
    fn input(term: Term) -> Self {
        Self {
            constant: false,
            terms: BTreeSet::from([term]),
        }
    }

    fn xor_assign(&mut self, other: &Self) {
        self.constant ^= other.constant;
        for term in &other.terms {
            if !self.terms.insert(*term) {
                self.terms.remove(term);
            }
        }
    }

    /// Number of binary XOR operations in this sum (constants included).
    pub fn xor_count(&self) -> usize {
        (self.terms.len() + usize::from(self.constant)).saturating_sub(1)
    }

    fn evaluate(&self, crc: u64, data: u64, temporaries: &[bool]) -> bool {
        self.terms.iter().fold(self.constant, |value, term| {
            value
                ^ match *term {
                    Term::Crc(i) => crc & (1u64 << i) != 0,
                    Term::Data(i) => data & (1u64 << i) != 0,
                    Term::Temporary(i) => temporaries[i],
                }
        })
    }

    fn rust(&self) -> String {
        let mut terms: Vec<_> = self
            .terms
            .iter()
            .map(|term| match term {
                Term::Crc(i) => format!("((crc >> {i}) & 1)"),
                Term::Data(i) => format!("((data >> {i}) & 1)"),
                Term::Temporary(i) => format!("t{i}"),
            })
            .collect();
        if self.constant {
            terms.push("1".into());
        }
        if terms.is_empty() {
            "0".into()
        } else {
            terms.join(" ^ ")
        }
    }
}

impl fmt::Display for LinearExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut separator = "";
        for term in &self.terms {
            write!(f, "{separator}{term}")?;
            separator = " ^ ";
        }
        if self.constant {
            write!(f, "{separator}1")?;
        } else if self.terms.is_empty() {
            f.write_str("0")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
enum Node {
    Constant(bool),
    Input(Term),
    Xor(usize, usize),
}

#[derive(Clone, Debug, Default)]
struct Arena {
    nodes: Vec<Node>,
}

impl Arena {
    fn push(&mut self, node: Node) -> usize {
        let id = self.nodes.len();
        self.nodes.push(node);
        id
    }

    fn xor(&mut self, left: usize, right: usize) -> usize {
        self.push(Node::Xor(left, right))
    }

    fn reachable(&self, outputs: &[usize]) -> Vec<bool> {
        let mut reachable = vec![false; self.nodes.len()];
        let mut pending = outputs.to_vec();
        while let Some(id) = pending.pop() {
            if std::mem::replace(&mut reachable[id], true) {
                continue;
            }
            if let Node::Xor(left, right) = self.nodes[id] {
                pending.extend([left, right]);
            }
        }
        reachable
    }

    fn simplify(&self) -> Vec<LinearExpr> {
        // Nodes refer only to earlier nodes, so this is a memoized topological pass.
        let mut simplified: Vec<LinearExpr> = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let value = match *node {
                Node::Constant(constant) => LinearExpr {
                    constant,
                    ..LinearExpr::default()
                },
                Node::Input(term) => LinearExpr::input(term),
                Node::Xor(left, right) => {
                    let mut value = simplified[left].clone();
                    value.xor_assign(&simplified[right]);
                    value
                }
            };
            simplified.push(value);
        }
        simplified
    }
}

#[derive(Clone, Debug)]
pub struct SymbolicCrc {
    spec: CrcSpec,
    arena: Arena,
    outputs: Vec<usize>,
}

impl SymbolicCrc {
    pub fn simplify(&self) -> Equations {
        let expressions = self.arena.simplify();
        Equations {
            spec: self.spec,
            outputs: self
                .outputs
                .iter()
                .map(|id| expressions[*id].clone())
                .collect(),
        }
    }

    /// Number of reachable XOR nodes, counting a shared subexpression once.
    pub fn xor_count(&self) -> usize {
        self.arena
            .reachable(&self.outputs)
            .into_iter()
            .zip(&self.arena.nodes)
            .filter(|(reachable, node)| *reachable && matches!(node, Node::Xor(_, _)))
            .count()
    }

    /// Print the reachable DAG, rather than exponentially expanding shared expressions.
    pub fn raw_equations(&self) -> String {
        let reachable = self.arena.reachable(&self.outputs);
        let mut result = String::new();
        for (id, node) in self.arena.nodes.iter().enumerate() {
            if !reachable[id] {
                continue;
            }
            match node {
                Node::Constant(value) => writeln!(result, "e{id} = {};", u8::from(*value)),
                Node::Input(term) => writeln!(result, "e{id} = {term};"),
                Node::Xor(left, right) => writeln!(result, "e{id} = e{left} ^ e{right};"),
            }
            .unwrap();
        }
        for (bit, id) in self.outputs.iter().enumerate() {
            writeln!(result, "out[{bit}] = e{id};").unwrap();
        }
        result
    }
}

/// Fully expanded canonical bit equations, before common-subexpression factoring.
#[derive(Clone, Debug)]
pub struct Equations {
    spec: CrcSpec,
    outputs: Vec<LinearExpr>,
}

impl Equations {
    pub fn outputs(&self) -> &[LinearExpr] {
        &self.outputs
    }

    pub fn xor_count(&self) -> usize {
        self.outputs.iter().map(LinearExpr::xor_count).sum()
    }

    pub fn evaluate(&self, crc: u64, data: u64) -> u64 {
        evaluate_outputs(&self.outputs, crc, data, &[])
    }

    pub fn equations(&self) -> String {
        print_equations(&[], &self.outputs)
    }

    /// Greedily extract the most frequent XOR pair into a shared temporary.
    /// Ties are broken lexicographically for reproducible output. Each extraction
    /// strictly reduces the XOR count; global optimality is not guaranteed.
    pub fn factor(&self) -> Program {
        let mut outputs = self.outputs.clone();
        let mut temporaries = Vec::new();
        loop {
            let mut counts = BTreeMap::new();
            for expression in &outputs {
                let terms: Vec<_> = expression.terms.iter().copied().collect();
                for (i, left) in terms.iter().enumerate() {
                    for right in &terms[i + 1..] {
                        *counts.entry((*left, *right)).or_insert(0usize) += 1;
                    }
                }
            }
            let mut best = None;
            let mut best_count = 1;
            for (pair, count) in counts {
                if count > best_count {
                    best = Some(pair);
                    best_count = count;
                }
            }
            let Some((left, right)) = best else {
                break;
            };
            let temporary = Term::Temporary(temporaries.len());
            temporaries.push(LinearExpr {
                constant: false,
                terms: BTreeSet::from([left, right]),
            });
            for expression in &mut outputs {
                if expression.terms.contains(&left) && expression.terms.contains(&right) {
                    expression.terms.remove(&left);
                    expression.terms.remove(&right);
                    expression.terms.insert(temporary);
                }
            }
        }
        Program {
            spec: self.spec,
            temporaries,
            outputs,
        }
    }

    /// Group contributions with equal source-to-output shifts into masked words.
    /// This is an alternative to bit-level factoring, not a claim of minimal code.
    pub fn word_rust(&self) -> String {
        let mut groups = BTreeMap::<(bool, i16), u64>::new();
        let mut constant = 0u64;
        for (output, expression) in self.outputs.iter().enumerate() {
            if expression.constant {
                constant |= 1u64 << output;
            }
            for term in &expression.terms {
                let (data, input) = match *term {
                    Term::Crc(i) => (false, i),
                    Term::Data(i) => (true, i),
                    Term::Temporary(_) => unreachable!("canonical equations have no temporaries"),
                };
                *groups
                    .entry((data, output as i16 - i16::from(input)))
                    .or_default() |= 1u64 << input;
            }
        }
        let mut terms = Vec::new();
        for ((data, shift), mask) in groups {
            let source = if data { "data" } else { "crc" };
            let term = format!("({source} & {mask:#x})");
            terms.push(match shift.cmp(&0) {
                std::cmp::Ordering::Less => format!("({term} >> {})", -shift),
                std::cmp::Ordering::Greater => format!("({term} << {shift})"),
                std::cmp::Ordering::Equal => term,
            });
        }
        if constant != 0 {
            terms.push(format!("{constant:#x}"));
        }
        if terms.is_empty() {
            terms.push("0".into());
        }
        format!(
            "{}    {}\n}}\n",
            rust_header(self.spec),
            terms.join("\n        ^ ")
        )
    }
}

/// Straight-line XOR program. Temporary definitions refer only to earlier ones.
#[derive(Clone, Debug)]
pub struct Program {
    spec: CrcSpec,
    temporaries: Vec<LinearExpr>,
    outputs: Vec<LinearExpr>,
}

impl Program {
    pub fn temporaries(&self) -> &[LinearExpr] {
        &self.temporaries
    }
    pub fn outputs(&self) -> &[LinearExpr] {
        &self.outputs
    }

    pub fn xor_count(&self) -> usize {
        self.temporaries
            .iter()
            .chain(&self.outputs)
            .map(LinearExpr::xor_count)
            .sum()
    }

    pub fn evaluate(&self, crc: u64, data: u64) -> u64 {
        let mut temporaries = Vec::with_capacity(self.temporaries.len());
        for expression in &self.temporaries {
            temporaries.push(expression.evaluate(crc, data, &temporaries));
        }
        evaluate_outputs(&self.outputs, crc, data, &temporaries)
    }

    pub fn equations(&self) -> String {
        print_equations(&self.temporaries, &self.outputs)
    }

    /// Emit a standalone u64 update function; only the specified low input bits
    /// are read and output bits outside the CRC width are always zero.
    pub fn rust(&self) -> String {
        let mut result = rust_header(self.spec);
        for (i, expression) in self.temporaries.iter().enumerate() {
            writeln!(result, "    let t{i} = {};", expression.rust()).unwrap();
        }
        writeln!(result, "    let mut out = 0u64;").unwrap();
        for (i, expression) in self.outputs.iter().enumerate() {
            writeln!(result, "    out |= ({}) << {i};", expression.rust()).unwrap();
        }
        result.push_str("    out\n}\n");
        result
    }
}

fn evaluate_outputs(outputs: &[LinearExpr], crc: u64, data: u64, temporaries: &[bool]) -> u64 {
    outputs
        .iter()
        .enumerate()
        .fold(0, |value, (bit, expression)| {
            value | (u64::from(expression.evaluate(crc, data, temporaries)) << bit)
        })
}

fn print_equations(temporaries: &[LinearExpr], outputs: &[LinearExpr]) -> String {
    let mut result = String::new();
    for (i, expression) in temporaries.iter().enumerate() {
        writeln!(result, "t{i} = {expression};").unwrap();
    }
    for (i, expression) in outputs.iter().enumerate() {
        writeln!(result, "out[{i}] = {expression};").unwrap();
    }
    result
}

fn rust_header(spec: CrcSpec) -> String {
    format!(
        "// width={}, polynomial={:#x}, data_bits={}, order={:?}\n\
         #[allow(unused_variables, unused_parens)]\n\
         pub fn crc_update(crc: u64, data: u64) -> u64 {{\n",
        spec.width, spec.polynomial, spec.data_bits, spec.order
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_cancel_and_fold_constants() {
        let mut arena = Arena::default();
        let x = arena.push(Node::Input(Term::Crc(0)));
        let y = arena.push(Node::Input(Term::Data(0)));
        let zero = arena.push(Node::Constant(false));
        let one = arena.push(Node::Constant(true));
        let xy = arena.xor(x, y);
        let yx = arena.xor(y, x);
        let cancel = arena.xor(xy, yx);
        let identity = arena.xor(xy, zero);
        let constant = arena.xor(one, one);
        let not_x = arena.xor(x, one);
        let values = arena.simplify();
        assert_eq!(values[cancel].to_string(), "0");
        assert_eq!(values[identity], values[xy]);
        assert_eq!(values[constant].to_string(), "0");
        assert_eq!(values[not_x].to_string(), "crc[0] ^ 1");
        assert_eq!(values[not_x].xor_count(), 1);
    }

    #[test]
    fn factoring_reuses_nested_pairs_and_preserves_constants() {
        let mut shared = LinearExpr::input(Term::Crc(0));
        shared.xor_assign(&LinearExpr::input(Term::Crc(1)));
        shared.xor_assign(&LinearExpr::input(Term::Crc(2)));
        let mut complement = shared.clone();
        complement.constant = true;
        let equations = Equations {
            spec: CrcSpec::new(4, 3, 1, BitOrder::MsbFirst).unwrap(),
            outputs: vec![shared.clone(), complement, shared.clone(), shared],
        };
        let program = equations.factor();
        assert_eq!(program.temporaries.len(), 2);
        assert!(program.xor_count() < equations.xor_count());
        for crc in 0..16 {
            assert_eq!(program.evaluate(crc, 0), equations.evaluate(crc, 0));
        }
        assert_eq!(program.equations(), equations.factor().equations());
    }
}
