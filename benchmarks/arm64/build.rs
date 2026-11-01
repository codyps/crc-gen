use crc_gen::{BitOrder, CrcSpec};
use std::{env, fmt::Write, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CRC_GEN_BENCH_COMMIT");
    let compiler = std::process::Command::new(env::var_os("RUSTC").unwrap())
        .arg("--version")
        .output()
        .unwrap();
    assert!(compiler.status.success());
    println!(
        "cargo:rustc-env=CRC_GEN_BENCH_RUSTC_VERSION={}",
        String::from_utf8_lossy(&compiler.stdout).trim()
    );
    // All polynomials are normal, with their leading coefficient omitted.
    let cases = [
        ("smbus8", 8, 7, false, Some("CRC_8_SMBUS")),
        ("maxim8", 8, 0x31, true, Some("CRC_8_MAXIM_DOW")),
        ("arc16", 16, 0x8005, true, Some("CRC_16_ARC")),
        ("xmodem16", 16, 0x1021, false, Some("CRC_16_XMODEM")),
        ("kermit16", 16, 0x1021, true, Some("CRC_16_KERMIT")),
        ("ieee32", 32, 0x04c11db7, true, Some("CRC_32_ISO_HDLC")),
        ("castagnoli32", 32, 0x1edc6f41, true, Some("CRC_32_ISCSI")),
        (
            "ecma64",
            64,
            0x42f0e1eba9ea3693,
            false,
            Some("CRC_64_ECMA_182"),
        ),
        ("we64", 64, 0x42f0e1eba9ea3693, false, Some("CRC_64_WE")),
        ("xz64", 64, 0x42f0e1eba9ea3693, true, Some("CRC_64_XZ")),
        ("iso64", 64, 0x1b, true, Some("CRC_64_GO_ISO")),
        ("nvme64", 64, 0xad93d23594c93659, true, Some("CRC_64_NVME")),
        ("candidate32_e5", 32, 0xe5, false, None),
        ("candidate64_f1", 64, 0xf1, false, None),
    ];
    let mut code = String::new();
    let mut registration =
        String::from("pub(super) fn make_cases() -> Vec<Case> { let mut cases=Vec::new();\n");
    for (name, width, poly, reflected, algorithm) in cases {
        let order = if reflected {
            BitOrder::LsbFirst
        } else {
            BitOrder::MsbFirst
        };
        let spec = CrcSpec::new(width, poly, 8, order).unwrap();
        writeln!(
            code,
            "#[allow(dead_code)] mod {name} {{ {} }}",
            spec.pmull().unwrap().rust()
        )
        .unwrap();
        writeln!(
            code,
            "#[allow(dead_code)] mod {name}_block {{ {} }}",
            spec.block16().unwrap().rust()
        )
        .unwrap();
        let ty = format!("u{width}");
        let alg = if let Some(algorithm) = algorithm {
            format!("crc::{algorithm}")
        } else {
            let alg = format!("ALG_{}", name.to_uppercase());
            writeln!(code,"const {alg}:crc::Algorithm<{ty}>=crc::Algorithm {{width:{width},poly:{poly},init:0,refin:{reflected},refout:{reflected},xorout:0,check:0,residue:0}};").unwrap();
            alg
        };
        for method in ["scalar_byte", "scalar_block16", "dispatch", "pmull"] {
            let body = match method {
                "scalar_byte" => format!("{name}::crc_update_bytes_scalar(value,bytes)"),
                "scalar_block16" => format!("{name}_block::crc_update_bytes(value,bytes)"),
                "dispatch" => format!("{name}::crc_update_bytes(value,bytes)"),
                _ => format!(
                    r#"{{
                    #[cfg(all(target_arch="aarch64",target_endian="little"))]
                    {{ unsafe {{ {name}::crc_update_bytes_pmull(value,bytes) }} }}
                    #[cfg(not(all(target_arch="aarch64",target_endian="little")))]
                    {{ {name}::crc_update_bytes_scalar(value,bytes) }}
                }}"#
                ),
            };
            writeln!(
                code,
                r#"#[inline(never)] fn {name}_{method}(bytes:&[u8])->u64 {{
                let value=if {alg}.refin {{{alg}.init.reverse_bits()}} else {{{alg}.init}} as u64;
                let value={body}; value ^ {alg}.xorout as u64
            }}"#
            )
            .unwrap();
        }
        for lanes in [1, 16] {
            writeln!(code,r#"#[inline(never)] fn {name}_table{lanes}(bytes:&[u8])->u64 {{
                static CRC:crc::Crc<{ty},crc::Table<{lanes}>>=crc::Crc::<{ty},crc::Table<{lanes}>>::new(&{alg});
                CRC.checksum(bytes) as u64
            }}"#).unwrap();
        }
        let check = if algorithm.is_some() {
            format!("Some({alg}.check as u64)")
        } else {
            "None".into()
        };
        writeln!(registration,r#"cases.push(Case {{name:"{name}",width:{width},polynomial:{poly},reflected:{reflected},check:{check},methods:vec![
            Method {{name:"scalar_byte",checksum:{name}_scalar_byte}},
            Method {{name:"scalar_block16",checksum:{name}_scalar_block16}},
            Method {{name:"table1",checksum:{name}_table1}},
            Method {{name:"table16",checksum:{name}_table16}},
            Method {{name:"dispatch",checksum:{name}_dispatch}},
        ]}});
        if {name}::pmull_available() {{cases.last_mut().unwrap().methods.push(Method {{name:"pmull",checksum:{name}_pmull}});}}
        assert_eq!({alg}.refin,{alg}.refout,"benchmark wrapper requires matching refin/refout");
        "#).unwrap();
    }
    registration.push_str("cases }\n");
    code.push_str(&registration);
    code.push_str("pub(super) fn hardware_available()->bool {ecma64::pmull_available()}\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("generated.rs"),
        code,
    )
    .unwrap();
}
