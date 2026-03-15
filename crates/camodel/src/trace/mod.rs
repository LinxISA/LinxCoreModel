pub mod emit;
pub mod labels;

use std::collections::VecDeque;

use isa::{
    StageTraceEvent, TRAP_BRU_RECOVERY_NOT_BSTART, TRAP_DYNAMIC_TARGET_MISSING,
    TRAP_DYNAMIC_TARGET_NOT_BSTART, TRAP_DYNAMIC_TARGET_STALE, TRAP_SETRET_NOT_ADJACENT,
};
use runtime::GuestRuntime;

use crate::{
    CycleUop, FRONTEND_STAGE_NAMES, IqEntry, L1dTxnKind, StageQueues, branch_kind_label,
    call_materialization_kind_label, dynamic_target_source_kind_label, i2_issue_eligible,
    i2_waits_on_lsid, iq_entry_wait_cause_from_state, live_boundary_epoch_for_seq,
    live_branch_kind_for_seq, live_call_materialization_kind_for_seq,
    live_control_target_owner_row_id_for_seq, live_dynamic_target_producer_kind_for_seq,
    live_dynamic_target_setup_epoch_for_seq, live_dynamic_target_source_epoch_for_seq,
    live_dynamic_target_source_kind_for_seq, live_dynamic_target_source_owner_row_id_for_seq,
    live_return_consumer_kind_for_seq, live_rob_checkpoint_id_for_seq, load_forward_visible,
    ready_iq_winners, redirect_resolve_cycle, resolved_frontend_redirect,
    return_consumer_kind_label,
};

pub(crate) fn tag_stage_cycles(cycle: u64, pipeline: &StageQueues, uops: &mut [CycleUop]) {
    for &seq in &pipeline.e1 {
        let uop = &mut uops[seq];
        if uop.e1_cycle.is_none() {
            uop.e1_cycle = Some(cycle);
            if uop.is_load {
                uop.pick_wakeup_visible = Some(cycle + 1);
            }
        }
    }
    for &seq in &pipeline.e4 {
        let uop = &mut uops[seq];
        if uop.e4_cycle.is_none() {
            uop.e4_cycle = Some(cycle);
            uop.data_ready_visible = Some(cycle + 1);
        }
    }
    for &seq in &pipeline.w1 {
        let uop = &mut uops[seq];
        if uop.w1_cycle.is_none() {
            uop.w1_cycle = Some(cycle);
            if !uop.is_load {
                uop.pick_wakeup_visible = Some(cycle + 1);
                uop.data_ready_visible = Some(cycle + 1);
            }
        }
    }
    for &seq in &pipeline.w2 {
        let uop = &mut uops[seq];
        if uop.done_cycle.is_none() {
            uop.done_cycle = Some(cycle);
        }
    }
}

