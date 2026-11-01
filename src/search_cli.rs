use clap::{Args, Parser, Subcommand};
use crc_gen::{
    catalogue::{self, Catalogue},
    rating::CostModel,
    search::{self, Manifest, RankOptions, RunOptions},
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "crc-gen search",
    about = "Durable polynomial search and portable operation-graph ratings"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct Database {
    #[arg(long)]
    db: PathBuf,
    #[arg(long)]
    run_id: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Create or resume a search. Completed jobs are never recomputed.
    Run {
        #[arg(long)]
        db: PathBuf,
        /// Resume an immutable stored manifest with the original engine binary.
        #[arg(long, conflicts_with_all=["catalogue","exhaustive","width","data_bits","orders","include_even"])]
        resume: Option<String>,
        /// A snapshot produced by scripts/import_koopman.py.
        #[arg(long)]
        catalogue: Option<PathBuf>,
        /// Enumerate CRC-8/16 (all monic polynomials with constant coefficient 1).
        #[arg(long)]
        exhaustive: bool,
        /// Default: 8,16 for exhaustive; 32,64 for catalogue searches.
        #[arg(long, value_delimiter = ',')]
        width: Vec<u8>,
        #[arg(long, value_delimiter = ',', default_value = "8")]
        data_bits: Vec<u8>,
        #[arg(long, value_delimiter = ',', default_value = "msb,lsb")]
        orders: Vec<String>,
        /// Also enumerate even/zero polynomials (degenerate CRC generators).
        #[arg(long, requires = "exhaustive")]
        include_even: bool,
        #[arg(long, default_value_t = 1)]
        workers: usize,
        #[arg(long, default_value_t = 0)]
        shard_index: usize,
        #[arg(long, default_value_t = 1)]
        shards: usize,
        /// Stop after at most this many new job attempts; resume with the same manifest.
        #[arg(long)]
        max_jobs: Option<usize>,
        #[arg(long, default_value_t = 30)]
        lease_seconds: u64,
        #[arg(long)]
        retry_failed: bool,
    },
    /// Show durable progress, failures and expired leases.
    Status {
        #[command(flatten)]
        database: Database,
    },
    /// Verify SQLite integrity, the complete inventory, payload digests and all saved DAGs.
    Verify {
        #[command(flatten)]
        database: Database,
    },
    /// Rank separately by CRC width, update width, and bit order.
    Rank {
        /// Rank 128-bit block updates separately from scalar updates.
        #[arg(long)]
        block16: bool,
        #[command(flatten)]
        database: Database,
        #[arg(long)]
        width: Option<u8>,
        #[arg(long)]
        data_bits: Option<u8>,
        #[arg(long)]
        order: Option<String>,
        /// Use only confirmed exact/lower HD bounds (dataword lengths, in bits).
        #[arg(long, requires = "min_length")]
        min_hd: Option<u32>,
        #[arg(long, requires = "min_hd")]
        min_length: Option<u64>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Re-rate saved DAGs using JSON operation costs and latencies.
        #[arg(long)]
        model: Option<PathBuf>,
    },
    /// Export the exact recorded DAG or standalone Rust for a completed job.
    Export {
        /// Export the stored 16-byte block DAG instead of a scalar variant.
        #[arg(long, conflicts_with = "variant")]
        block16: bool,
        #[command(flatten)]
        database: Database,
        #[arg(long)]
        job: usize,
        #[arg(long)]
        variant: Option<usize>,
        #[arg(long,default_value="rust",value_parser=["rust","graph","result"])]
        format: String,
    },
    /// Merge local databases from separate machines/shards. Stop destination workers first.
    Merge {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        from: PathBuf,
    },
    /// Produce a consistent portable backup, including committed WAL data.
    Snapshot {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

fn execute(command: Command) -> search::Result<()> {
    match command {
        Command::Run {
            db,
            resume,
            catalogue,
            exhaustive,
            width,
            mut data_bits,
            mut orders,
            include_even,
            workers,
            shard_index,
            shards,
            max_jobs,
            lease_seconds,
            retry_failed,
        } => {
            let manifest = if let Some(id) = resume {
                search::manifest(&db, Some(&id))?.1
            } else {
                let bytes = catalogue.as_ref().map(std::fs::read).transpose()?;
                let catalog = bytes
                    .as_ref()
                    .map(|bytes| Catalogue::parse(bytes))
                    .transpose()?;
                if let (Some(catalog), Some(path)) = (&catalog, &catalogue) {
                    catalog.verify_sources(
                        path.parent().unwrap_or_else(|| std::path::Path::new(".")),
                    )?;
                }
                let identity = catalog.as_ref().map(|catalog| search::CatalogueIdentity {
                    sha256: catalogue::sha256(bytes.as_ref().unwrap()),
                    sources: catalog.sources.clone(),
                });
                let mut width = if width.is_empty() {
                    if exhaustive {
                        vec![8, 16]
                    } else {
                        vec![32, 64]
                    }
                } else {
                    width
                };
                width.sort_unstable();
                width.dedup();
                let mut candidates = Vec::new();
                if exhaustive {
                    for width in width {
                        candidates.extend(catalogue::exhaustive(
                            width,
                            include_even,
                            catalog.as_ref(),
                        )?);
                    }
                } else {
                    let catalog = catalog.ok_or(
                        "provide --catalogue or --exhaustive (or --resume for an existing run)",
                    )?;
                    candidates.extend(
                        catalog
                            .candidates
                            .into_iter()
                            .filter(|candidate| width.contains(&candidate.width)),
                    );
                }
                data_bits.sort_unstable();
                data_bits.dedup();
                orders.sort();
                orders.dedup();
                candidates
                    .sort_by_key(|c| (c.width, catalogue::polynomial(&c.polynomial).unwrap_or(0)));
                Manifest {
                    version: 1,
                    engine: search::engine_fingerprint(),
                    optimizer: crc_gen::rating::OPTIMIZER.into(),
                    domain: if exhaustive {
                        "exhaustive"
                    } else {
                        "catalogue"
                    }
                    .into(),
                    include_even,
                    catalogue: identity,
                    candidates,
                    data_bits,
                    orders,
                }
            };
            manifest.validate()?;
            eprintln!(
                "run {}: {} jobs; {} workers; shard {}/{}",
                manifest.id()?,
                manifest.total(),
                workers,
                shard_index,
                shards
            );
            let status = search::run(
                &db,
                &manifest,
                &RunOptions {
                    workers,
                    shard_index,
                    shards,
                    max_jobs,
                    lease_seconds,
                    retry_failed,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&status)?);
            if status.failed > 0 {
                return Err(
                    "search has failed jobs; inspect status and resume with --retry-failed".into(),
                );
            }
        }
        Command::Status { database } => println!(
            "{}",
            serde_json::to_string_pretty(&search::status(
                &database.db,
                database.run_id.as_deref()
            )?)?
        ),
        Command::Verify { database } => println!(
            "Verified {} committed results",
            search::verify(&database.db, database.run_id.as_deref())?
        ),
        Command::Rank {
            block16,
            database,
            width,
            data_bits,
            order,
            min_hd,
            min_length,
            limit,
            model,
        } => {
            if let Some(order) = &order {
                crc_gen::rating::parse_order(order)?;
            }
            let model: CostModel = if let Some(path) = model {
                serde_json::from_slice(&std::fs::read(path)?)?
            } else {
                CostModel::default()
            };
            let result = search::rank(
                &database.db,
                database.run_id.as_deref(),
                &model,
                &RankOptions {
                    block16,
                    width,
                    bits: data_bits,
                    order,
                    min_hd,
                    min_length,
                    limit,
                },
            )?;
            let run_id = search::manifest(&database.db, database.run_id.as_deref())?.0;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"version":2,"run_id":run_id,"model_sha256":catalogue::sha256(&serde_json::to_vec(&model)?),"model":model,"groups":result})
                )?
            );
        }
        Command::Export {
            block16,
            database,
            job,
            variant,
            format,
        } => {
            let result = search::load(&database.db, database.run_id.as_deref(), job)?;
            if format == "result" {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                let graph = if block16 {
                    &result
                        .block16
                        .as_ref()
                        .ok_or("result has no 16-byte block graph")?
                        .graph
                } else {
                    let variant = variant.unwrap_or(result.best(&CostModel::default())?.0);
                    &result
                        .variants
                        .get(variant)
                        .ok_or("variant index out of range")?
                        .graph
                };
                if format == "graph" {
                    println!("{}", serde_json::to_string_pretty(graph)?);
                } else {
                    println!(
                        "// width={}, polynomial={}, data_bits={}, order={}, optimizer={}",
                        result.width,
                        result.polynomial,
                        if block16 { 128 } else { result.data_bits },
                        result.order,
                        result.optimizer
                    );
                    print!("{}", graph.rust()?);
                }
            }
        }
        Command::Merge { db, from } => {
            println!("Imported {} completed results", search::merge(&db, &from)?)
        }
        Command::Snapshot { db, output } => {
            search::snapshot(&db, &output)?;
            println!("Snapshot saved to {}", output.display());
        }
    }
    Ok(())
}

pub fn main() -> std::process::ExitCode {
    let cli = match Cli::try_parse_from(
        std::iter::once("crc-gen search".to_string()).chain(std::env::args().skip(2)),
    ) {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() {
                std::process::ExitCode::FAILURE
            } else {
                std::process::ExitCode::SUCCESS
            };
            let _ = error.print();
            return code;
        }
    };
    match execute(cli.command) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("crc-gen search: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
