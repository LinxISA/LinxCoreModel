pub mod decode;
pub mod dispatch;
pub mod fetch;

use std::collections::VecDeque;

use isa::DecodedInstruction;
use isa::TRAP_SETRET_NOT_ADJACENT;

use crate::{
    BranchOwnerContext, BranchOwnerKind, CallMaterializationKind, CycleUop, DISPATCH_WIDTH,
    DynamicTargetSourceKind, FRONTEND_STAGE_NAMES, IQ_ENQUEUE_PORTS, ISSUE_WIDTH, IqEntry,
    PHYS_IQ_COUNT, PhysIq, ReturnConsumerKind, StageQueues, allocate_qtag, annotate_qtag_sources,
    checkpoint_id_for_seq, is_boundary_redirect_owner, is_bstart, is_bstop, is_macro_boundary,
    is_start_marker, packet_checkpoint_id, rebuild_iq_owner_table, register_iq_wait_crossbar_entry,
    rob_checkpoint_id_for_seq, route_phys_iq, snapshot_ready_tables_for_checkpoint,
    unresolved_redirect_barrier,
};

#[cfg(test)]
pub(crate) fn live_checkpoint_id_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> u8 {
    pipeline
        .seq_checkpoint_ids
        .get(&seq)
        .copied()
        .unwrap_or_else(|| checkpoint_id_for_seq(seq, uops))
}

pub(crate) fn live_rob_checkpoint_id_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> u8 {
    pipeline
        .seq_rob_checkpoint_ids
        .get(&seq)
        .copied()
        .unwrap_or_else(|| {
            if !is_start_marker(&uops[seq].decoded) {
                return 0;
            }
            let static_fetch_checkpoint = checkpoint_id_for_seq(seq, uops);
            let live_fetch_checkpoint = pipeline
                .seq_checkpoint_ids
                .get(&seq)
                .copied()
                .unwrap_or(static_fetch_checkpoint);
            let slot_offset =
                rob_checkpoint_id_for_seq(seq, uops).wrapping_sub(static_fetch_checkpoint) & 0x3f;
            live_fetch_checkpoint.wrapping_add(slot_offset) & 0x3f
        })
}

pub(crate) fn recovery_checkpoint_id_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> u8 {
    if let Some(checkpoint_id) = pipeline.seq_recovery_checkpoint_ids.get(&seq).copied() {
        return checkpoint_id;
    }
    if is_start_marker(&uops[seq].decoded) {
        return live_rob_checkpoint_id_for_seq(seq, pipeline, uops);
    }
    (0..seq)
        .rev()
        .find(|&candidate| is_start_marker(&uops[candidate].decoded))
        .map(|candidate| live_rob_checkpoint_id_for_seq(candidate, pipeline, uops))
        .unwrap_or(0)
}

pub(crate) fn recovery_epoch_for_seq(seq: usize, pipeline: &StageQueues, uops: &[CycleUop]) -> u16 {
    pipeline
        .seq_recovery_epochs
        .get(&seq)
        .copied()
        .unwrap_or_else(|| crate::block_epoch_for_seq(seq, uops))
}

pub(crate) fn branch_context_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> BranchOwnerContext {
    pipeline
        .seq_branch_contexts
        .get(&seq)
        .copied()
        .or_else(|| fallback_branch_context_for_seq(seq, pipeline, uops))
        .unwrap_or_default()
}

pub(crate) fn live_boundary_target_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> Option<u64> {
    let uop = uops.get(seq)?;
    if is_boundary_redirect_owner(&uop.decoded) {
        return boundary_context_from_uop(seq, uop, pipeline)
            .map(|context| context.target_pc)
            .filter(|target_pc| *target_pc != 0)
            .or_else(|| {
                pipeline
                    .seq_branch_contexts
                    .get(&seq)
                    .map(|context| context.target_pc)
                    .filter(|target_pc| *target_pc != 0)
            })
            .or(uop.redirect_target);
    }
    pipeline
        .seq_branch_contexts
        .get(&seq)
        .map(|context| context.target_pc)
        .filter(|target_pc| *target_pc != 0)
}

pub(crate) fn live_branch_kind_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> Option<BranchOwnerKind> {
    let uop = uops.get(seq)?;
    if is_boundary_redirect_owner(&uop.decoded) {
        return boundary_context_from_uop(seq, uop, pipeline)
            .map(|context| context.kind)
            .filter(|kind| *kind != BranchOwnerKind::None)
            .or_else(|| {
                pipeline
                    .seq_branch_contexts
                    .get(&seq)
                    .map(|context| context.kind)
                    .filter(|kind| *kind != BranchOwnerKind::None)
            });
    }
    let kind = branch_context_for_seq(seq, pipeline, uops).kind;
    (kind != BranchOwnerKind::None).then_some(kind)
}