pub(crate) fn emit_stage_events(
    cycle: u64,
    runtime: &GuestRuntime,
    pipeline: &StageQueues,
    iq: &[IqEntry],
    rob: &VecDeque<usize>,
    uops: &[CycleUop],
    out: &mut Vec<StageTraceEvent>,
) {
    for (idx, queue) in pipeline.frontend.iter().enumerate() {
        for &seq in queue {
            out.push(stage_event(
                cycle,
                runtime,
                uops,
                seq,
                FRONTEND_STAGE_NAMES[idx],
                "resident",
            ));
        }
    }
    for entry in &pipeline.liq {
        let cause = if entry.refill_ready_cycle <= cycle {
            "eligible"
        } else {
            "wait_refill"
        };
        out.push(stage_event(cycle, runtime, uops, entry.seq, "LIQ", cause));
    }
    for &seq in &pipeline.lhq {
        out.push(stage_event(
            cycle,
            runtime,
            uops,
            seq,
            "LHQ",
            "inflight_load",
        ));
    }
    for entry in &pipeline.mdb {
        let cause = if entry.refill_ready_cycle <= cycle {
            "refill_ready"
        } else {
            "wait_refill"
        };
        out.push(stage_event(cycle, runtime, uops, entry.seq, "MDB", cause));
    }
    for &seq in &pipeline.stq {
        out.push(stage_event(
            cycle,
            runtime,
            uops,
            seq,
            "STQ",
            "store_visible",
        ));
    }
    for entry in &pipeline.scb {
        let cause = if entry.enqueue_cycle < cycle {
            "drain_ready"
        } else {
            "coalesce"
        };
        out.push(stage_event(cycle, runtime, uops, entry.seq, "SCB", cause));
    }
    for entry in &pipeline.l1d {
        let cause = match entry.kind {
            L1dTxnKind::LoadHit if entry.ready_cycle <= cycle => "load_hit_resp",
            L1dTxnKind::LoadHit => "load_hit_req",
            L1dTxnKind::StoreDrain if entry.ready_cycle <= cycle => "store_drain_resp",
            L1dTxnKind::StoreDrain => "store_drain_req",
        };
        out.push(stage_event(cycle, runtime, uops, entry.seq, "L1D", cause));
    }
    let iq_ready_winners = ready_iq_winners(cycle, pipeline.lsid_issue_ptr, iq, uops, rob);
    for entry in iq {
        let cause = if entry.inflight {
            "inflight"
        } else if let Some(cause) =
            iq_entry_wait_cause_from_state(entry, cycle, pipeline.lsid_issue_ptr, uops)
        {
            cause
        } else if iq_ready_winners
            .iter()
            .any(|&(_, seq, phys_iq)| seq == entry.seq && phys_iq == entry.phys_iq)
        {
            "ready"
        } else {
            "wait_iq_age"
        };
        out.push(stage_event(cycle, runtime, uops, entry.seq, "IQ", cause));
    }
    for &seq in &pipeline.p1 {
        out.push(stage_event(cycle, runtime, uops, seq, "P1", "pick"));
    }
    for &seq in &pipeline.i1 {
        out.push(stage_event(cycle, runtime, uops, seq, "I1", "rf_arbitrate"));
    }
    for &seq in &pipeline.i2 {
        let cause = if i2_issue_eligible(seq, cycle, pipeline.lsid_issue_ptr, uops) {
            "issue_confirm"
        } else if i2_waits_on_lsid(seq, cycle, pipeline.lsid_issue_ptr, uops) {
            "wait_lsid"
        } else {
            "wait_forward"
        };
        out.push(stage_event(cycle, runtime, uops, seq, "I2", cause));
    }
    for &seq in &pipeline.e1 {
        out.push(stage_event(
            cycle,
            runtime,
            uops,
            seq,
            "E1",
            if uops[seq].is_load {
                "ld_spec_wakeup"
            } else {
                "execute"
            },
        ));
    }
    for &seq in &pipeline.e2 {
        out.push(stage_event(cycle, runtime, uops, seq, "E2", "execute"));
    }
    for &seq in &pipeline.e3 {
        out.push(stage_event(cycle, runtime, uops, seq, "E3", "execute"));
    }
    for &seq in &pipeline.e4 {
        let cause = if uops[seq].is_load && load_forward_visible(seq, pipeline, uops) {
            "ld_store_forward"
        } else {
            "ld_data"
        };
        out.push(stage_event(cycle, runtime, uops, seq, "E4", cause));
    }
    for &seq in &pipeline.w1 {
        out.push(stage_event(cycle, runtime, uops, seq, "W1", "wakeup"));
    }
    for &seq in &pipeline.w2 {
        out.push(stage_event(cycle, runtime, uops, seq, "W2", "complete"));
    }
    if let Some(pending_trap) = pipeline
        .pending_trap
        .filter(|pending_trap| pending_trap.visible_cycle == cycle)
    {
        let target_source_kind =
            live_dynamic_target_source_kind_for_seq(pending_trap.seq, pipeline, uops);
        out.push(stage_event_with_meta(
            cycle,
            runtime,
            uops,
            pending_trap.seq,
            "FLS",
            pending_trap_stage_cause(pending_trap.cause, target_source_kind),
            Some(pending_trap.checkpoint_id),
            Some(pending_trap.cause),
            Some(pending_trap.traparg0),
            live_dynamic_target_setup_epoch_for_seq(pending_trap.seq, pipeline, uops),
            live_boundary_epoch_for_seq(pending_trap.seq, pipeline, uops),
            live_dynamic_target_source_owner_row_id_for_seq(pending_trap.seq, pipeline, uops)
                .as_deref(),
            live_dynamic_target_source_epoch_for_seq(pending_trap.seq, pipeline, uops),
            live_control_target_owner_row_id_for_seq(pending_trap.seq, pipeline, uops).as_deref(),
            live_dynamic_target_producer_kind_for_seq(pending_trap.seq, pipeline, uops)
                .map(return_consumer_kind_label),
            live_branch_kind_for_seq(pending_trap.seq, pipeline, uops).and_then(branch_kind_label),
            live_return_consumer_kind_for_seq(pending_trap.seq, pipeline, uops)
                .map(return_consumer_kind_label),
            live_call_materialization_kind_for_seq(pending_trap.seq, pipeline, uops)
                .map(call_materialization_kind_label),
            target_source_kind.map(dynamic_target_source_kind_label),
        ));
    }
    if let Some(redirect) = resolved_frontend_redirect(cycle, pipeline, uops) {
        out.push(stage_event_with_meta(
            cycle,
            runtime,
            uops,
            redirect.source_seq,
            "FLS",
            if redirect.from_correction {
                "redirect_br_corr"
            } else {
                "redirect_boundary"
            },
            Some(redirect.checkpoint_id),
            None,
            None,
            live_dynamic_target_setup_epoch_for_seq(redirect.source_seq, pipeline, uops),
            live_boundary_epoch_for_seq(redirect.source_seq, pipeline, uops),
            live_dynamic_target_source_owner_row_id_for_seq(redirect.source_seq, pipeline, uops)
                .as_deref(),
            live_dynamic_target_source_epoch_for_seq(redirect.source_seq, pipeline, uops),
            live_control_target_owner_row_id_for_seq(redirect.source_seq, pipeline, uops)
                .as_deref(),
            live_dynamic_target_producer_kind_for_seq(redirect.source_seq, pipeline, uops)
                .map(return_consumer_kind_label),
            live_branch_kind_for_seq(redirect.source_seq, pipeline, uops)
                .and_then(branch_kind_label),
            live_return_consumer_kind_for_seq(redirect.source_seq, pipeline, uops)
                .map(return_consumer_kind_label),
            live_call_materialization_kind_for_seq(redirect.source_seq, pipeline, uops)
                .map(call_materialization_kind_label),
            live_dynamic_target_source_kind_for_seq(redirect.source_seq, pipeline, uops)
                .map(dynamic_target_source_kind_label),
        ));
    } else {
        for (seq, uop) in uops.iter().enumerate() {
            if redirect_resolve_cycle(uop) == Some(cycle) {
                out.push(stage_event_with_meta(
                    cycle,
                    runtime,
                    uops,
                    seq,
                    "FLS",
                    "redirect",
                    Some(live_rob_checkpoint_id_for_seq(seq, pipeline, uops)),
                    None,
                    None,
                    live_dynamic_target_setup_epoch_for_seq(seq, pipeline, uops),
                    live_boundary_epoch_for_seq(seq, pipeline, uops),
                    live_dynamic_target_source_owner_row_id_for_seq(seq, pipeline, uops).as_deref(),
                    live_dynamic_target_source_epoch_for_seq(seq, pipeline, uops),
                    live_control_target_owner_row_id_for_seq(seq, pipeline, uops).as_deref(),
                    live_dynamic_target_producer_kind_for_seq(seq, pipeline, uops)
                        .map(return_consumer_kind_label),
                    live_branch_kind_for_seq(seq, pipeline, uops).and_then(branch_kind_label),
                    live_return_consumer_kind_for_seq(seq, pipeline, uops)
                        .map(return_consumer_kind_label),
                    live_call_materialization_kind_for_seq(seq, pipeline, uops)
                        .map(call_materialization_kind_label),
                    live_dynamic_target_source_kind_for_seq(seq, pipeline, uops)
                        .map(dynamic_target_source_kind_label),
                ));
            }
        }
    }
    for &seq in rob {
        let cause = if uops[seq].done_cycle.is_some() {
            "ready"
        } else {
            "wait_head"
        };
        out.push(stage_event(cycle, runtime, uops, seq, "ROB", cause));
    }
}

