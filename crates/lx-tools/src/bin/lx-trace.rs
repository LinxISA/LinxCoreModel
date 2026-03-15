use anyhow::Result;
use clap::Parser;
use isa::EngineKind;
use lx_tools::{execute, prepare_runtime};
use std::path::PathBuf;
use trace::write_linxtrace;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    engine: EngineKind,
    #[arg(long)]
    elf: PathBuf,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long, default_value = "out/lx-trace")]
    out_dir: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let prepared = prepare_runtime(&args.elf, &args.out_dir, args.config.as_ref())?;
    let bundle = execute(&prepared, args.engine)?;
    let path = prepared.out_dir.join("trace.linxtrace");
    write_linxtrace(&path, bundle.result(), bundle.stage_events())?;
    println!("{}", path.display());
    Ok(())
}
