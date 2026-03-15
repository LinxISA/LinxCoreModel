use anyhow::{Context, Result};
use camodel::{CycleEngine, CycleRunOptions};
use elf::load_static_elf;
use funcmodel::{FuncEngine, FuncRunOptions};
use isa::EngineKind;
use runtime::{GuestRuntime, RuntimeConfig};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepSpec {
    pub cases: Vec<SweepCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepCase {
    pub name: String,
    pub engine: EngineKind,
    pub elf: String,
    pub iterations: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepCaseReport {
    pub name: String,
    pub engine: EngineKind,
    pub iterations: usize,
    pub cycles: Vec<u64>,
    pub commits: Vec<u64>,
    pub exit_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepReport {
    pub cases: Vec<SweepCaseReport>,
}

pub fn load_sweep_spec(path: impl AsRef<Path>) -> Result<SweepSpec> {
    let text = fs::read_to_string(path.as_ref())
        .with_context(|| format!("failed to read {}", path.as_ref().display()))?;
    toml::from_str(&text).with_context(|| format!("failed to parse {}", path.as_ref().display()))
}

pub fn run_sweep(spec: &SweepSpec) -> Result<SweepReport> {
    let mut reports = Vec::new();
    let func = FuncEngine;
    let cycle = CycleEngine;
    for case in &spec.cases {
        let mut cycles = Vec::new();
        let mut commits = Vec::new();
        let mut exit_reason = String::new();
        for _ in 0..case.iterations {
            let image = load_static_elf(&case.elf)?;
            let runtime = GuestRuntime::bootstrap(image, RuntimeConfig::default())?;
            match case.engine {
                EngineKind::Func => {
                    let bundle = func.run(&runtime, &FuncRunOptions::default())?;
                    cycles.push(bundle.result.metrics.cycles);
                    commits.push(bundle.result.metrics.commits);
                    exit_reason = bundle.result.metrics.exit_reason;
                }
                EngineKind::Cycle => {
                    let bundle = cycle.run(&runtime, &CycleRunOptions::default())?;
                    cycles.push(bundle.result.metrics.cycles);
                    commits.push(bundle.result.metrics.commits);
                    exit_reason = bundle.result.metrics.exit_reason;
                }
            }
        }
        reports.push(SweepCaseReport {
            name: case.name.clone(),
            engine: case.engine,
            iterations: case.iterations,
            cycles,
            commits,
            exit_reason,
        });
    }
    Ok(SweepReport { cases: reports })
}

pub fn render_markdown(report: &SweepReport) -> String {
    let mut text =
        String::from("| Case | Engine | Iterations | Avg cycles | Avg commits | Exit |\n");
    text.push_str("|---|---|---:|---:|---:|---|\n");
    for case in &report.cases {
        let avg_cycles = if case.cycles.is_empty() {
            0.0
        } else {
            case.cycles.iter().sum::<u64>() as f64 / case.cycles.len() as f64
        };
        let avg_commits = if case.commits.is_empty() {
            0.0
        } else {
            case.commits.iter().sum::<u64>() as f64 / case.commits.len() as f64
        };
        text.push_str(&format!(
            "| {} | {:?} | {} | {:.1} | {:.1} | {} |\n",
            case.name, case.engine, case.iterations, avg_cycles, avg_commits, case.exit_reason
        ));
    }
    text
}