pub(crate) fn branch_kind_label(kind: BranchOwnerKind) -> Option<&'static str> {
    match kind {
        BranchOwnerKind::None => None,
        BranchOwnerKind::Fall => Some("fall"),
        BranchOwnerKind::Cond => Some("cond"),
        BranchOwnerKind::Call => Some("call"),
        BranchOwnerKind::Ret => Some("ret"),
        BranchOwnerKind::Direct => Some("direct"),
        BranchOwnerKind::Ind => Some("ind"),
        BranchOwnerKind::ICall => Some("icall"),
    }
}

pub(crate) fn return_consumer_kind_label(kind: ReturnConsumerKind) -> &'static str {
    match kind {
        ReturnConsumerKind::SetcTgt => "setc_tgt",
        ReturnConsumerKind::FretRa => "fret_ra",
        ReturnConsumerKind::FretStk => "fret_stk",
    }
}

pub(crate) fn call_materialization_kind_label(kind: CallMaterializationKind) -> &'static str {
    match kind {
        CallMaterializationKind::FusedCall => "fused_call",
        CallMaterializationKind::AdjacentSetret => "adjacent_setret",
    }
}

pub(crate) fn dynamic_target_source_kind_label(kind: DynamicTargetSourceKind) -> &'static str {
    match kind {
        DynamicTargetSourceKind::ArchTargetSetup => "arch_target_setup",
        DynamicTargetSourceKind::CallReturnFused => "call_return_fused",
        DynamicTargetSourceKind::CallReturnAdjacentSetret => "call_return_adjacent_setret",
    }
}

pub(crate) fn live_return_consumer_kind_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> Option<ReturnConsumerKind> {
    if let Some(kind) = pipeline.seq_return_consumer_kinds.get(&seq).copied() {
        return Some(kind);
    }
    match uops.get(seq)?.decoded.mnemonic.as_str() {
        "FRET.RA" => Some(ReturnConsumerKind::FretRa),
        "FRET.STK" => Some(ReturnConsumerKind::FretStk),
        _ => None,
    }
}

pub(crate) fn live_call_materialization_kind_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> Option<CallMaterializationKind> {
    if let Some(kind) = pipeline.seq_call_materialization_kinds.get(&seq).copied() {
        return Some(kind);
    }
    let uop = uops.get(seq)?;
    if is_setret(&uop.decoded) {
        return Some(CallMaterializationKind::AdjacentSetret);
    }
    (is_call_header_uop(uop) && call_return_target_pc(uop).is_some())
        .then_some(CallMaterializationKind::FusedCall)
}

fn dynamic_target_source_kind_from_call_materialization(
    kind: CallMaterializationKind,
) -> DynamicTargetSourceKind {
    match kind {
        CallMaterializationKind::FusedCall => DynamicTargetSourceKind::CallReturnFused,
        CallMaterializationKind::AdjacentSetret => {
            DynamicTargetSourceKind::CallReturnAdjacentSetret
        }
    }
}

pub(crate) fn live_dynamic_target_source_kind_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    _uops: &[CycleUop],
) -> Option<DynamicTargetSourceKind> {
    pipeline.seq_boundary_target_source_kinds.get(&seq).copied()
}

pub(crate) fn live_dynamic_target_setup_epoch_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    _uops: &[CycleUop],
) -> Option<u16> {
    pipeline.seq_boundary_target_setup_epochs.get(&seq).copied()
}

pub(crate) fn live_dynamic_target_source_epoch_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    _uops: &[CycleUop],
) -> Option<u16> {
    pipeline
        .seq_boundary_target_source_epochs
        .get(&seq)
        .copied()
}

pub(crate) fn live_boundary_epoch_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> Option<u16> {
    live_branch_kind_for_seq(seq, pipeline, uops)
        .map(|_| recovery_epoch_for_seq(seq, pipeline, uops))
}

pub(crate) fn live_dynamic_target_producer_kind_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    _uops: &[CycleUop],
) -> Option<ReturnConsumerKind> {
    pipeline
        .seq_boundary_target_producer_kinds
        .get(&seq)
        .copied()
}

pub(crate) fn live_dynamic_target_source_owner_row_id_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    _uops: &[CycleUop],
) -> Option<String> {
    pipeline
        .seq_boundary_target_source_owner_seqs
        .get(&seq)
        .copied()
        .map(|owner_seq| format!("uop{owner_seq}"))
}

