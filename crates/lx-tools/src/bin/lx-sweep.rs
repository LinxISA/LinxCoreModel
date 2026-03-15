use anyhow::Result;
use clap::Parser;
use dse::{load_sweep_spec, render_markdown, run_sweep};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    suite: PathBuf,
    #[arg(long, default_value = "out/lx-sweep")]
    out_dir: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    fs::create_dir_all(&args.out_dir)?;
    let spec = load_sweep_spec(&args.suite)?;
    let report = run_sweep(&spec)?;
    fs::write(
        args.out_dir.join("report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    fs::write(args.out_dir.join("report.md"), render_markdown(&report))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