fn pending_trap_stage_cause(
    cause: u64,
    target_source_kind: Option<crate::DynamicTargetSourceKind>,
) -> &'static str {
    match cause {
        TRAP_BRU_RECOVERY_NOT_BSTART => "bru_recovery_fault",
        TRAP_DYNAMIC_TARGET_MISSING => "dynamic_target_missing",
        TRAP_DYNAMIC_TARGET_STALE => match target_source_kind {
            Some(crate::DynamicTargetSourceKind::CallReturnFused)
            | Some(crate::DynamicTargetSourceKind::CallReturnAdjacentSetret) => {
                "dynamic_target_stale_return"
            }
            Some(crate::DynamicTargetSourceKind::ArchTargetSetup) | None => {
                "dynamic_target_stale_setup"
            }
        },
        TRAP_DYNAMIC_TARGET_NOT_BSTART => "dynamic_target_not_bstart",
        TRAP_SETRET_NOT_ADJACENT => "call_header_fault",
        _ => "trap_fault",
    }
}

pub(crate) fn stage_event(
    cycle: u64,
    runtime: &GuestRuntime,
    uops: &[CycleUop],
    seq: usize,
    stage: &str,
    cause: &str,
) -> StageTraceEvent {
    stage_event_with_meta(
        cycle, runtime, uops, seq, stage, cause, None, None, None, None, None, None, None, None,
        None, None, None, None, None,
    )
}