pub(crate) fn live_control_target_owner_row_id_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> Option<String> {
    if let Some(owner_seq) = pipeline.seq_boundary_target_owner_seqs.get(&seq).copied() {
        return Some(format!("uop{owner_seq}"));
    }
    if let Some(owner_seq) = pipeline
        .seq_call_return_target_owner_seqs
        .get(&seq)
        .copied()
    {
        return Some(format!("uop{owner_seq}"));
    }
    let decoded = &uops.get(seq)?.decoded;
    if is_setret(decoded) || matches!(decoded.mnemonic.as_str(), "FRET.RA" | "FRET.STK") {
        return Some(format!("uop{seq}"));
    }
    None
}

fn fallback_branch_context_for_seq(
    seq: usize,
    pipeline: &StageQueues,
    uops: &[CycleUop],
) -> Option<BranchOwnerContext> {
    (0..seq)
        .rev()
        .find_map(|candidate| boundary_context_from_uop(candidate, &uops[candidate], pipeline))
}

fn branch_owner_kind_for_uop(uop: &CycleUop) -> BranchOwnerKind {
    if matches!(uop.decoded.mnemonic.as_str(), "FRET.RA" | "FRET.STK") {
        return BranchOwnerKind::Ret;
    }
    if matches!(
        uop.decoded.mnemonic.as_str(),
        "BSTART CALL" | "HL.BSTART CALL"
    ) {
        return BranchOwnerKind::Call;
    }

    let asm = uop.decoded.asm.to_ascii_uppercase();
    if asm.contains(" COND") {
        return BranchOwnerKind::Cond;
    }
    if asm.contains(" CALL") {
        return BranchOwnerKind::Call;
    }
    if asm.contains(" RET") {
        return BranchOwnerKind::Ret;
    }
    if asm.contains(" DIRECT") {
        return BranchOwnerKind::Direct;
    }
    if asm.contains(" ICALL") {
        return BranchOwnerKind::ICall;
    }
    if asm.contains(" IND") {
        return BranchOwnerKind::Ind;
    }
    if asm.contains(" FALL") {
        return BranchOwnerKind::Fall;
    }

    if uop.decoded.mnemonic == "C.BSTART.STD" {
        return match decoded_field_u64(&uop.decoded, "BrType") {
            Some(1) => BranchOwnerKind::Fall,
            Some(2) => BranchOwnerKind::Direct,
            Some(3) => BranchOwnerKind::Cond,
            Some(4) => BranchOwnerKind::Call,
            Some(5) => BranchOwnerKind::Ind,
            Some(6) => BranchOwnerKind::ICall,
            Some(7) => BranchOwnerKind::Ret,
            _ => BranchOwnerKind::None,
        };
    }

    BranchOwnerKind::None
}

fn boundary_context_from_uop(
    seq: usize,
    uop: &CycleUop,
    pipeline: &StageQueues,
) -> Option<BranchOwnerContext> {
    let kind = branch_owner_kind_for_uop(uop);
    if kind == BranchOwnerKind::None {
        return None;
    }
    let target_pc = boundary_target_pc(seq, uop, pipeline, kind).unwrap_or(0);
    Some(BranchOwnerContext {
        kind,
        base_pc: uop.commit.pc,
        target_pc,
        off: if target_pc == 0 {
            0
        } else {
            target_pc.wrapping_sub(uop.commit.pc)
        },
        pred_take: match kind {
            BranchOwnerKind::Cond => target_pc != 0 && target_pc < uop.commit.pc,
            BranchOwnerKind::Ret => false,
            BranchOwnerKind::Fall
            | BranchOwnerKind::Call
            | BranchOwnerKind::Direct
            | BranchOwnerKind::Ind
            | BranchOwnerKind::ICall
            | BranchOwnerKind::None => false,
        },
        epoch: pipeline
            .seq_recovery_epochs
            .get(&seq)
            .copied()
            .unwrap_or(0)
            .wrapping_add(1),
    })
}

fn boundary_target_pc(
    seq: usize,
    uop: &CycleUop,
    pipeline: &StageQueues,
    kind: BranchOwnerKind,
) -> Option<u64> {
    match kind {
        BranchOwnerKind::Ret | BranchOwnerKind::Ind | BranchOwnerKind::ICall => pipeline
            .seq_boundary_target_pcs
            .get(&seq)
            .copied()
            .or_else(|| pipeline.seq_dynamic_target_pcs.get(&seq).copied())
            .or(pipeline.active_dynamic_target_pc)
            .or(uop.redirect_target),
        BranchOwnerKind::None => None,
        BranchOwnerKind::Fall
        | BranchOwnerKind::Cond
        | BranchOwnerKind::Call
        | BranchOwnerKind::Direct => uop.redirect_target,
    }
}

