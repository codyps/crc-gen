use crc_gen::{rating::reference, BitOrder, CrcSpec};
use std::fmt::Write;
use std::process::Command;

#[test]
fn folding_constants_match_long_division_including_degenerate_polynomials() {
    for (width, polynomial) in [
        (1, 1),
        (8, 7),
        (16, 0x1021),
        (32, 0xe5),
        (63, 2),
        (64, 0),
        (64, 2),
        (64, 0x42f0e1eba9ea3693),
        (64, u64::MAX),
    ] {
        let generator = CrcSpec::new(width, polynomial, 8, BitOrder::MsbFirst)
            .unwrap()
            .pclmul()
            .unwrap();
        let c = generator.constants();
        let spec = CrcSpec::new(64, c.polynomial, 64, BitOrder::MsbFirst).unwrap();
        for (exponent, expected) in [
            (64, c.x64),
            (128, c.x128),
            (192, c.x192),
            (256, c.x256),
            (320, c.x320),
            (512, c.x512),
            (576, c.x576),
        ] {
            // Compute each power independently via polynomial long division.
            let mut remainder = 1;
            for _ in 0..exponent / 64 {
                remainder = reference(spec, remainder, 0);
            }
            assert_eq!(remainder, expected);
        }
        let mut product = 0u128;
        for bit in 0..64 {
            if c.reciprocal & (1 << bit) != 0 {
                product ^= u128::from(c.polynomial) << bit;
            }
        }
        let residual = product ^ (u128::from(c.reciprocal ^ c.polynomial) << 64);
        assert_eq!(residual, u128::from(c.x128), "quotient identity");
    }
    assert!(CrcSpec::new(16, 0x1021, 16, BitOrder::MsbFirst)
        .unwrap()
        .pclmul()
        .is_err());
}

