use anyhow::{Context, Result};
use camodel::{CycleEngine, CycleRunBundle, CycleRunOptions};
use elf::load_static_elf;
use funcmodel::{FuncEngine, FuncRunBundle, FuncRunOptions};
use isa::{EngineKind, RunResult};
use runtime::{GuestRuntime, RuntimeConfig};
use std::path::{Path, PathBuf};

pub struct PreparedRun {
    pub runtime: GuestRuntime,
    pub out_dir: PathBuf,
}

pub enum PreparedBundle {
    Func(FuncRunBundle),
    Cycle(CycleRunBundle),
}

impl PreparedBundle {
    pub fn result(&self) -> &RunResult {
        match self {
            Self::Func(bundle) => &bundle.result,
            Self::Cycle(bundle) => &bundle.result,
        }
    }

    pub fn stage_events(&self) -> &[isa::StageTraceEvent] {
        match self {
            Self::Func(bundle) => &bundle.stage_events,
            Self::Cycle(bundle) => &bundle.stage_events,
        }
    }
}

pub enum EngineRunOptions {
    Func(FuncRunOptions),
    Cycle(CycleRunOptions),
}

impl Default for EngineRunOptions {
    fn default() -> Self {
        Self::Func(FuncRunOptions::default())
    }
}

pub fn prepare_runtime(
    elf: impl AsRef<Path>,
    out_dir: impl AsRef<Path>,
    config: Option<impl AsRef<Path>>,
) -> Result<PreparedRun> {
    let image = load_static_elf(&elf)?;
    let runtime_config = match config {
        Some(path) => RuntimeConfig::load(path)?,
        None => RuntimeConfig::default(),
    };
    let runtime = GuestRuntime::bootstrap(image, runtime_config)?;
    let out_dir = out_dir.as_ref().to_path_buf();
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;
    Ok(PreparedRun { runtime, out_dir })
}

pub fn execute(prepared: &PreparedRun, engine: EngineKind) -> Result<PreparedBundle> {
    execute_with_options(prepared, engine, None)
}

pub fn execute_with_options(
    prepared: &PreparedRun,
    engine: EngineKind,
    options: Option<EngineRunOptions>,
) -> Result<PreparedBundle> {
    match engine {
        EngineKind::Func => {
            let engine = FuncEngine;
            let options = match options {
                Some(EngineRunOptions::Func(opts)) => opts,
                _ => FuncRunOptions::default(),
            };
            Ok(PreparedBundle::Func(
                engine.run(&prepared.runtime, &options)?,
            ))
        }
        EngineKind::Cycle => {
            let engine = CycleEngine;
            let options = match options {
                Some(EngineRunOptions::Cycle(opts)) => opts,
                _ => CycleRunOptions::default(),
            };
            Ok(PreparedBundle::Cycle(
                engine.run(&prepared.runtime, &options)?,
            ))
        }
    }
}