fn is_call_header_uop(uop: &CycleUop) -> bool {
    is_bstart(&uop.decoded) && branch_owner_kind_for_uop(uop) == BranchOwnerKind::Call
}

fn is_setret(decoded: &DecodedInstruction) -> bool {
    matches!(
        decoded.mnemonic.as_str(),
        "SETRET" | "C.SETRET" | "HL.SETRET"
    )
}

fn call_return_target_pc(uop: &CycleUop) -> Option<u64> {
    ((is_call_header_uop(uop)
        || matches!(
            uop.decoded.mnemonic.as_str(),
            "SETRET" | "C.SETRET" | "HL.SETRET"
        ))
        && uop.commit.wb_valid != 0
        && uop.commit.wb_rd == 10)
        .then_some(uop.commit.wb_data)
}

fn update_dynamic_target_owner(seq: usize, pipeline: &mut StageQueues, uops: &[CycleUop]) {
    let uop = &uops[seq];
    let boundary_kind = branch_owner_kind_for_uop(uop);
    let target_pc = match uop.decoded.mnemonic.as_str() {
        "SETC.TGT" | "C.SETC.TGT" if uop.commit.src0_valid != 0 => Some(uop.commit.src0_data),
        "FRET.RA" | "FRET.STK" => uop.redirect_target,
        "BSTOP" | "C.BSTOP" => pipeline.active_dynamic_target_pc,
        _ => None,
    };
    if let Some(target_pc) = target_pc {
        pipeline.seq_dynamic_target_pcs.insert(seq, target_pc);
    }
    if matches!(uop.decoded.mnemonic.as_str(), "SETC.TGT" | "C.SETC.TGT") {
        let setup_epoch = target_pc.map(|_| recovery_epoch_for_seq(seq, pipeline, uops));
        let sourced_from_call = target_pc.is_some()
            && uop.commit.src0_reg == 10
            && pipeline.active_call_return_target_pc == target_pc;
        let source_owner_seq = target_pc.map(|_| {
            if sourced_from_call {
                pipeline.active_call_return_target_owner_seq.unwrap_or(seq)
            } else {
                seq
            }
        });
        let source_epoch = target_pc.map(|_| {
            if sourced_from_call {
                pipeline
                    .active_call_return_target_epoch
                    .unwrap_or_else(|| recovery_epoch_for_seq(seq, pipeline, uops))
            } else {
                recovery_epoch_for_seq(seq, pipeline, uops)
            }
        });
        pipeline.active_dynamic_target_pc = target_pc;
        pipeline.active_dynamic_target_owner_seq = target_pc.map(|_| seq);
        pipeline.active_dynamic_target_producer_kind =
            target_pc.map(|_| ReturnConsumerKind::SetcTgt);
        pipeline.active_dynamic_target_setup_epoch = setup_epoch;
        pipeline.active_dynamic_target_owner_kind = target_pc.map(|_| ReturnConsumerKind::SetcTgt);
        pipeline.active_dynamic_target_source_owner_seq = source_owner_seq;
        pipeline.active_dynamic_target_source_epoch = source_epoch;
        pipeline.active_dynamic_target_call_materialization_kind =
            target_pc.and_then(|target_pc| {
                (uop.commit.src0_reg == 10
                    && pipeline.active_call_return_target_pc == Some(target_pc))
                .then_some(pipeline.active_call_return_materialization_kind)
                .flatten()
            });
        pipeline.active_dynamic_target_source_kind = target_pc.map(|target_pc| {
            if uop.commit.src0_reg == 10 && pipeline.active_call_return_target_pc == Some(target_pc)
            {
                pipeline
                    .active_call_return_materialization_kind
                    .map(dynamic_target_source_kind_from_call_materialization)
                    .unwrap_or(DynamicTargetSourceKind::ArchTargetSetup)
            } else {
                DynamicTargetSourceKind::ArchTargetSetup
            }
        });
        if matches!(
            pipeline.active_branch_context.kind,
            BranchOwnerKind::Ret | BranchOwnerKind::Ind | BranchOwnerKind::ICall
        ) {
            pipeline.active_branch_context.target_pc = target_pc.unwrap_or(0);
            pipeline.active_branch_context.off = target_pc
                .map(|target_pc| target_pc.wrapping_sub(pipeline.active_branch_context.base_pc))
                .unwrap_or(0);
        }
    }
    let boundary_owner = match uop.decoded.mnemonic.as_str() {
        "FRET.RA" => target_pc.map(|target_pc| {
            let sourced_from_call = pipeline.active_call_return_target_pc == Some(target_pc);
            let source_owner_seq = if sourced_from_call {
                pipeline.active_call_return_target_owner_seq.unwrap_or(seq)
            } else {
                seq
            };
            let source_epoch = if sourced_from_call {
                pipeline
                    .active_call_return_target_epoch
                    .unwrap_or_else(|| recovery_epoch_for_seq(seq, pipeline, uops))
            } else {
                recovery_epoch_for_seq(seq, pipeline, uops)
            };
            (
                target_pc,
                if sourced_from_call {
                    pipeline.active_call_return_target_owner_seq.unwrap_or(seq)
                } else {
                    seq
                },
                Some(ReturnConsumerKind::FretRa),
                Some(recovery_epoch_for_seq(seq, pipeline, uops)),
                Some(ReturnConsumerKind::FretRa),
                Some(source_owner_seq),
                Some(source_epoch),
                sourced_from_call
                    .then_some(pipeline.active_call_return_materialization_kind)
                    .flatten(),
                sourced_from_call
                    .then_some(
                        pipeline
                            .active_call_return_materialization_kind
                            .map(dynamic_target_source_kind_from_call_materialization),
                    )
                    .flatten(),
            )
        }),
        "FRET.STK" => target_pc.map(|target_pc| {
            (
                target_pc,
                seq,
                Some(ReturnConsumerKind::FretStk),
                Some(recovery_epoch_for_seq(seq, pipeline, uops)),
                Some(ReturnConsumerKind::FretStk),
                Some(seq),
                Some(recovery_epoch_for_seq(seq, pipeline, uops)),
                None,
                None,
            )
        }),
        "BSTOP" | "C.BSTOP"
            if matches!(
                pipeline.active_branch_context.kind,
                BranchOwnerKind::Ret | BranchOwnerKind::Ind | BranchOwnerKind::ICall
            ) =>
        {
            pipeline
                .active_dynamic_target_pc
                .zip(pipeline.active_dynamic_target_owner_seq)
                .zip(pipeline.active_dynamic_target_owner_kind)
                .zip(pipeline.active_dynamic_target_producer_kind)
                .zip(pipeline.active_dynamic_target_setup_epoch)
                .zip(pipeline.active_dynamic_target_source_owner_seq)
                .zip(pipeline.active_dynamic_target_source_epoch)
                .map(
                    |(
                        (
                            ((((target_pc, owner_seq), owner_kind), producer_kind), setup_epoch),
                            source_owner_seq,
                        ),
                        source_epoch,
                    )| {
                        (
                            target_pc,
                            owner_seq,
                            Some(producer_kind),
                            Some(setup_epoch),
                            matches!(pipeline.active_branch_context.kind, BranchOwnerKind::Ret)
                                .then_some(owner_kind),
                            Some(source_owner_seq),
                            Some(source_epoch),
                            pipeline.active_dynamic_target_call_materialization_kind,
                            pipeline.active_dynamic_target_source_kind,
                        )
                    },
                )
        }
        _ if matches!(boundary_kind, BranchOwnerKind::Ret) => pipeline
            .active_dynamic_target_pc
            .zip(pipeline.active_dynamic_target_owner_seq)
            .zip(pipeline.active_dynamic_target_owner_kind)
            .zip(pipeline.active_dynamic_target_producer_kind)
            .zip(pipeline.active_dynamic_target_setup_epoch)
            .zip(pipeline.active_dynamic_target_source_owner_seq)
            .zip(pipeline.active_dynamic_target_source_epoch)
            .map(
                |(
                    (
                        ((((target_pc, owner_seq), owner_kind), producer_kind), setup_epoch),
                        source_owner_seq,
                    ),
                    source_epoch,
                )| {
                    (
                        target_pc,
                        owner_seq,
                        Some(producer_kind),
                        Some(setup_epoch),
                        Some(owner_kind),
                        Some(source_owner_seq),
                        Some(source_epoch),
                        pipeline.active_dynamic_target_call_materialization_kind,
                        pipeline.active_dynamic_target_source_kind,
                    )
                },
            ),
        _ if matches!(boundary_kind, BranchOwnerKind::Ind | BranchOwnerKind::ICall) => pipeline
            .active_dynamic_target_pc
            .zip(pipeline.active_dynamic_target_owner_seq)
            .zip(pipeline.active_dynamic_target_producer_kind)
            .zip(pipeline.active_dynamic_target_setup_epoch)
            .zip(pipeline.active_dynamic_target_source_owner_seq)
            .zip(pipeline.active_dynamic_target_source_epoch)
            .map(
                |(
                    ((((target_pc, owner_seq), producer_kind), setup_epoch), source_owner_seq),
                    source_epoch,
                )| {
                    (
                        target_pc,
                        owner_seq,
                        Some(producer_kind),
                        Some(setup_epoch),
                        None,
                        Some(source_owner_seq),
                        Some(source_epoch),
                        pipeline.active_dynamic_target_call_materialization_kind,
                        pipeline.active_dynamic_target_source_kind,
                    )
                },
            ),
        _ => None,
    };
    if let Some((
        target_pc,
        owner_seq,
        producer_kind,
        setup_epoch,
        return_kind,
        source_owner_seq,
        source_epoch,
        call_materialization_kind,
        target_source_kind,
    )) = boundary_owner
    {
        pipeline.seq_boundary_target_pcs.insert(seq, target_pc);
        pipeline
            .seq_boundary_target_owner_seqs
            .insert(seq, owner_seq);
        if let Some(producer_kind) = producer_kind {
            pipeline
                .seq_boundary_target_producer_kinds
                .insert(seq, producer_kind);
        }
        if let Some(setup_epoch) = setup_epoch {
            pipeline
                .seq_boundary_target_setup_epochs
                .insert(seq, setup_epoch);
        }
        if let Some(source_owner_seq) = source_owner_seq {
            pipeline
                .seq_boundary_target_source_owner_seqs
                .insert(seq, source_owner_seq);
        }
        if let Some(source_epoch) = source_epoch {
            pipeline
                .seq_boundary_target_source_epochs
                .insert(seq, source_epoch);
        }
        if let Some(target_source_kind) = target_source_kind {
            pipeline
                .seq_boundary_target_source_kinds
                .insert(seq, target_source_kind);
        }
        if let Some(return_kind) = return_kind {
            pipeline.seq_return_consumer_kinds.insert(seq, return_kind);
        }
        if let Some(call_materialization_kind) = call_materialization_kind {
            pipeline
                .seq_call_materialization_kinds
                .insert(seq, call_materialization_kind);
        }
    }
}

