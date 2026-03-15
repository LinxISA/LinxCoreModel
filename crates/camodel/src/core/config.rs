use serde::{Deserialize, Serialize};

use isa::{RunResult, StageTraceEvent};

pub(crate) const FETCH_WIDTH: usize = 4;
pub(crate) const DISPATCH_WIDTH: usize = 4;
pub(crate) const ISSUE_WIDTH: usize = 4;
pub(crate) const COMMIT_WIDTH: usize = 4;
pub(crate) const READ_PORTS: usize = 3;
pub(crate) const ROB_CAPACITY: usize = 128;
pub(crate) const PHYS_IQ_COUNT: usize = 8;
pub(crate) const IQ_CAPACITY: usize = 32;
pub(crate) const IQ_ENQUEUE_PORTS: usize = 2;
pub(crate) const LSU_WIDTH: usize = 1;
pub(crate) const L1D_WIDTH: usize = 1;
pub(crate) const FRONTEND_REDIRECT_RESTART_DELAY: u64 = 1;
pub(crate) const REG_T1: u8 = 24;
pub(crate) const REG_U1: u8 = 28;
pub(crate) const LD_GEN_E1: u8 = 1 << 0;
pub(crate) const LD_GEN_E2: u8 = 1 << 1;
pub(crate) const LD_GEN_E3: u8 = 1 << 2;
pub(crate) const LD_GEN_E4: u8 = 1 << 3;

pub(crate) const FRONTEND_STAGE_NAMES: [&str; 11] = [
    "F0", "F1", "F2", "F3", "IB", "F4", "D1", "D2", "D3", "S1", "S2",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleRunOptions {
    pub max_cycles: u64,
    pub load_miss_every: Option<u64>,
    pub load_miss_penalty: u64,
}

impl Default for CycleRunOptions {
    fn default() -> Self {
        Self {
            max_cycles: 256,
            load_miss_every: None,
            load_miss_penalty: 8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleRunBundle {
    pub result: RunResult,
    pub stage_events: Vec<StageTraceEvent>,
}

#[derive(Debug, Default)]
pub struct CycleEngine;