#[test]
fn emitted_intrinsics_match_division_across_boundaries_alignments_and_streaming() {
    let cases = [
        (1, 1),
        (4, 3),
        (8, 7),
        (8, 0x25),
        (9, 0x107),
        (16, 0x1021),
        (16, 7),
        (16, 0x11b),
        (24, 0x864cfb),
        (32, 0xe5),
        (32, 0x210801),
        (32, 0x04c11db7),
        (32, 0x1edc6f41),
        (63, 2),
        (64, 0),
        (64, 2),
        (64, 0x57),
        (64, 0xf1),
        (64, 0x42f0e1eba9ea3693),
        (64, u64::MAX),
    ];
    let mut source = String::new();
    let mut calls = String::new();
    for (i, (width, poly)) in cases.into_iter().enumerate() {
        for (j, order) in [BitOrder::MsbFirst, BitOrder::LsbFirst]
            .into_iter()
            .enumerate()
        {
            let name = format!("case_{i}_{j}");
            let spec = CrcSpec::new(width, poly, 8, order).unwrap();
            writeln!(
                source,
                "#[allow(dead_code)] mod {name} {{ {} }}",
                spec.pclmul().unwrap().rust()
            )
            .unwrap();
            writeln!(calls,r#"
    {{
        let mut functions: Vec<(&str, fn(u64,&[u8])->u64)> = vec![("scalar",{name}::crc_update_bytes_scalar),("dispatch",{name}::crc_update_bytes)];
        #[cfg(target_arch="x86_64")]
        {{
            if {name}::pclmul_available() {{ functions.push(("pclmul",|c,b| unsafe {{ {name}::crc_update_bytes_pclmul(c,b) }})); }}
            if {name}::vpclmul256_available() {{ functions.push(("vpclmul256",|c,b| unsafe {{ {name}::crc_update_bytes_vpclmul256(c,b) }})); }}
            if {name}::vpclmul512_available() {{ functions.push(("vpclmul512",|c,b| unsafe {{ {name}::crc_update_bytes_vpclmul512(c,b) }})); }}
        }}
        run_case("{name}",{width},{poly}, {reflected},&functions);
    }}
"#,reflected=order==BitOrder::LsbFirst).unwrap();
        }
    }
    source.push_str(r#"
// Independent byte-by-byte polynomial long division, not the generator's code.
fn oracle(width:u32,poly:u64,lsb:bool,mut crc:u64,bytes:&[u8])->u64 {
    let mask=u64::MAX>>(64-width);crc&=mask;
    if lsb {crc=crc.reverse_bits()>>(64-width);}
    let divisor=(1u128<<width)|poly as u128;
    for &byte in bytes {
        let byte=if lsb {byte.reverse_bits()} else {byte};
        let mut dividend=((crc as u128)<<8)^((byte as u128)<<width);
        for bit in (width..width+8).rev() {
            if dividend&(1u128<<bit)!=0 {dividend^=divisor<<(bit-width);}
        }
        crc=dividend as u64;
    }
    if lsb {crc.reverse_bits()>>(64-width)} else {crc}
}
fn run_case(name:&str,width:u32,poly:u64,lsb:bool,functions:&[(&str,fn(u64,&[u8])->u64)]) {
    let mut rng=0x9e3779b97f4a7c15u64;
    let buffer:Vec<u8>=(0..8300).map(|_|{rng^=rng<<13;rng^=rng>>7;rng^=rng<<17;rng as u8}).collect();
    for len in (0..=260).chain([511,512,513,1023,1024,1025,4095,4096,4097,8192]) {
        let offset=len%64;
        let bytes=&buffer[offset..offset+len];
        for seed in [0,u64::MAX,0xfedcba9876543210] {
            let expected=oracle(width,poly,lsb,seed,bytes);
            for &(method,f) in functions {assert_eq!(f(seed,bytes),expected,"{name} {method} len={len} offset={offset} seed={seed:x}");}
        }
    }
    for offset in 0..64 {
        for len in [16,17,31,32,33,63,64,65,127,128,129] {
            let bytes=&buffer[offset..offset+len];let expected=oracle(width,poly,lsb,rng,bytes);
            for &(method,f) in functions {assert_eq!(f(rng,bytes),expected,"{name} {method} alignment={offset} len={len}");}
        }
    }
    // Every data and seed basis bit exercises the complete affine 128-bit map.
    for bit in 0..192 {
        let mut block=[0u8;16];let seed=if bit<64 {1<<bit} else {block[(bit-64)/8]=1<<((bit-64)%8);0};
        let expected=oracle(width,poly,lsb,seed,&block);
        for &(method,f) in functions {assert_eq!(f(seed,&block),expected,"{name} {method} basis={bit}");}
    }
    // A real inaccessible page on each side catches overreads that output
    // comparisons cannot detect. Keep platform-specific FFI in the subprocess.
    #[cfg(all(target_os="linux",target_arch="x86_64"))]
    unsafe {
        unsafe extern "C" {
            fn mmap(addr:*mut core::ffi::c_void,len:usize,prot:i32,flags:i32,fd:i32,offset:isize)->*mut core::ffi::c_void;
            fn mprotect(addr:*mut core::ffi::c_void,len:usize,prot:i32)->i32;
            fn munmap(addr:*mut core::ffi::c_void,len:usize)->i32;
        }
        let pages=mmap(core::ptr::null_mut(),3*4096,0,0x22,-1,0);
        assert_ne!(pages as isize,-1);
        let readable=pages.cast::<u8>().add(4096);
        assert_eq!(mprotect(readable.cast(),4096,3),0);
        readable.write_bytes(0xa5,4096);
        for len in [0,1,7,15,16,17,31,32,33,63,64,65,127,128,129,255,256,511] {
            for start in [readable,readable.add(4096-len)] {
                let bytes=core::slice::from_raw_parts(start,len);
                let expected=oracle(width,poly,lsb,rng,bytes);
                for &(method,f) in functions {assert_eq!(f(rng,bytes),expected,"{name} {method} guarded len={len}");}
            }
        }
        assert_eq!(munmap(pages,3*4096),0);
    }
    let bytes=&buffer[..2049];let expected=oracle(width,poly,lsb,rng,bytes);
    for chunk in [1,7,15,16,17,31,32,33,63,64,65,128,257] {
        for &(method,f) in functions {
            let mut crc=rng;for part in bytes.chunks(chunk) {crc=f(crc,part);}
            assert_eq!(crc,expected,"{name} {method} chunk={chunk}");
        }
    }
}
fn main(){
"#);
    source.push_str(&calls);
    source
        .push_str("println!(\"Verified 40 CRC configurations with all available backends\");\n}\n");
    let dir = std::env::temp_dir().join(format!("crc-gen-pclmul-test-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let input = dir.join("generated.rs");
    let output = dir.join(format!("generated{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&input, source).unwrap();
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compiled = Command::new(compiler)
        .args([
            "--edition=2021",
            "-Dwarnings",
            "-Dunsafe_op_in_unsafe_fn",
            "-O",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let executed = Command::new(&output).output().unwrap();
    assert!(
        executed.status.success(),
        "{}",
        String::from_utf8_lossy(&executed.stderr)
    );
    println!("{}", String::from_utf8_lossy(&executed.stdout));
    std::fs::remove_dir_all(dir).unwrap();
}