fn update_call_header_owner(seq: usize, pipeline: &mut StageQueues, uops: &[CycleUop]) {
    let uop = &uops[seq];
    let is_call_header = is_call_header_uop(uop);
    let is_setret = is_setret(&uop.decoded);

    if let Some(active_header_seq) = pipeline.active_call_header_seq {
        if seq != active_header_seq.saturating_add(1) || !is_setret {
            pipeline.active_call_header_seq = None;
        }
    }

    if is_call_header {
        pipeline.active_call_header_seq = None;
        if let Some(target_pc) = call_return_target_pc(uop) {
            let target_epoch = recovery_epoch_for_seq(seq, pipeline, uops);
            pipeline.seq_call_return_target_pcs.insert(seq, target_pc);
            pipeline.seq_call_return_target_owner_seqs.insert(seq, seq);
            pipeline
                .seq_call_return_target_epochs
                .insert(seq, target_epoch);
            pipeline
                .seq_call_materialization_kinds
                .insert(seq, CallMaterializationKind::FusedCall);
            pipeline.active_call_return_target_pc = Some(target_pc);
            pipeline.active_call_return_target_owner_seq = Some(seq);
            pipeline.active_call_return_target_epoch = Some(target_epoch);
            pipeline.active_call_return_materialization_kind =
                Some(CallMaterializationKind::FusedCall);
        } else {
            pipeline.active_call_header_seq = Some(seq);
        }
        return;
    }

    if is_setret {
        if let Some(header_seq) = pipeline
            .active_call_header_seq
            .filter(|&header_seq| seq == header_seq.saturating_add(1))
        {
            if let Some(target_pc) = call_return_target_pc(uop) {
                let target_epoch = recovery_epoch_for_seq(seq, pipeline, uops);
                pipeline
                    .seq_call_return_target_pcs
                    .insert(header_seq, target_pc);
                pipeline
                    .seq_call_return_target_owner_seqs
                    .insert(header_seq, seq);
                pipeline
                    .seq_call_return_target_epochs
                    .insert(header_seq, target_epoch);
                pipeline
                    .seq_call_materialization_kinds
                    .insert(header_seq, CallMaterializationKind::AdjacentSetret);
                pipeline.seq_call_return_target_pcs.insert(seq, target_pc);
                pipeline.seq_call_return_target_owner_seqs.insert(seq, seq);
                pipeline
                    .seq_call_return_target_epochs
                    .insert(seq, target_epoch);
                pipeline
                    .seq_call_materialization_kinds
                    .insert(seq, CallMaterializationKind::AdjacentSetret);
                pipeline.active_call_return_target_pc = Some(target_pc);
                pipeline.active_call_return_target_owner_seq = Some(seq);
                pipeline.active_call_return_target_epoch = Some(target_epoch);
                pipeline.active_call_return_materialization_kind =
                    Some(CallMaterializationKind::AdjacentSetret);
            }
            pipeline.active_call_header_seq = None;
        } else {
            pipeline
                .seq_call_materialization_kinds
                .insert(seq, CallMaterializationKind::AdjacentSetret);
            pipeline
                .seq_call_header_faults
                .insert(seq, TRAP_SETRET_NOT_ADJACENT);
            pipeline.active_call_header_seq = None;
        }
    }
}

