use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crc-gen"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn output_formats_and_stats() {
    for (format, marker) in [
        ("raw", "out[15] = e"),
        ("simplified", "out[15] = "),
        ("equations", "t0 = "),
        ("rust", "pub fn crc_update("),
        ("word-rust", "pub fn crc_update("),
        ("block-rust", "pub fn crc_update_16("),
        ("pclmul-rust", "pub unsafe fn crc_update_bytes_pclmul("),
    ] {
        for order in ["--msb-first", "--lsb-first"] {
            let output = run(&["0x10_21", order, "--format", format, "--stats"]);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains(marker));
            assert!(String::from_utf8_lossy(&output.stderr).contains("factored:"));
        }
    }
    assert_eq!(run(&[]).stdout, run(&["0b1000000100001"]).stdout);
    assert!(run(&["--help"]).status.success());
    assert!(String::from_utf8_lossy(&run(&["--help"]).stdout).contains("block-rust"));
}

#[test]
fn reject_invalid_arguments_without_panicking() {
    for args in [
        vec!["--width", "0"],
        vec!["--width", "65"],
        vec!["--width", "256"],
        vec!["--data-bits", "0"],
        vec!["--data-bits", "65"],
        vec!["0x11021"],
        vec!["0xfffffffffffffffff"],
        vec!["-1"],
        vec!["--poly"],
        vec!["--poly", "wat"],
        vec!["--poly", "0x"],
        vec!["1", "2"],
        vec!["0x1021", "--poly", "0x1021"],
        vec!["--msb-first", "--lsb-first"],
        vec!["--format", "bogus"],
        vec!["--format"],
        vec!["--unknown"],
        vec!["--data-bits", "16", "--format", "pclmul-rust"],
        vec!["--width", "24", "--format", "block-rust"],
        vec!["--data-bits", "16", "--format", "block-rust"],
    ] {
        let output = run(&args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}");
        assert!(stderr.starts_with("crc-gen:"), "{args:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
        assert!(output.stdout.is_empty());
    }
}