pub(crate) fn stage_event_with_meta(
    cycle: u64,
    runtime: &GuestRuntime,
    uops: &[CycleUop],
    seq: usize,
    stage: &str,
    cause: &str,
    checkpoint_id: Option<u8>,
    trap_cause: Option<u64>,
    traparg0: Option<u64>,
    target_setup_epoch: Option<u16>,
    boundary_epoch: Option<u16>,
    target_source_owner_row_id: Option<&str>,
    target_source_epoch: Option<u16>,
    target_owner_row_id: Option<&str>,
    target_producer_kind: Option<&str>,
    branch_kind: Option<&str>,
    return_kind: Option<&str>,
    call_materialization_kind: Option<&str>,
    target_source_kind: Option<&str>,
) -> StageTraceEvent {
    StageTraceEvent {
        cycle,
        row_id: format!("uop{seq}"),
        stage_id: stage.to_string(),
        lane_id: uops[seq]
            .phys_iq
            .map(|phys_iq| phys_iq.lane_id().to_string())
            .unwrap_or_else(|| runtime.block.lane_id.clone()),
        stall: false,
        cause: cause.to_string(),
        checkpoint_id,
        trap_cause,
        traparg0,
        target_setup_epoch,
        boundary_epoch,
        target_source_owner_row_id: target_source_owner_row_id.map(str::to_string),
        target_source_epoch,
        target_owner_row_id: target_owner_row_id.map(str::to_string),
        target_producer_kind: target_producer_kind.map(str::to_string),
        branch_kind: branch_kind.map(str::to_string),
        return_kind: return_kind.map(str::to_string),
        call_materialization_kind: call_materialization_kind.map(str::to_string),
        target_source_kind: target_source_kind.map(str::to_string),
    }
}
