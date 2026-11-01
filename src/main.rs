use crc_gen::{BitOrder, CrcSpec};
use std::io::{self, Write};

mod search_cli;

const HELP: &str = "Usage: crc-gen [POLYNOMIAL] [OPTIONS]
       crc-gen search --help

Generate symbolic, table-free CRC streaming update equations.

  --width N          CRC register width, 1..64 (default: 16)
  --poly P           Polynomial without its leading bit (default: 0x1021)
  --data-bits N      Number of input bits, 1..64 (default: 8)
  --lsb-first        Consume low input bits first; polynomial stays in normal form
  --msb-first        Consume high input bits first (default)
  --format FORMAT    raw | simplified | equations | rust | word-rust
                     (default: equations, with shared XOR temporaries)
  --stats            Print XOR operation counts to stderr
  -h, --help         Show this help

Numbers accept decimal, 0x hexadecimal, or 0b binary, with optional underscores.
All bit indices count from the least significant bit. Updates accept an arbitrary
register; CRC initialization, final reflection and final XOR are not applied.
";

#[derive(Clone, Copy)]
enum Format {
    Raw,
    Simplified,
    Equations,
    Rust,
    WordRust,
}

struct Options {
    spec: CrcSpec,
    format: Format,
    stats: bool,
}

fn number(text: &str) -> Result<u64, String> {
    let text = text.replace('_', "");
    let (digits, radix) =
        if let Some(digits) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (digits, 16)
        } else if let Some(digits) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
            (digits, 2)
        } else {
            (text.as_str(), 10)
        };
    if digits.is_empty() || !digits.chars().all(|digit| digit.is_digit(radix)) {
        return Err(format!("invalid unsigned integer: {text:?}"));
    }
    u64::from_str_radix(digits, radix).map_err(|_| format!("integer exceeds 64 bits: {text:?}"))
}

fn parse(args: impl Iterator<Item = String>) -> Result<Option<Options>, String> {
    let args: Vec<_> = args.collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(None);
    }
    let mut args = args.into_iter();
    let mut width = 16;
    let mut polynomial = None;
    let mut data_bits = 8;
    let mut order = BitOrder::MsbFirst;
    let mut order_set = false;
    let mut format = Format::Equations;
    let mut stats = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--width" | "--data-bits" | "--poly" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("{arg} requires a value"))?;
                let value = number(&value)?;
                match arg.as_str() {
                    "--width" => {
                        width =
                            u8::try_from(value).map_err(|_| "CRC width must be between 1 and 64")?
                    }
                    "--data-bits" => {
                        data_bits = u8::try_from(value)
                            .map_err(|_| "data width must be between 1 and 64")?
                    }
                    _ => {
                        if polynomial.replace(value).is_some() {
                            return Err("polynomial specified more than once".into());
                        }
                    }
                }
            }
            "--lsb-first" | "--msb-first" => {
                if order_set {
                    return Err("specify the bit order only once".into());
                }
                order = if arg == "--lsb-first" {
                    BitOrder::LsbFirst
                } else {
                    BitOrder::MsbFirst
                };
                order_set = true;
            }
            "--format" => {
                format = match args.next().as_deref() {
                    Some("raw") => Format::Raw,
                    Some("simplified") => Format::Simplified,
                    Some("equations") => Format::Equations,
                    Some("rust") => Format::Rust,
                    Some("word-rust") => Format::WordRust,
                    Some(other) => return Err(format!("unknown output format: {other}")),
                    None => return Err("--format requires a value".into()),
                };
            }
            "--stats" => stats = true,
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
            _ => {
                if polynomial.replace(number(&arg)?).is_some() {
                    return Err("polynomial specified more than once".into());
                }
            }
        }
    }
    let spec = CrcSpec::new(width, polynomial.unwrap_or(0x1021), data_bits, order)
        .map_err(|error| error.to_string())?;
    Ok(Some(Options {
        spec,
        format,
        stats,
    }))
}

fn run() -> Result<(), String> {
    let options = parse(std::env::args().skip(1))?;
    let mut output = io::BufWriter::new(io::stdout().lock());
    let Some(options) = options else {
        return output
            .write_all(HELP.as_bytes())
            .map_err(|error| error.to_string());
    };
    let symbolic = options.spec.symbolic();
    let simplified = symbolic.simplify();
    // Factoring is optional: don't pay for it when requesting raw or canonical output.
    let program = if options.stats || matches!(options.format, Format::Equations | Format::Rust) {
        Some(simplified.factor())
    } else {
        None
    };
    if options.stats {
        eprintln!(
            "raw DAG: {} XORs; canonical bit sums: {} XORs; factored: {} XORs",
            symbolic.xor_count(),
            simplified.xor_count(),
            program.as_ref().unwrap().xor_count()
        );
    }
    let text = match options.format {
        Format::Raw => symbolic.raw_equations(),
        Format::Simplified => simplified.equations(),
        Format::Equations => program.unwrap().equations(),
        Format::Rust => program.unwrap().rust(),
        Format::WordRust => simplified.word_rust(),
    };
    output
        .write_all(text.as_bytes())
        .and_then(|()| output.flush())
        .or_else(|error| {
            if error.kind() == io::ErrorKind::BrokenPipe {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(|error| error.to_string())
}

fn main() -> std::process::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("search") {
        return search_cli::main();
    }
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("crc-gen: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
