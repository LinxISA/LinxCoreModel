pub mod bru;
pub mod dynamic_target;
pub mod fls;

use std::collections::{BTreeSet, VecDeque};

use isa::{
    TRAP_BRU_RECOVERY_NOT_BSTART, TRAP_DYNAMIC_TARGET_MISSING, TRAP_DYNAMIC_TARGET_NOT_BSTART,
    TRAP_DYNAMIC_TARGET_STALE,
};

use crate::{
    BranchOwnerKind, BruCorrectionState, CycleUop, FrontendRedirectState, IqEntry,
    PendingFlushState, PendingTrapState, StageQueues, branch_context_for_seq,
    deferred_bru_correction_target, is_boundary_redirect_owner, legal_redirect_restart_seq,
    live_boundary_target_for_seq, live_branch_kind_for_seq, live_rob_checkpoint_id_for_seq,
    prune_iq_wait_crossbar_on_redirect, rebuild_iq_owner_table, recovery_checkpoint_id_for_seq,
    recovery_epoch_for_seq, restore_ready_tables_for_checkpoint,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedFrontendRedirect {
    pub(crate) source_seq: usize,
    pub(crate) target_pc: u64,
    pub(crate) checkpoint_id: u8,
    pub(crate) from_correction: bool,
}

pub(crate) fn redirect_resolve_cycle(uop: &CycleUop) -> Option<u64> {
    if uop.redirect_target.is_some() {
        uop.w1_cycle.or(uop.done_cycle)
    } else {
        None
    }
}

pub(crate) fn unresolved_redirect_barrier(
    next_fetch_seq: usize,
    uops: &[CycleUop],
) -> Option<usize> {
    (0..next_fetch_seq).find(|&seq| {
        uops[seq].redirect_target.is_some() && redirect_resolve_cycle(&uops[seq]).is_none()
    })
}

pub(crate) fn publish_bru_correction_state(
    cycle: u64,
    pipeline: &mut StageQueues,
    uops: &[CycleUop],
) {
    let fault = uops
        .iter()
        .enumerate()
        .filter_map(|(seq, _uop)| {
            let candidate = bru_correction_candidate(seq, cycle, pipeline, uops)?;
            if !candidate.actual_take {
                return None;
            }
            legal_redirect_restart_seq(seq, candidate.target_pc, uops)
                .is_none()
                .then_some(PendingTrapState {
                    seq,
                    cause: TRAP_BRU_RECOVERY_NOT_BSTART,
                    traparg0: uops[seq].commit.pc,
                    checkpoint_id: candidate.checkpoint_id,
                    visible_cycle: cycle,
                })
        })
        .min_by_key(|trap| trap.seq);

    if let Some(fault) = fault {
        pipeline.pending_trap = Some(match pipeline.pending_trap {
            Some(active) if active.seq < fault.seq => active,
            _ => fault,
        });
        return;
    }

    let next = uops
        .iter()
        .enumerate()
        .filter_map(|(seq, _uop)| bru_correction_candidate(seq, cycle, pipeline, uops))
        .map(|candidate| BruCorrectionState {
            source_seq: candidate.source_seq,
            epoch: candidate.epoch,
            actual_take: candidate.actual_take,
            target_pc: candidate.target_pc,
            checkpoint_id: candidate.checkpoint_id,
            visible_cycle: candidate.visible_cycle,
        })
        .max_by_key(|state| (state.epoch, state.source_seq));

    if let Some(next) = next {
        pipeline.pending_bru_correction = Some(match pipeline.pending_bru_correction {
            Some(active) if active.source_seq > next.source_seq => active,
            _ => next,
        });
    }
}

pub(crate) fn publish_dynamic_boundary_target_fault_state(
    cycle: u64,
    pipeline: &mut StageQueues,
    uops: &[CycleUop],
) {
    let fault = uops
        .iter()
        .enumerate()
        .filter_map(|(seq, _uop)| dynamic_boundary_target_fault(seq, cycle, pipeline, uops))
        .min_by_key(|trap| trap.seq);

    if let Some(fault) = fault {
        pipeline.pending_trap = Some(match pipeline.pending_trap {
            Some(active) if active.seq < fault.seq => active,
            _ => fault,
        });
    }
}

pub(crate) fn publish_call_header_fault_state(
    cycle: u64,
    pipeline: &mut StageQueues,
    uops: &[CycleUop],
) {
    let fault = pipeline
        .seq_call_header_faults
        .iter()
        .filter_map(|(&seq, &cause)| {
            let uop = uops.get(seq)?;
            let visible_cycle = uop.w1_cycle.or(uop.done_cycle)?;
            (visible_cycle == cycle).then_some(PendingTrapState {
                seq,
                cause,
                traparg0: uop.commit.pc,
                checkpoint_id: recovery_checkpoint_id_for_seq(seq, pipeline, uops),
                visible_cycle: cycle,
            })
        })
        .min_by_key(|trap| trap.seq);

    if let Some(fault) = fault {
        pipeline.pending_trap = Some(match pipeline.pending_trap {
            Some(active) if active.seq < fault.seq => active,
            _ => fault,
        });
    }
}

pub(crate) fn prune_speculative_state_on_redirect(
    cycle: u64,
    pipeline: &mut StageQueues,
    iq: &mut Vec<IqEntry>,
    rob: &mut VecDeque<usize>,
    uops: &[CycleUop],
) {
    let Some(flush_seq) = active_flush(cycle, pipeline, uops).map(|flush| flush.flush_seq) else {
        return;
    };

    for queue in &mut pipeline.frontend {
        queue.retain(|&seq| seq <= flush_seq);
    }
    pipeline.p1.retain(|&seq| seq <= flush_seq);
    pipeline.i1.retain(|&seq| seq <= flush_seq);
    pipeline.i2.retain(|&seq| seq <= flush_seq);
    pipeline.e1.retain(|&seq| seq <= flush_seq);
    pipeline.e2.retain(|&seq| seq <= flush_seq);
    pipeline.e3.retain(|&seq| seq <= flush_seq);
    pipeline.e4.retain(|&seq| seq <= flush_seq);
    pipeline.w1.retain(|&seq| seq <= flush_seq);
    pipeline.w2.retain(|&seq| seq <= flush_seq);
    pipeline.iq_tags.retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_checkpoint_ids
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_rob_checkpoint_ids
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_recovery_checkpoint_ids
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_recovery_epochs
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_dynamic_target_pcs
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_boundary_target_pcs
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_boundary_target_owner_seqs
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_boundary_target_producer_kinds
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_boundary_target_setup_epochs
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_boundary_target_source_owner_seqs
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_boundary_target_source_epochs
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_boundary_target_source_kinds
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_return_consumer_kinds
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_call_return_target_pcs
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_call_return_target_owner_seqs
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_call_return_target_epochs
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_call_materialization_kinds
        .retain(|&seq, _| seq <= flush_seq);
    pipeline
        .seq_call_header_faults
        .retain(|&seq, _| seq <= flush_seq);
    prune_iq_wait_crossbar_on_redirect(&mut pipeline.qtag_wait_crossbar, flush_seq);
    iq.retain(|entry| entry.seq <= flush_seq);
    rebuild_iq_owner_table(&mut pipeline.iq_owner_table, iq, &pipeline.iq_tags);
    rob.retain(|&seq| seq <= flush_seq);
}

pub(crate) fn schedule_frontend_redirect_recovery(
    cycle: u64,
    pipeline: &mut StageQueues,
    uops: &[CycleUop],
) {
    let Some(redirect) = resolved_frontend_redirect(cycle, pipeline, uops) else {
        return;
    };
    let restart_seq = legal_redirect_restart_seq(redirect.source_seq, redirect.target_pc, uops)
        .unwrap_or_else(|| redirect.source_seq.saturating_add(1));

    let next = FrontendRedirectState {
        source_seq: redirect.source_seq,
        target_pc: redirect.target_pc,
        restart_seq,
        checkpoint_id: redirect.checkpoint_id,
        from_correction: redirect.from_correction,
        resume_cycle: cycle.saturating_add(crate::FRONTEND_REDIRECT_RESTART_DELAY),
    };
    pipeline.frontend_redirect = Some(match pipeline.frontend_redirect {
        Some(active) if active.resume_cycle >= next.resume_cycle => active,
        _ => next,
    });
    pipeline.flush_checkpoint_id = Some(redirect.checkpoint_id);
    pipeline.pending_flush = Some(match pipeline.pending_flush {
        Some(active)
            if active.apply_cycle
                <= cycle.saturating_add(crate::FRONTEND_REDIRECT_RESTART_DELAY) =>
        {
            active
        }
        _ => PendingFlushState {
            flush_seq: redirect.source_seq,
            checkpoint_id: redirect.checkpoint_id,
            apply_cycle: cycle.saturating_add(crate::FRONTEND_REDIRECT_RESTART_DELAY),
        },
    });
    if redirect.from_correction {
        pipeline.pending_bru_correction = None;
    } else if pipeline.pending_bru_correction.is_some_and(|pending| {
        pending.epoch < recovery_epoch_for_seq(redirect.source_seq, pipeline, uops)
    }) {
        pipeline.pending_bru_correction = None;
    }
}

pub(crate) fn prune_memory_owner_state_on_redirect(
    cycle: u64,
    pipeline: &mut StageQueues,
    uops: &[CycleUop],
) {
    let Some(flush_seq) = active_flush(cycle, pipeline, uops).map(|flush| flush.flush_seq) else {
        return;
    };

    pipeline.stq.retain(|&seq| seq <= flush_seq);
    pipeline.lhq.retain(|&seq| seq <= flush_seq);
    pipeline.liq.retain(|entry| entry.seq <= flush_seq);
    pipeline.mdb.retain(|entry| entry.seq <= flush_seq);
    pipeline.scb.retain(|entry| entry.seq <= flush_seq);
    pipeline.l1d.retain(|entry| entry.seq <= flush_seq);
}

pub(crate) fn rebase_lsid_on_redirect(
    cycle: u64,
    pipeline: &mut StageQueues,
    iq: &[IqEntry],
    rob: &VecDeque<usize>,
    uops: &[CycleUop],
) {
    if active_flush(cycle, pipeline, uops).is_none() {
        return;
    }

    if let Some(head) = surviving_unissued_lsid_head(pipeline, iq, rob, uops) {
        pipeline.lsid_issue_ptr = head;
        pipeline.lsid_complete_ptr = head;
    }
    if let Some(head) = surviving_active_lsid_head(pipeline, iq, rob, uops) {
        pipeline.lsid_cache_ptr = head;
    }
}

pub(crate) fn apply_pending_flush(
    cycle: u64,
    pipeline: &mut StageQueues,
    iq: &mut Vec<IqEntry>,
    rob: &mut VecDeque<usize>,
    uops: &[CycleUop],
) {
    let Some(pending_flush) = pipeline.pending_flush else {
        return;
    };
    if pending_flush.apply_cycle > cycle {
        return;
    }
    restore_ready_tables_for_checkpoint(pipeline, pending_flush.checkpoint_id);
    pipeline.active_recovery_checkpoint_id = pending_flush.checkpoint_id;
    prune_speculative_state_on_redirect(cycle, pipeline, iq, rob, uops);
    prune_memory_owner_state_on_redirect(cycle, pipeline, uops);
    rebase_lsid_on_redirect(cycle, pipeline, iq, rob, uops);
    pipeline.pending_flush = None;
}

pub(crate) fn resolved_frontend_redirect(
    cycle: u64,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> Option<ResolvedFrontendRedirect> {
    let source_seq = resolved_boundary_seq(cycle, uops)?;
    if pipeline
        .pending_trap
        .is_some_and(|pending| pending.seq == source_seq && pending.visible_cycle <= cycle)
    {
        return None;
    }
    let boundary_epoch = recovery_epoch_for_seq(source_seq, pipeline, uops);
    if let Some(pending) = pipeline
        .pending_bru_correction
        .filter(|pending| pending.source_seq < source_seq && pending.epoch == boundary_epoch)
    {
        return Some(ResolvedFrontendRedirect {
            source_seq,
            target_pc: if pending.actual_take {
                pending.target_pc
            } else {
                fallthrough_pc(&uops[source_seq].commit)
            },
            checkpoint_id: pending.checkpoint_id,
            from_correction: true,
        });
    }
    crate::live_boundary_target_for_seq(source_seq, pipeline, uops).map(|target_pc| {
        ResolvedFrontendRedirect {
            source_seq,
            target_pc,
            checkpoint_id: live_rob_checkpoint_id_for_seq(source_seq, pipeline, uops),
            from_correction: false,
        }
    })
}

fn resolved_boundary_seq(cycle: u64, uops: &[CycleUop]) -> Option<usize> {
    uops.iter()
        .enumerate()
        .filter_map(|(seq, uop)| (boundary_resolve_cycle(uop) == Some(cycle)).then_some(seq))
        .min()
}

fn active_flush(
    cycle: u64,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> Option<PendingFlushState> {
    pipeline
        .pending_flush
        .filter(|pending| pending.apply_cycle <= cycle)
        .or_else(|| {
            resolved_frontend_redirect(cycle, pipeline, uops).map(|redirect| PendingFlushState {
                flush_seq: redirect.source_seq,
                checkpoint_id: redirect.checkpoint_id,
                apply_cycle: cycle,
            })
        })
}

fn boundary_resolve_cycle(uop: &CycleUop) -> Option<u64> {
    is_boundary_redirect_owner(&uop.decoded)
        .then_some(uop.w1_cycle.or(uop.done_cycle))
        .flatten()
}

fn dynamic_boundary_target_fault(
    seq: usize,
    cycle: u64,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> Option<PendingTrapState> {
    let uop = uops.get(seq)?;
    if boundary_resolve_cycle(uop)? != cycle {
        return None;
    }
    let kind = live_branch_kind_for_seq(seq, pipeline, uops)?;
    if !matches!(
        kind,
        BranchOwnerKind::Ret | BranchOwnerKind::Ind | BranchOwnerKind::ICall
    ) {
        return None;
    }

    let target_pc = live_boundary_target_for_seq(seq, pipeline, uops);
    let setup_epoch = pipeline.seq_boundary_target_setup_epochs.get(&seq).copied();
    let boundary_epoch = recovery_epoch_for_seq(seq, pipeline, uops);
    let cause = match target_pc {
        None => TRAP_DYNAMIC_TARGET_MISSING,
        Some(_) if setup_epoch.is_some_and(|setup_epoch| setup_epoch != boundary_epoch) => {
            TRAP_DYNAMIC_TARGET_STALE
        }
        Some(target_pc) if legal_redirect_restart_seq(seq, target_pc, uops).is_none() => {
            TRAP_DYNAMIC_TARGET_NOT_BSTART
        }
        Some(_) => return None,
    };
    Some(PendingTrapState {
        seq,
        cause,
        traparg0: uop.commit.pc,
        checkpoint_id: live_rob_checkpoint_id_for_seq(seq, pipeline, uops),
        visible_cycle: cycle,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BruCorrectionCandidate {
    source_seq: usize,
    epoch: u16,
    actual_take: bool,
    target_pc: u64,
    checkpoint_id: u8,
    visible_cycle: u64,
}

fn bru_correction_candidate(
    seq: usize,
    cycle: u64,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> Option<BruCorrectionCandidate> {
    let uop = uops.get(seq)?;
    let visible_cycle = uop.w1_cycle.or(uop.done_cycle)?;
    if visible_cycle != cycle {
        return None;
    }
    let has_live_branch_context = pipeline.seq_branch_contexts.contains_key(&seq);
    let branch_context = branch_context_for_seq(seq, pipeline, uops);
    let actual_take = bru_actual_take(uop)?;
    let target_pc = bru_target_pc(uop, branch_context, actual_take, has_live_branch_context)?;
    let mismatch = if matches!(
        branch_context.kind,
        BranchOwnerKind::Cond | BranchOwnerKind::Ret
    ) {
        actual_take != branch_context.pred_take
    } else if has_live_branch_context {
        false
    } else {
        actual_take
    };
    mismatch.then_some(BruCorrectionCandidate {
        source_seq: seq,
        epoch: recovery_epoch_for_seq(seq, pipeline, uops),
        actual_take,
        target_pc,
        checkpoint_id: recovery_checkpoint_id_for_seq(seq, pipeline, uops),
        visible_cycle,
    })
}

fn bru_actual_take(uop: &CycleUop) -> Option<bool> {
    (uop.commit.trap_valid == 0
        && uop.decoded.uop_group == "BRU"
        && !is_boundary_redirect_owner(&uop.decoded))
    .then_some(uop.commit.next_pc != fallthrough_pc(&uop.commit))
}

fn bru_target_pc(
    uop: &CycleUop,
    branch_context: crate::BranchOwnerContext,
    actual_take: bool,
    has_live_branch_context: bool,
) -> Option<u64> {
    if branch_context.kind != BranchOwnerKind::None {
        return Some(branch_context.target_pc);
    }
    if has_live_branch_context {
        return None;
    }
    actual_take
        .then(|| deferred_bru_correction_target(&uop.commit, &uop.decoded))
        .flatten()
}

fn fallthrough_pc(commit: &isa::CommitRecord) -> u64 {
    commit.pc.saturating_add(commit.len as u64)
}

fn surviving_unissued_lsid_head(
    pipeline: &StageQueues,
    iq: &[IqEntry],
    rob: &VecDeque<usize>,
    uops: &[CycleUop],
) -> Option<usize> {
    active_unissued_memory_seqs(pipeline, iq, rob)
        .into_iter()
        .filter_map(|seq| uops.get(seq).and_then(|uop| uop.load_store_id))
        .min()
}

fn surviving_active_lsid_head(
    pipeline: &StageQueues,
    iq: &[IqEntry],
    rob: &VecDeque<usize>,
    uops: &[CycleUop],
) -> Option<usize> {
    active_memory_seqs(pipeline, iq, rob)
        .into_iter()
        .filter_map(|seq| uops.get(seq).and_then(|uop| uop.load_store_id))
        .min()
}

fn active_unissued_memory_seqs(
    pipeline: &StageQueues,
    iq: &[IqEntry],
    rob: &VecDeque<usize>,
) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    out.extend(rob.iter().copied());
    out.extend(iq.iter().map(|entry| entry.seq));
    out.extend(
        pipeline
            .frontend
            .iter()
            .flat_map(|queue| queue.iter().copied()),
    );
    out.extend(pipeline.p1.iter().copied());
    out.extend(pipeline.i1.iter().copied());
    out.extend(pipeline.i2.iter().copied());

    for seq in issued_memory_seqs(pipeline) {
        out.remove(&seq);
    }
    out
}

fn active_memory_seqs(
    pipeline: &StageQueues,
    iq: &[IqEntry],
    rob: &VecDeque<usize>,
) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    out.extend(rob.iter().copied());
    out.extend(iq.iter().map(|entry| entry.seq));
    out.extend(
        pipeline
            .frontend
            .iter()
            .flat_map(|queue| queue.iter().copied()),
    );
    out.extend(pipeline.p1.iter().copied());
    out.extend(pipeline.i1.iter().copied());
    out.extend(pipeline.i2.iter().copied());
    out.extend(issued_memory_seqs(pipeline));
    out
}

fn issued_memory_seqs(pipeline: &StageQueues) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    out.extend(pipeline.e1.iter().copied());
    out.extend(pipeline.e2.iter().copied());
    out.extend(pipeline.e3.iter().copied());
    out.extend(pipeline.e4.iter().copied());
    out.extend(pipeline.w1.iter().copied());
    out.extend(pipeline.w2.iter().copied());
    out.extend(pipeline.liq.iter().map(|entry| entry.seq));
    out.extend(pipeline.lhq.iter().copied());
    out.extend(pipeline.mdb.iter().map(|entry| entry.seq));
    out.extend(pipeline.stq.iter().copied());
    out.extend(pipeline.scb.iter().map(|entry| entry.seq));
    out.extend(pipeline.l1d.iter().map(|entry| entry.seq));
    out
}