fn decoded_field_u64(decoded: &DecodedInstruction, name: &str) -> Option<u64> {
    decoded
        .fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.value_u64)
}

pub(crate) fn fill_fetch(
    cycle: u64,
    pipeline: &mut StageQueues,
    next_fetch_seq: &mut usize,
    uops: &[CycleUop],
) {
    if let Some(redirect) = pipeline.frontend_redirect {
        if cycle < redirect.resume_cycle {
            return;
        }
        *next_fetch_seq = redirect.restart_seq;
        pipeline.frontend_redirect = None;
    }

    let total_uops = uops.len();
    let mut fetched = Vec::new();
    while pipeline.frontend[0].len() < crate::FETCH_WIDTH && *next_fetch_seq < total_uops {
        if let Some(barrier_seq) = unresolved_redirect_barrier(*next_fetch_seq, uops) {
            if *next_fetch_seq > barrier_seq {
                break;
            }
        }
        let seq = *next_fetch_seq;
        pipeline.frontend[0].push_back(seq);
        fetched.push(seq);
        *next_fetch_seq += 1;
        if uops[seq].redirect_target.is_some() {
            break;
        }
    }
    if let Some(&packet_head_seq) = fetched.first() {
        let checkpoint_id = packet_checkpoint_id(uops[packet_head_seq].commit.pc);
        for seq in fetched {
            pipeline
                .seq_checkpoint_ids
                .entry(seq)
                .or_insert(checkpoint_id);
        }
    }
}

