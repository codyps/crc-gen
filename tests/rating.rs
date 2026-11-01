use crc_gen::{rating::*, BitOrder, CrcSpec};

fn graph() -> Graph {
    Graph {
        version: GRAPH_VERSION,
        nodes: vec![
            Node {
                op: Op::Crc,
                inputs: vec![],
            },
            Node {
                op: Op::Data,
                inputs: vec![],
            },
            Node {
                op: Op::And(1),
                inputs: vec![0],
            },
            Node {
                op: Op::And(1),
                inputs: vec![1],
            },
            Node {
                op: Op::Xor,
                inputs: vec![2, 3],
            },
            Node {
                op: Op::Shl(1),
                inputs: vec![4],
            },
        ],
        output: 5,
    }
}

#[test]
fn operation_kinds_and_dependency_latency() {
    let graph = graph();
    let metrics = graph.measure(&CostModel::default()).unwrap();
    assert_eq!(metrics.operations.total(), 4);
    assert_eq!(metrics.weighted_cost, 4);
    assert_eq!(metrics.critical_path, 3);
    assert_eq!(metrics.max_live_values, 3);
    let mut model = CostModel {
        id: "custom".into(),
        ..CostModel::default()
    };
    model.costs.shl = 7;
    model.latencies.shl = 4;
    let metrics = graph.measure(&model).unwrap();
    assert_eq!(metrics.weighted_cost, 10);
    assert_eq!(metrics.critical_path, 6);
    let restored: Graph = serde_json::from_slice(&serde_json::to_vec(&graph).unwrap()).unwrap();
    assert_eq!(restored, graph);
    assert_eq!(graph.evaluate(1, 0).unwrap(), 2);
}

#[test]
fn reject_non_affine_invalid_or_overflowing_graphs() {
    let mut invalid = graph();
    invalid.nodes[4].inputs[0] = 4;
    assert!(invalid.validate().is_err());
    let mut invalid = graph();
    invalid.nodes[5].op = Op::Shl(64);
    assert!(invalid.validate().is_err());
    let mut invalid = graph();
    invalid.nodes[4].op = Op::Or;
    assert!(invalid.validate().is_err());
    let mut model = CostModel::default();
    model.costs.and = u64::MAX;
    assert!(graph().measure(&model).is_err());
}

#[test]
fn wide_and_non_byte_results_are_verified_and_serializable() {
    for (width, poly, bits) in [
        (1, 1, 64),
        (9, 0x107, 7),
        (16, 0x1021, 8),
        (32, 0x1edc6f41, 8),
        (64, 0x42f0e1eba9ea3693, 8),
    ] {
        for order in [BitOrder::MsbFirst, BitOrder::LsbFirst] {
            let spec = CrcSpec::new(width, poly, bits, order).unwrap();
            let evaluation = Evaluation::generate(spec).unwrap();
            let bytes = serde_json::to_vec(&evaluation).unwrap();
            let restored: Evaluation = serde_json::from_slice(&bytes).unwrap();
            restored.verify(spec).unwrap();
            assert!(restored.variants.iter().all(|v| !v.graph.nodes.is_empty()));
        }
    }
}

#[test]
fn block_ratings_and_historical_scalar_results_round_trip() {
    let spec = CrcSpec::new(32, 0xe5, 8, BitOrder::MsbFirst).unwrap();
    let evaluation = Evaluation::generate(spec).unwrap();
    let block = evaluation.block16.as_ref().unwrap();
    assert_eq!(block.graph.version, BLOCK_GRAPH_VERSION);
    assert_eq!(
        block.metrics,
        block.graph.measure(&evaluation.model).unwrap()
    );
    let mut corrupted = evaluation.clone();
    corrupted.block16.as_mut().unwrap().metrics.weighted_cost += 1;
    assert!(corrupted.verify(spec).is_err());
    let mut missing = evaluation.clone();
    missing.block16 = None;
    assert!(missing.verify(spec).is_err());
    let mut old = serde_json::to_value(&evaluation).unwrap();
    old.as_object_mut().unwrap().remove("block16");
    old["optimizer"] = serde_json::json!("gf2-shift-fold-words-v2");
    let historical: Evaluation = serde_json::from_value(old).unwrap();
    assert!(historical.block16.is_none());
    historical.verify(spec).unwrap();
    let mut invalid = block.graph.clone();
    let input = invalid
        .nodes
        .iter_mut()
        .find(|n| matches!(n.op, Op::BlockWord { .. }))
        .unwrap();
    input.op = Op::BlockWord {
        offset: 15,
        width: 64,
        reflected: false,
    };
    assert!(invalid.validate().is_err());
    let mut invalid = block.graph.clone();
    invalid.version = GRAPH_VERSION;
    assert!(invalid.validate().is_err());
}
