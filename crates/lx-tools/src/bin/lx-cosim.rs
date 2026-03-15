use anyhow::{Result, bail};
use clap::Parser;
use cosim::{compare_commit_streams, load_commit_jsonl, require_cosim_match};
use isa::EngineKind;
use lx_tools::{execute, prepare_runtime};
use std::path::PathBuf;
use trace::write_commit_jsonl;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    engine: EngineKind,
    #[arg(long)]
    elf: PathBuf,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    qemu: PathBuf,
    #[arg(long, default_value = "m1")]
    protocol: String,
    #[arg(long, default_value = "out/lx-cosim")]
    out_dir: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.protocol != "m1" {
        bail!("unsupported protocol {}; expected m1", args.protocol);
    }
    let prepared = prepare_runtime(&args.elf, &args.out_dir, args.config.as_ref())?;
    let bundle = execute(&prepared, args.engine)?;
    let dut_trace = prepared.out_dir.join("dut.commit.jsonl");
    write_commit_jsonl(&dut_trace, bundle.result())?;
    let qemu_trace = load_commit_jsonl(&args.qemu)?;
    let report = compare_commit_streams(&qemu_trace, &bundle.result().commits);
    require_cosim_match(&report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