pub(crate) fn dispatch_to_iq_and_bypass(
    cycle: u64,
    pipeline: &mut StageQueues,
    iq: &mut Vec<IqEntry>,
    _rob: &mut VecDeque<usize>,
    uops: &mut [CycleUop],
) {
    let mut stay_s2 = VecDeque::new();
    let mut enqueue_ports_used = [0usize; PHYS_IQ_COUNT];
    while let Some(seq) = pipeline.frontend[10].pop_front() {
        let decoded = &uops[seq].decoded;
        let is_call_header = is_call_header_uop(&uops[seq]);
        let is_bstart_head = is_bstart(decoded) && pipeline.active_block_head;
        let is_bstart_mid = is_bstart(decoded) && !pipeline.active_block_head;
        let is_boundary = is_bstart_mid || is_bstop(decoded) || is_macro_boundary(decoded);
        let recovery_checkpoint_id = if is_start_marker(&uops[seq].decoded) {
            let checkpoint_id = live_rob_checkpoint_id_for_seq(seq, pipeline, uops);
            pipeline.seq_rob_checkpoint_ids.insert(seq, checkpoint_id);
            pipeline.active_recovery_checkpoint_id = checkpoint_id;
            snapshot_ready_tables_for_checkpoint(pipeline, checkpoint_id);
            checkpoint_id
        } else {
            pipeline.active_recovery_checkpoint_id
        };
        pipeline
            .seq_recovery_checkpoint_ids
            .insert(seq, recovery_checkpoint_id);
        pipeline
            .seq_recovery_epochs
            .insert(seq, pipeline.active_recovery_epoch);
        pipeline
            .seq_branch_contexts
            .insert(seq, pipeline.active_branch_context);
        update_dynamic_target_owner(seq, pipeline, uops);
        update_call_header_owner(seq, pipeline, uops);
        if is_boundary {
            pipeline.active_block_head = true;
        }
        if is_bstart_head {
            pipeline.active_block_head = false;
        }
        if is_boundary || is_bstart_head {
            pipeline.active_recovery_epoch = pipeline.active_recovery_epoch.wrapping_add(1);
        }
        if is_boundary || is_bstart_head {
            pipeline.active_branch_context = boundary_context_from_uop(seq, &uops[seq], pipeline)
                .map(|context| BranchOwnerContext {
                    epoch: pipeline.active_recovery_epoch,
                    ..context
                })
                .unwrap_or(BranchOwnerContext {
                    epoch: pipeline.active_recovery_epoch,
                    ..BranchOwnerContext::default()
                });
            pipeline.active_dynamic_target_pc = None;
            pipeline.active_dynamic_target_owner_seq = None;
            pipeline.active_dynamic_target_producer_kind = None;
            pipeline.active_dynamic_target_setup_epoch = None;
            pipeline.active_dynamic_target_owner_kind = None;
            pipeline.active_dynamic_target_source_owner_seq = None;
            pipeline.active_dynamic_target_source_epoch = None;
            pipeline.active_dynamic_target_source_kind = None;
            pipeline.active_dynamic_target_call_materialization_kind = None;
            if !is_call_header {
                pipeline.active_call_header_seq = None;
            }
        }
        if uops[seq].bypass_d2 {
            uops[seq].pick_wakeup_visible.get_or_insert(cycle + 1);
            uops[seq].data_ready_visible.get_or_insert(cycle + 1);
            uops[seq].phys_iq = Some(PhysIq::CmdIq);
            if pipeline.w2.len() < ISSUE_WIDTH {
                pipeline.w2.push_back(seq);
            } else {
                stay_s2.push_back(seq);
            }
        } else {
            let Some(phys_iq) = route_phys_iq(seq, iq, uops, &enqueue_ports_used) else {
                stay_s2.push_back(seq);
                continue;
            };
            let Some(qtag) = allocate_qtag(&pipeline.iq_tags, phys_iq) else {
                stay_s2.push_back(seq);
                continue;
            };
            let queue_idx = phys_iq.index();
            if enqueue_ports_used[queue_idx] >= IQ_ENQUEUE_PORTS {
                stay_s2.push_back(seq);
                continue;
            }
            enqueue_ports_used[queue_idx] += 1;
            uops[seq].phys_iq = Some(phys_iq);
            uops[seq].dst_qtag = Some(qtag);
            pipeline.iq_tags.insert(seq, qtag);
            annotate_qtag_sources(
                seq,
                &pipeline.iq_tags,
                &pipeline.ready_table_t,
                &pipeline.ready_table_u,
                uops,
            );
            register_iq_wait_crossbar_entry(&mut pipeline.qtag_wait_crossbar, seq, &uops[seq]);
            iq.push(crate::make_iq_entry(
                cycle,
                seq,
                phys_iq,
                &pipeline.ready_table_t,
                &pipeline.ready_table_u,
                uops,
            ));
        }
    }
    pipeline.frontend[10] = stay_s2;
    rebuild_iq_owner_table(&mut pipeline.iq_owner_table, iq, &pipeline.iq_tags);
}

