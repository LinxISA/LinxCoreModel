use anyhow::Result;
use camodel::CycleRunOptions;
use clap::Parser;
use funcmodel::FuncRunOptions;
use isa::EngineKind;
use lx_tools::{EngineRunOptions, execute_with_options, prepare_runtime};
use std::path::PathBuf;
use trace::{write_commit_jsonl, write_linxtrace};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    engine: EngineKind,
    #[arg(long)]
    elf: PathBuf,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long, default_value_t = 100_000)]
    max_steps: u64,
    #[arg(long, default_value_t = 64)]
    max_cycles: u64,
    #[arg(long, default_value = "out/lx-run")]
    out_dir: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let prepared = prepare_runtime(&args.elf, &args.out_dir, args.config.as_ref())?;
    let run_options = match args.engine {
        EngineKind::Func => EngineRunOptions::Func(FuncRunOptions {
            max_steps: args.max_steps,
        }),
        EngineKind::Cycle => EngineRunOptions::Cycle(CycleRunOptions {
            max_cycles: args.max_cycles,
            ..CycleRunOptions::default()
        }),
    };
    let bundle = execute_with_options(&prepared, args.engine, Some(run_options))?;
    write_commit_jsonl(prepared.out_dir.join("commit.jsonl"), bundle.result())?;
    write_linxtrace(
        prepared.out_dir.join("trace.linxtrace"),
        bundle.result(),
        bundle.stage_events(),
    )?;
    println!("{}", serde_json::to_string_pretty(bundle.result())?);
    Ok(())
}
