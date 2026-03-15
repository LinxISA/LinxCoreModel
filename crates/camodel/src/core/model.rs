use isa::{CommitRecord, DecodedInstruction};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{FRONTEND_STAGE_NAMES, IQ_CAPACITY, PHYS_IQ_COUNT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueWakeKind {
    T,
    U,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IqWakeKind {
    Spec,
    Nonspec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhysIq {
    AluIq0,
    SharedIq1,
    BruIq,
    AguIq0,
    AguIq1,
    StdIq0,
    StdIq1,
    CmdIq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QTag {
    pub(crate) phys_iq: PhysIq,
    pub(crate) entry_id: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LogicalQueueTag {
    pub(crate) kind: QueueWakeKind,
    pub(crate) tag: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchOwnerKind {
    None,
    Fall,
    Cond,
    Call,
    Ret,
    Direct,
    Ind,
    ICall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReturnConsumerKind {
    SetcTgt,
    FretRa,
    FretStk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallMaterializationKind {
    FusedCall,
    AdjacentSetret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DynamicTargetSourceKind {
    ArchTargetSetup,
    CallReturnFused,
    CallReturnAdjacentSetret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BranchOwnerContext {
    pub(crate) kind: BranchOwnerKind,
    pub(crate) base_pc: u64,
    pub(crate) target_pc: u64,
    pub(crate) off: u64,
    pub(crate) pred_take: bool,
    pub(crate) epoch: u16,
}

impl Default for BranchOwnerContext {
    fn default() -> Self {
        Self {
            kind: BranchOwnerKind::None,
            base_pc: 0,
            target_pc: 0,
            off: 0,
            pred_take: false,
            epoch: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IqWakeEvent {
    pub(crate) producer: usize,
    pub(crate) wake_kind: IqWakeKind,
    pub(crate) queue_kind: Option<QueueWakeKind>,
    pub(crate) logical_tag: Option<LogicalQueueTag>,
    pub(crate) qtag: Option<QTag>,
}

#[derive(Debug, Clone)]
pub(crate) struct CycleUop {
    pub(crate) decoded: DecodedInstruction,
    pub(crate) commit: CommitRecord,
    pub(crate) deps: [Option<usize>; 2],
    pub(crate) src_queue_kinds: [Option<QueueWakeKind>; 2],
    pub(crate) src_logical_tags: [Option<LogicalQueueTag>; 2],
    pub(crate) src_qtags: [Option<QTag>; 2],
    pub(crate) dst_queue_kind: Option<QueueWakeKind>,
    pub(crate) dst_logical_tag: Option<LogicalQueueTag>,
    pub(crate) dst_qtag: Option<QTag>,
    pub(crate) bypass_d2: bool,
    pub(crate) is_load: bool,
    pub(crate) is_store: bool,
    pub(crate) load_ordinal: Option<usize>,
    pub(crate) load_store_id: Option<usize>,
    pub(crate) miss_injected: bool,
    pub(crate) redirect_target: Option<u64>,
    pub(crate) phys_iq: Option<PhysIq>,
    pub(crate) pick_wakeup_visible: Option<u64>,
    pub(crate) data_ready_visible: Option<u64>,
    pub(crate) miss_pending_until: Option<u64>,
    pub(crate) e1_cycle: Option<u64>,
    pub(crate) e4_cycle: Option<u64>,
    pub(crate) w1_cycle: Option<u64>,
    pub(crate) done_cycle: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct IqEntry {
    pub(crate) seq: usize,
    pub(crate) phys_iq: PhysIq,
    pub(crate) inflight: bool,
    pub(crate) src_valid: [bool; 2],
    pub(crate) src_ready_nonspec: [bool; 2],
    pub(crate) src_ready_spec: [bool; 2],
    pub(crate) src_wait_qtag: [bool; 2],
}

impl PhysIq {
    pub(crate) fn index(self) -> usize {
        self as usize
    }

    pub(crate) fn lane_id(self) -> &'static str {
        match self {
            Self::AluIq0 => "alu_iq0",
            Self::SharedIq1 => "shared_iq1",
            Self::BruIq => "bru_iq",
            Self::AguIq0 => "agu_iq0",
            Self::AguIq1 => "agu_iq1",
            Self::StdIq0 => "std_iq0",
            Self::StdIq1 => "std_iq1",
            Self::CmdIq => "cmd_iq",
        }
    }

    pub(crate) fn capacity(self) -> usize {
        IQ_CAPACITY
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StageQueues {
    pub(crate) frontend: [VecDeque<usize>; FRONTEND_STAGE_NAMES.len()],
    pub(crate) frontend_redirect: Option<FrontendRedirectState>,
    pub(crate) pending_flush: Option<PendingFlushState>,
    pub(crate) flush_checkpoint_id: Option<u8>,
    pub(crate) seq_checkpoint_ids: BTreeMap<usize, u8>,
    pub(crate) seq_rob_checkpoint_ids: BTreeMap<usize, u8>,
    pub(crate) seq_recovery_checkpoint_ids: BTreeMap<usize, u8>,
    pub(crate) seq_recovery_epochs: BTreeMap<usize, u16>,
    pub(crate) seq_branch_contexts: BTreeMap<usize, BranchOwnerContext>,
    pub(crate) seq_dynamic_target_pcs: BTreeMap<usize, u64>,
    pub(crate) seq_boundary_target_pcs: BTreeMap<usize, u64>,
    pub(crate) seq_boundary_target_owner_seqs: BTreeMap<usize, usize>,
    pub(crate) seq_boundary_target_producer_kinds: BTreeMap<usize, ReturnConsumerKind>,
    pub(crate) seq_boundary_target_setup_epochs: BTreeMap<usize, u16>,
    pub(crate) seq_boundary_target_source_owner_seqs: BTreeMap<usize, usize>,
    pub(crate) seq_boundary_target_source_epochs: BTreeMap<usize, u16>,
    pub(crate) seq_boundary_target_source_kinds: BTreeMap<usize, DynamicTargetSourceKind>,
    pub(crate) seq_return_consumer_kinds: BTreeMap<usize, ReturnConsumerKind>,
    pub(crate) seq_call_return_target_pcs: BTreeMap<usize, u64>,
    pub(crate) seq_call_return_target_owner_seqs: BTreeMap<usize, usize>,
    pub(crate) seq_call_return_target_epochs: BTreeMap<usize, u16>,
    pub(crate) seq_call_materialization_kinds: BTreeMap<usize, CallMaterializationKind>,
    pub(crate) seq_call_header_faults: BTreeMap<usize, u64>,
    pub(crate) active_recovery_checkpoint_id: u8,
    pub(crate) active_recovery_epoch: u16,
    pub(crate) active_block_head: bool,
    pub(crate) active_branch_context: BranchOwnerContext,
    pub(crate) active_dynamic_target_pc: Option<u64>,
    pub(crate) active_dynamic_target_owner_seq: Option<usize>,
    pub(crate) active_dynamic_target_producer_kind: Option<ReturnConsumerKind>,
    pub(crate) active_dynamic_target_setup_epoch: Option<u16>,
    pub(crate) active_dynamic_target_owner_kind: Option<ReturnConsumerKind>,
    pub(crate) active_dynamic_target_source_owner_seq: Option<usize>,
    pub(crate) active_dynamic_target_source_epoch: Option<u16>,
    pub(crate) active_dynamic_target_source_kind: Option<DynamicTargetSourceKind>,
    pub(crate) active_dynamic_target_call_materialization_kind: Option<CallMaterializationKind>,
    pub(crate) active_call_header_seq: Option<usize>,
    pub(crate) active_call_return_target_pc: Option<u64>,
    pub(crate) active_call_return_target_owner_seq: Option<usize>,
    pub(crate) active_call_return_target_epoch: Option<u16>,
    pub(crate) active_call_return_materialization_kind: Option<CallMaterializationKind>,
    pub(crate) ready_table_checkpoints: BTreeMap<u8, ReadyTableCheckpoint>,
    pub(crate) pending_bru_correction: Option<BruCorrectionState>,
    pub(crate) pending_trap: Option<PendingTrapState>,
    pub(crate) iq_tags: BTreeMap<usize, QTag>,
    pub(crate) iq_owner_table: Vec<Vec<Option<usize>>>,
    pub(crate) qtag_wait_crossbar: Vec<Vec<Vec<(usize, usize)>>>,
    pub(crate) ready_table_t: BTreeSet<usize>,
    pub(crate) ready_table_u: BTreeSet<usize>,
    pub(crate) liq: VecDeque<LiqEntry>,
    pub(crate) lhq: VecDeque<usize>,
    pub(crate) mdb: VecDeque<MdbEntry>,
    pub(crate) stq: VecDeque<usize>,
    pub(crate) scb: VecDeque<ScbEntry>,
    pub(crate) l1d: VecDeque<L1dEntry>,
    pub(crate) p1: VecDeque<usize>,
    pub(crate) i1: VecDeque<usize>,
    pub(crate) i2: VecDeque<usize>,
    pub(crate) e1: VecDeque<usize>,
    pub(crate) e2: VecDeque<usize>,
    pub(crate) e3: VecDeque<usize>,
    pub(crate) e4: VecDeque<usize>,
    pub(crate) w1: VecDeque<usize>,
    pub(crate) w2: VecDeque<usize>,
    pub(crate) lsid_issue_ptr: usize,
    pub(crate) lsid_complete_ptr: usize,
    pub(crate) lsid_cache_ptr: usize,
}

impl Default for StageQueues {
    fn default() -> Self {
        Self {
            frontend: Default::default(),
            frontend_redirect: None,
            pending_flush: None,
            flush_checkpoint_id: None,
            seq_checkpoint_ids: Default::default(),
            seq_rob_checkpoint_ids: Default::default(),
            seq_recovery_checkpoint_ids: Default::default(),
            seq_recovery_epochs: Default::default(),
            seq_branch_contexts: Default::default(),
            seq_dynamic_target_pcs: Default::default(),
            seq_boundary_target_pcs: Default::default(),
            seq_boundary_target_owner_seqs: Default::default(),
            seq_boundary_target_producer_kinds: Default::default(),
            seq_boundary_target_setup_epochs: Default::default(),
            seq_boundary_target_source_owner_seqs: Default::default(),
            seq_boundary_target_source_epochs: Default::default(),
            seq_boundary_target_source_kinds: Default::default(),
            seq_return_consumer_kinds: Default::default(),
            seq_call_return_target_pcs: Default::default(),
            seq_call_return_target_owner_seqs: Default::default(),
            seq_call_return_target_epochs: Default::default(),
            seq_call_materialization_kinds: Default::default(),
            seq_call_header_faults: Default::default(),
            active_recovery_checkpoint_id: 0,
            active_recovery_epoch: 0,
            active_block_head: true,
            active_branch_context: BranchOwnerContext::default(),
            active_dynamic_target_pc: None,
            active_dynamic_target_owner_seq: None,
            active_dynamic_target_producer_kind: None,
            active_dynamic_target_setup_epoch: None,
            active_dynamic_target_owner_kind: None,
            active_dynamic_target_source_owner_seq: None,
            active_dynamic_target_source_epoch: None,
            active_dynamic_target_source_kind: None,
            active_dynamic_target_call_materialization_kind: None,
            active_call_header_seq: None,
            active_call_return_target_pc: None,
            active_call_return_target_owner_seq: None,
            active_call_return_target_epoch: None,
            active_call_return_materialization_kind: None,
            ready_table_checkpoints: Default::default(),
            pending_bru_correction: None,
            pending_trap: None,
            iq_tags: Default::default(),
            iq_owner_table: empty_iq_owner_table(),
            qtag_wait_crossbar: empty_qtag_wait_crossbar(),
            ready_table_t: Default::default(),
            ready_table_u: Default::default(),
            liq: Default::default(),
            lhq: Default::default(),
            mdb: Default::default(),
            stq: Default::default(),
            scb: Default::default(),
            l1d: Default::default(),
            p1: Default::default(),
            i1: Default::default(),
            i2: Default::default(),
            e1: Default::default(),
            e2: Default::default(),
            e3: Default::default(),
            e4: Default::default(),
            w1: Default::default(),
            w2: Default::default(),
            lsid_issue_ptr: 0,
            lsid_complete_ptr: 0,
            lsid_cache_ptr: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadyTableCheckpoint {
    pub(crate) ready_table_t: BTreeSet<usize>,
    pub(crate) ready_table_u: BTreeSet<usize>,
    pub(crate) recovery_epoch: u16,
    pub(crate) block_head: bool,
    pub(crate) branch_context: BranchOwnerContext,
    pub(crate) dynamic_target_pc: Option<u64>,
    pub(crate) dynamic_target_owner_seq: Option<usize>,
    pub(crate) dynamic_target_producer_kind: Option<ReturnConsumerKind>,
    pub(crate) dynamic_target_setup_epoch: Option<u16>,
    pub(crate) dynamic_target_owner_kind: Option<ReturnConsumerKind>,
    pub(crate) dynamic_target_source_owner_seq: Option<usize>,
    pub(crate) dynamic_target_source_epoch: Option<u16>,
    pub(crate) dynamic_target_source_kind: Option<DynamicTargetSourceKind>,
    pub(crate) dynamic_target_call_materialization_kind: Option<CallMaterializationKind>,
    pub(crate) call_header_seq: Option<usize>,
    pub(crate) call_return_target_pc: Option<u64>,
    pub(crate) call_return_target_owner_seq: Option<usize>,
    pub(crate) call_return_target_epoch: Option<u16>,
    pub(crate) call_return_materialization_kind: Option<CallMaterializationKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrontendRedirectState {
    pub(crate) source_seq: usize,
    pub(crate) target_pc: u64,
    pub(crate) restart_seq: usize,
    pub(crate) checkpoint_id: u8,
    pub(crate) from_correction: bool,
    pub(crate) resume_cycle: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingFlushState {
    pub(crate) flush_seq: usize,
    pub(crate) checkpoint_id: u8,
    pub(crate) apply_cycle: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BruCorrectionState {
    pub(crate) source_seq: usize,
    pub(crate) epoch: u16,
    pub(crate) actual_take: bool,
    pub(crate) target_pc: u64,
    pub(crate) checkpoint_id: u8,
    pub(crate) visible_cycle: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingTrapState {
    pub(crate) seq: usize,
    pub(crate) cause: u64,
    pub(crate) traparg0: u64,
    pub(crate) checkpoint_id: u8,
    pub(crate) visible_cycle: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct LiqEntry {
    pub(crate) seq: usize,
    pub(crate) refill_ready_cycle: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct MdbEntry {
    pub(crate) seq: usize,
    pub(crate) refill_ready_cycle: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct ScbEntry {
    pub(crate) seq: usize,
    pub(crate) enqueue_cycle: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum L1dTxnKind {
    LoadHit,
    StoreDrain,
}

#[derive(Debug, Clone)]
pub(crate) struct L1dEntry {
    pub(crate) seq: usize,
    pub(crate) kind: L1dTxnKind,
    pub(crate) ready_cycle: u64,
}

pub(crate) fn empty_qtag_wait_crossbar() -> Vec<Vec<Vec<(usize, usize)>>> {
    vec![vec![Vec::new(); IQ_CAPACITY]; PHYS_IQ_COUNT]
}

pub(crate) fn empty_iq_owner_table() -> Vec<Vec<Option<usize>>> {
    vec![vec![None; IQ_CAPACITY]; PHYS_IQ_COUNT]
}