pub(crate) fn advance_frontend(pipeline: &mut StageQueues, rob: &mut VecDeque<usize>) {
    for idx in (1..FRONTEND_STAGE_NAMES.len()).rev() {
        let mut prev = std::mem::take(&mut pipeline.frontend[idx - 1]);
        if idx == 7 {
            let needed_rob = prev.iter().filter(|seq| !rob.contains(seq)).count();
            if rob.len() + needed_rob > crate::ROB_CAPACITY {
                pipeline.frontend[idx - 1] = prev;
                continue;
            }
            let mut bypass = VecDeque::new();
            for &seq in &prev {
                if !rob.contains(&seq) {
                    rob.push_back(seq);
                }
                bypass.push_back(seq);
            }
            advance_simple(&mut pipeline.frontend[idx], &mut bypass, DISPATCH_WIDTH);
            pipeline.frontend[idx - 1] = bypass;
        } else {
            advance_simple(&mut pipeline.frontend[idx], &mut prev, DISPATCH_WIDTH);
            pipeline.frontend[idx - 1] = prev;
        }
    }
}

pub(crate) fn issue_queue_candidates(uop: &CycleUop) -> Vec<PhysIq> {
    match uop.decoded.uop_group.as_str() {
        "ALU" => vec![PhysIq::AluIq0, PhysIq::SharedIq1],
        "BRU" => vec![PhysIq::BruIq],
        "LDA/BASE_IMM" | "LDA" | "AGU" => vec![PhysIq::AguIq0, PhysIq::AguIq1],
        "STA/BASE_IMM" | "STA" | "STD" => vec![PhysIq::StdIq0, PhysIq::StdIq1],
        "FSU" => vec![PhysIq::SharedIq1],
        "SYS" => vec![PhysIq::SharedIq1],
        "CMD" | "BBD" => vec![PhysIq::CmdIq],
        _ => vec![PhysIq::SharedIq1],
    }
}

pub(crate) fn d2_bypass(decoded: &DecodedInstruction) -> bool {
    matches!(
        decoded.mnemonic.as_str(),
        "SETRET" | "C.SETRET" | "HL.SETRET"
    ) || decoded.uop_group == "BBD"
}

fn advance_simple(dst: &mut VecDeque<usize>, src: &mut VecDeque<usize>, capacity: usize) {
    while dst.len() < capacity {
        let Some(seq) = src.pop_front() else {
            break;
        };
        dst.push_back(seq);
    }
}
