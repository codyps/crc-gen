//! Versioned, serializable operation graphs and target-independent ratings.
//! Graphs describe word operations, not predicted machine instructions.

use crate::{BitOrder, CrcSpec, Equations, LinearExpr, Program, Term};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

pub type Result<T> = std::result::Result<T, String>;
pub const GRAPH_VERSION: u32 = 1;
pub const OPTIMIZER: &str = "gf2-pairs-and-diagonal-words-v1";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", content = "argument", rename_all = "snake_case")]
pub enum Op {
    Crc,
    Data,
    Constant(u64),
    Xor,
    And(u64),
    Or,
    Shl(u8),
    Shr(u8),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub op: Op,
    pub inputs: Vec<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    pub version: u32,
    pub nodes: Vec<Node>,
    pub output: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operations {
    pub xor: u64,
    pub and: u64,
    pub or: u64,
    pub shl: u64,
    pub shr: u64,
}

impl Operations {
    fn value(&self, op: &Op) -> u64 {
        match op {
            Op::Xor => self.xor,
            Op::And(_) => self.and,
            Op::Or => self.or,
            Op::Shl(_) => self.shl,
            Op::Shr(_) => self.shr,
            _ => 0,
        }
    }
    pub fn total(&self) -> u64 {
        self.xor + self.and + self.or + self.shl + self.shr
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostModel {
    pub id: String,
    pub costs: Operations,
    pub latencies: Operations,
}

impl Default for CostModel {
    fn default() -> Self {
        let unit = Operations {
            xor: 1,
            and: 1,
            or: 1,
            shl: 1,
            shr: 1,
        };
        Self {
            id: "unit-word-ops-v1".into(),
            costs: unit.clone(),
            latencies: unit,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metrics {
    pub model: String,
    pub operations: Operations,
    pub weighted_cost: u64,
    pub critical_path: u64,
    /// Maximum simultaneously live DAG values in the recorded execution order.
    /// Includes inputs and constants; this is not an ISA register allocation.
    pub max_live_values: usize,
}

#[derive(Default)]
struct Builder {
    nodes: Vec<Node>,
    intern: BTreeMap<(Op, Vec<usize>), usize>,
}

impl Builder {
    fn node(&mut self, op: Op, mut inputs: Vec<usize>) -> usize {
        if matches!(op, Op::Xor | Op::Or) {
            inputs.sort_unstable();
        }
        let key = (op.clone(), inputs.clone());
        if let Some(id) = self.intern.get(&key) {
            return *id;
        }
        let id = self.nodes.len();
        self.nodes.push(Node { op, inputs });
        self.intern.insert(key, id);
        id
    }
    fn constant(&mut self, v: u64) -> usize {
        self.node(Op::Constant(v), vec![])
    }
    fn shift(&mut self, input: usize, amount: i16) -> usize {
        if amount == 0 {
            input
        } else if amount > 0 {
            self.node(Op::Shl(amount as u8), vec![input])
        } else {
            self.node(Op::Shr((-amount) as u8), vec![input])
        }
    }
    fn and(&mut self, input: usize, mask: u64) -> usize {
        if mask == 0 {
            self.constant(0)
        } else if mask == u64::MAX {
            input
        } else {
            self.node(Op::And(mask), vec![input])
        }
    }
    fn xor_sum(&mut self, terms: Vec<usize>) -> usize {
        let mut terms = terms.into_iter();
        let Some(mut value) = terms.next() else {
            return self.constant(0);
        };
        for next in terms {
            value = self.node(Op::Xor, vec![value, next]);
        }
        value
    }
    fn expr(&mut self, expression: &LinearExpr, crc: usize, data: usize, temps: &[usize]) -> usize {
        let mut terms = Vec::new();
        for term in &expression.terms {
            let value = match *term {
                Term::Temporary(i) => temps[i],
                Term::Crc(i) | Term::Data(i) => {
                    let word = if matches!(term, Term::Crc(_)) {
                        crc
                    } else {
                        data
                    };
                    let shifted = self.shift(word, -i16::from(i));
                    self.and(shifted, 1)
                }
            };
            terms.push(value);
        }
        if expression.constant {
            terms.push(self.constant(1));
        }
        self.xor_sum(terms)
    }
    fn finish(self, output: usize) -> Graph {
        let mut reachable = vec![false; self.nodes.len()];
        let mut pending = vec![output];
        while let Some(id) = pending.pop() {
            if std::mem::replace(&mut reachable[id], true) {
                continue;
            }
            pending.extend(&self.nodes[id].inputs);
        }
        let mut remap = vec![0; self.nodes.len()];
        let mut nodes = Vec::new();
        for (i, node) in self.nodes.into_iter().enumerate() {
            if reachable[i] {
                remap[i] = nodes.len();
                nodes.push(Node {
                    op: node.op,
                    inputs: node.inputs.iter().map(|id| remap[*id]).collect(),
                });
            }
        }
        Graph {
            version: GRAPH_VERSION,
            nodes,
            output: remap[output],
        }
    }
}

impl Program {
    /// Hash-consed word DAG for the factored bit program (including bit extraction
    /// and packing). The graph has an explicit, reproducible execution schedule.
    pub fn operation_graph(&self) -> Graph {
        let mut b = Builder::default();
        let crc = b.node(Op::Crc, vec![]);
        let data = b.node(Op::Data, vec![]);
        let mut temps = Vec::new();
        for expression in &self.temporaries {
            temps.push(b.expr(expression, crc, data, &temps));
        }
        let mut packed = None;
        for (bit, expression) in self.outputs.iter().enumerate() {
            let value = b.expr(expression, crc, data, &temps);
            let value = b.shift(value, bit as i16);
            packed = Some(match packed {
                None => value,
                Some(previous) => b.node(Op::Or, vec![previous, value]),
            });
        }
        b.finish(packed.unwrap())
    }
}

impl Equations {
    /// Word DAG grouping equal shifts and masks, with shared source operations.
    pub fn word_operation_graph(&self) -> Graph {
        let mut groups = BTreeMap::<(bool, i16), u64>::new();
        let mut constant = 0;
        for (output, expression) in self.outputs.iter().enumerate() {
            if expression.constant {
                constant |= 1u64 << output;
            }
            for term in &expression.terms {
                let (data, input) = match *term {
                    Term::Crc(i) => (false, i),
                    Term::Data(i) => (true, i),
                    Term::Temporary(_) => unreachable!(),
                };
                *groups
                    .entry((data, output as i16 - i16::from(input)))
                    .or_default() |= 1u64 << input;
            }
        }
        let mut b = Builder::default();
        let crc = b.node(Op::Crc, vec![]);
        let data = b.node(Op::Data, vec![]);
        let mut values = Vec::new();
        for ((is_data, shift), mask) in groups {
            let value = b.and(if is_data { data } else { crc }, mask);
            values.push(b.shift(value, shift));
        }
        if constant != 0 {
            values.push(b.constant(constant));
        }
        let output = b.xor_sum(values);
        b.finish(output)
    }
}

impl Graph {
    pub fn validate(&self) -> Result<()> {
        if self.version != GRAPH_VERSION {
            return Err("unsupported graph version".into());
        }
        if self.nodes.is_empty() || self.output >= self.nodes.len() {
            return Err("invalid graph output".into());
        }
        let mut possible_bits = Vec::<u64>::with_capacity(self.nodes.len());
        for (i, node) in self.nodes.iter().enumerate() {
            let arity = match node.op {
                Op::Crc | Op::Data | Op::Constant(_) => 0,
                Op::And(_) | Op::Shl(_) | Op::Shr(_) => 1,
                Op::Xor | Op::Or => 2,
            };
            if node.inputs.len() != arity || node.inputs.iter().any(|input| *input >= i) {
                return Err(format!("invalid or non-topological inputs at node {i}"));
            }
            if matches!(node.op, Op::Shl(n) | Op::Shr(n) if n >= 64) {
                return Err("shift must be below 64".into());
            }
            let input = |i: usize| possible_bits[node.inputs[i]];
            let bits = match node.op {
                Op::Crc | Op::Data => u64::MAX,
                Op::Constant(value) => value,
                Op::And(mask) => input(0) & mask,
                Op::Shl(n) => input(0) << n,
                Op::Shr(n) => input(0) >> n,
                Op::Xor => input(0) | input(1),
                Op::Or => {
                    if input(0) & input(1) != 0 {
                        return Err(
                            "OR operands overlap; graph is not proven affine over GF(2)".into()
                        );
                    }
                    input(0) | input(1)
                }
            };
            possible_bits.push(bits);
        }
        let mut used = BTreeSet::from([self.output]);
        for i in (0..self.nodes.len()).rev() {
            if used.contains(&i) {
                used.extend(&self.nodes[i].inputs);
            }
        }
        if used.len() != self.nodes.len() {
            return Err("graph contains unreachable operations".into());
        }
        Ok(())
    }

    fn eval_unchecked(&self, crc: u64, data: u64) -> u64 {
        let mut values = Vec::<u64>::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let input = |i: usize| values[node.inputs[i]];
            let value = match node.op {
                Op::Crc => crc,
                Op::Data => data,
                Op::Constant(value) => value,
                Op::Xor => input(0) ^ input(1),
                Op::And(mask) => input(0) & mask,
                Op::Or => input(0) | input(1),
                Op::Shl(n) => input(0) << n,
                Op::Shr(n) => input(0) >> n,
            };
            values.push(value);
        }
        values[self.output]
    }

    pub fn evaluate(&self, crc: u64, data: u64) -> Result<u64> {
        self.validate()?;
        Ok(self.eval_unchecked(crc, data))
    }

    pub fn measure(&self, model: &CostModel) -> Result<Metrics> {
        self.validate()?;
        if model.id.is_empty() {
            return Err("cost model needs a nonempty id".into());
        }
        let mut counts = Operations::default();
        let mut cost = 0u64;
        let mut depth = Vec::<u64>::new();
        let mut last_use: Vec<_> = (0..self.nodes.len()).collect();
        for (i, node) in self.nodes.iter().enumerate() {
            for input in &node.inputs {
                last_use[*input] = i;
            }
        }
        last_use[self.output] = self.nodes.len();
        let (mut live, mut peak) = (0usize, 0usize);
        for (i, node) in self.nodes.iter().enumerate() {
            match node.op {
                Op::Xor => counts.xor += 1,
                Op::And(_) => counts.and += 1,
                Op::Or => counts.or += 1,
                Op::Shl(_) => counts.shl += 1,
                Op::Shr(_) => counts.shr += 1,
                _ => (),
            }
            cost = cost
                .checked_add(model.costs.value(&node.op))
                .ok_or("cost overflow")?;
            let previous = node.inputs.iter().map(|id| depth[*id]).max().unwrap_or(0);
            depth.push(
                previous
                    .checked_add(model.latencies.value(&node.op))
                    .ok_or("latency overflow")?,
            );
            live += 1;
            peak = peak.max(live);
            for input in node.inputs.iter().copied().collect::<BTreeSet<_>>() {
                if last_use[input] == i {
                    live -= 1;
                }
            }
        }
        Ok(Metrics {
            model: model.id.clone(),
            operations: counts,
            weighted_cost: cost,
            critical_path: depth[self.output],
            max_live_values: peak,
        })
    }

    pub fn rust(&self) -> Result<String> {
        self.validate()?;
        let mut code =
            "#[allow(unused_variables)]\npub fn crc_update(crc: u64, data: u64) -> u64 {\n"
                .to_string();
        for (i, node) in self.nodes.iter().enumerate() {
            let value = match node.op {
                Op::Crc => "crc".into(),
                Op::Data => "data".into(),
                Op::Constant(v) => format!("{v}u64"),
                Op::And(mask) => format!("v{} & {mask}u64", node.inputs[0]),
                Op::Shl(n) => format!("v{} << {n}", node.inputs[0]),
                Op::Shr(n) => format!("v{} >> {n}", node.inputs[0]),
                Op::Xor | Op::Or => format!(
                    "v{} {} v{}",
                    node.inputs[0],
                    if node.op == Op::Xor { "^" } else { "|" },
                    node.inputs[1]
                ),
            };
            writeln!(code, "    let v{i} = {value};").unwrap();
        }
        writeln!(code, "    v{}\n}}", self.output).unwrap();
        Ok(code)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub name: String,
    pub graph: Graph,
    pub metrics: Metrics,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evaluation {
    pub version: u32,
    pub optimizer: String,
    pub model: CostModel,
    pub width: u8,
    pub polynomial: String,
    pub data_bits: u8,
    pub order: String,
    pub raw_xors: usize,
    pub canonical_xors: usize,
    pub factored_xors: usize,
    pub variants: Vec<Variant>,
}

pub fn order_name(order: BitOrder) -> &'static str {
    match order {
        BitOrder::MsbFirst => "msb",
        BitOrder::LsbFirst => "lsb",
    }
}

pub fn parse_order(order: &str) -> Result<BitOrder> {
    match order {
        "msb" => Ok(BitOrder::MsbFirst),
        "lsb" => Ok(BitOrder::LsbFirst),
        _ => Err("order must be msb or lsb".into()),
    }
}

/// Independent carry-less long division, also used to verify every saved result.
pub fn reference(spec: CrcSpec, crc: u64, data: u64) -> u64 {
    let (width, bits) = (spec.width(), spec.data_bits());
    let (mut crc, mut data) = (
        crc & (u64::MAX >> (64 - width)),
        data & (u64::MAX >> (64 - bits)),
    );
    if spec.order() == BitOrder::LsbFirst {
        crc = crc.reverse_bits() >> (64 - width);
        data = data.reverse_bits() >> (64 - bits);
    }
    let mut numerator = ((crc as u128) << bits) ^ ((data as u128) << width);
    let divisor = (1u128 << width) | spec.polynomial() as u128;
    for bit in (u16::from(width)..u16::from(width) + u16::from(bits)).rev() {
        if numerator & (1u128 << bit) != 0 {
            numerator ^= divisor << (bit - u16::from(width));
        }
    }
    let value = numerator as u64;
    if spec.order() == BitOrder::LsbFirst {
        value.reverse_bits() >> (64 - width)
    } else {
        value
    }
}

impl Evaluation {
    pub fn generate(spec: CrcSpec) -> Result<Self> {
        let symbolic = spec.symbolic();
        let equations = symbolic.simplify();
        let factored = equations.factor();
        let mut variants = Vec::new();
        for (name, graph) in [
            ("factored-bits", factored.operation_graph()),
            ("masked-words", equations.word_operation_graph()),
        ] {
            let metrics = graph.measure(&CostModel::default())?;
            variants.push(Variant {
                name: name.into(),
                graph,
                metrics,
            });
        }
        let result = Self {
            version: 1,
            optimizer: OPTIMIZER.into(),
            model: CostModel::default(),
            width: spec.width(),
            polynomial: format!("{:#x}", spec.polynomial()),
            data_bits: spec.data_bits(),
            order: order_name(spec.order()).into(),
            raw_xors: symbolic.xor_count(),
            canonical_xors: equations.xor_count(),
            factored_xors: factored.xor_count(),
            variants,
        };
        result.verify(spec)?;
        Ok(result)
    }

    pub fn verify(&self, spec: CrcSpec) -> Result<()> {
        if self.version != 1
            || self.optimizer.is_empty()
            || self.width != spec.width()
            || self.polynomial != format!("{:#x}", spec.polynomial())
            || self.data_bits != spec.data_bits()
            || self.order != order_name(spec.order())
            || self.variants.is_empty()
        {
            return Err("result metadata does not match task/version".into());
        }
        for variant in &self.variants {
            variant.graph.validate()?;
            if variant.graph.measure(&self.model)? != variant.metrics {
                return Err("stored metrics disagree with graph".into());
            }
            let check = |crc, data| {
                if variant.graph.eval_unchecked(crc, data) == reference(spec, crc, data) {
                    Ok(())
                } else {
                    Err(format!(
                        "{} failed polynomial-division verification",
                        variant.name
                    ))
                }
            };
            check(0, 0)?;
            // Include all 64 bits: high bits must be ignored too. All supported
            // graphs are linear, so these basis vectors cover every assignment.
            for bit in 0..64 {
                check(1u64 << bit, 0)?;
                check(0, 1u64 << bit)?;
            }
            check(u64::MAX, u64::MAX)?;
        }
        Ok(())
    }

    pub fn best(&self, model: &CostModel) -> Result<(usize, Metrics)> {
        let mut best: Option<(usize, Metrics)> = None;
        for (i, variant) in self.variants.iter().enumerate() {
            let metrics = variant.graph.measure(model)?;
            if best.as_ref().is_none_or(|(_, current)| {
                (metrics.weighted_cost, metrics.critical_path)
                    < (current.weighted_cost, current.critical_path)
            }) {
                best = Some((i, metrics));
            }
        }
        best.ok_or_else(|| "result has no variants".into())
    }
}
