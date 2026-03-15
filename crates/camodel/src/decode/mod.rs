pub mod builder;
pub mod classify;

use crate::{CycleUop, LogicalQueueTag, QueueWakeKind, REG_T1, REG_U1, d2_bypass};
use isa::{CommitRecord, DecodedInstruction};

pub(crate) fn build_uops(
    commits: &[CommitRecord],
    decoded: &[DecodedInstruction],
) -> Vec<CycleUop> {
    let mut last_writer = [None; 32];
    let mut t_history = Vec::<(usize, LogicalQueueTag)>::new();
    let mut u_history = Vec::<(usize, LogicalQueueTag)>::new();
    let mut next_t_tag = 0usize;
    let mut next_u_tag = 0usize;
    let mut deps = Vec::with_capacity(commits.len());
    let mut src_queue_kinds = Vec::with_capacity(commits.len());
    let mut src_logical_tags = Vec::with_capacity(commits.len());
    let mut dst_queue_kinds = Vec::with_capacity(commits.len());
    let mut dst_logical_tags = Vec::with_capacity(commits.len());
    for (seq, commit) in commits.iter().enumerate() {
        let Some(decoded) = decoded.get(seq) else {
            break;
        };
        let src_kinds = [
            queue_src_kind(commit, decoded, 0),
            queue_src_kind(commit, decoded, 1),
        ];
        let src_tags = [
            src_kinds[0].and_then(|kind| {
                queue_src_rel(commit, decoded, 0)
                    .and_then(|rel| resolve_logical_queue_src(kind, rel, &t_history, &u_history))
                    .map(|(_, tag)| tag)
            }),
            src_kinds[1].and_then(|kind| {
                queue_src_rel(commit, decoded, 1)
                    .and_then(|rel| resolve_logical_queue_src(kind, rel, &t_history, &u_history))
                    .map(|(_, tag)| tag)
            }),
        ];
        let src0 = src_tags[0]
            .and_then(|tag| logical_tag_producer(tag, &t_history, &u_history))
            .or_else(|| {
                if commit.src0_valid != 0 && commit.src0_reg != 0 {
                    last_writer[commit.src0_reg as usize]
                } else {
                    None
                }
            });
        let src1 = src_tags[1]
            .and_then(|tag| logical_tag_producer(tag, &t_history, &u_history))
            .or_else(|| {
                if commit.src1_valid != 0 && commit.src1_reg != 0 {
                    last_writer[commit.src1_reg as usize]
                } else {
                    None
                }
            });
        let dst_kind = queue_dst_kind(commit, decoded);
        let dst_logical_tag =
            allocate_logical_queue_tag(dst_kind, &mut next_t_tag, &mut next_u_tag);
        deps.push([src0, src1]);
        src_queue_kinds.push(src_kinds);
        src_logical_tags.push(src_tags);
        dst_queue_kinds.push(dst_kind);
        dst_logical_tags.push(dst_logical_tag);
        if commit.wb_valid != 0 && commit.wb_rd != 0 {
            last_writer[commit.wb_rd as usize] = Some(deps.len() - 1);
        }
        if let Some(tag) = dst_logical_tag {
            match tag.kind {
                QueueWakeKind::T => t_history.push((seq, tag)),
                QueueWakeKind::U => u_history.push((seq, tag)),
            }
        }
    }

    let mut load_ordinal = 0usize;
    let mut load_store_id = 0usize;
    commits
        .iter()
        .enumerate()
        .filter_map(|(seq, commit)| {
            let is_load = commit.mem_valid != 0 && commit.mem_is_store == 0;
            let is_store = commit.mem_valid != 0 && commit.mem_is_store != 0;
            let this_load_ordinal = is_load.then_some(load_ordinal);
            let this_load_store_id = (is_load || is_store).then_some(load_store_id);
            if is_load {
                load_ordinal += 1;
            }
            if is_load || is_store {
                load_store_id += 1;
            }
            decoded.get(seq).cloned().map(|decoded| {
                let bypass_d2 = d2_bypass(&decoded);
                let redirect_target = architectural_redirect_target(commit, &decoded);
                CycleUop {
                    bypass_d2,
                    src_queue_kinds: src_queue_kinds[seq],
                    src_logical_tags: src_logical_tags[seq],
                    src_qtags: [None, None],
                    dst_queue_kind: dst_queue_kinds[seq],
                    dst_logical_tag: dst_logical_tags[seq],
                    dst_qtag: None,
                    is_load,
                    is_store,
                    load_ordinal: this_load_ordinal,
                    load_store_id: this_load_store_id,
                    miss_injected: false,
                    decoded,
                    commit: commit.clone(),
                    deps: deps[seq],
                    phys_iq: None,
                    pick_wakeup_visible: None,
                    data_ready_visible: None,
                    miss_pending_until: None,
                    redirect_target,
                    e1_cycle: None,
                    e4_cycle: None,
                    w1_cycle: None,
                    done_cycle: None,
                }
            })
        })
        .collect::<Vec<_>>()
}

pub(crate) fn checkpoint_id_for_seq(seq: usize, uops: &[CycleUop]) -> u8 {
    let Some(packet_head_seq) = fetch_packet_head_seq(seq, uops) else {
        return 0;
    };
    packet_checkpoint_id(uops[packet_head_seq].commit.pc)
}

pub(crate) fn rob_checkpoint_id_for_seq(seq: usize, uops: &[CycleUop]) -> u8 {
    if !is_start_marker(&uops[seq].decoded) {
        return 0;
    }
    let packet_checkpoint = checkpoint_id_for_seq(seq, uops);
    let slot = packet_slot_for_seq(seq, uops).unwrap_or(0) as u8;
    packet_checkpoint.wrapping_add(slot) & 0x3f
}

pub(crate) fn packet_checkpoint_id(packet_head_pc: u64) -> u8 {
    ((packet_head_pc >> 2) & 0x3f) as u8
}

fn fetch_packet_head_seq(seq: usize, uops: &[CycleUop]) -> Option<usize> {
    let mut head = 0usize;
    while head < uops.len() {
        let mut packet_end = head;
        while packet_end + 1 < uops.len() && (packet_end - head + 1) < crate::FETCH_WIDTH {
            if uops[packet_end].redirect_target.is_some() {
                break;
            }
            packet_end += 1;
        }
        if seq >= head && seq <= packet_end {
            return Some(head);
        }
        head = packet_end.saturating_add(1);
    }
    None
}

fn packet_slot_for_seq(seq: usize, uops: &[CycleUop]) -> Option<usize> {
    let head = fetch_packet_head_seq(seq, uops)?;
    Some(seq.saturating_sub(head))
}

pub(crate) fn architectural_redirect_target(
    commit: &CommitRecord,
    decoded: &DecodedInstruction,
) -> Option<u64> {
    let fallthrough = commit.pc.saturating_add(commit.len as u64);
    (commit.trap_valid == 0 && commit.next_pc != fallthrough && is_boundary_redirect_owner(decoded))
        .then_some(commit.next_pc)
}

pub(crate) fn deferred_bru_correction_target(
    commit: &CommitRecord,
    decoded: &DecodedInstruction,
) -> Option<u64> {
    let fallthrough = commit.pc.saturating_add(commit.len as u64);
    (commit.trap_valid == 0
        && commit.next_pc != fallthrough
        && decoded.uop_group == "BRU"
        && !is_boundary_redirect_owner(decoded))
    .then_some(commit.next_pc)
}

pub(crate) fn block_epoch_for_seq(seq: usize, uops: &[CycleUop]) -> u16 {
    let mut epoch = 0u16;
    let mut block_head = true;
    for (idx, uop) in uops.iter().enumerate().take(seq.saturating_add(1)) {
        if idx == seq {
            return epoch;
        }
        let is_bstart_head = is_bstart(decoded_ref(uop)) && block_head;
        let is_bstart_mid = is_bstart(decoded_ref(uop)) && !block_head;
        let is_boundary =
            is_bstart_mid || is_bstop(decoded_ref(uop)) || is_macro_boundary(decoded_ref(uop));
        if is_boundary {
            block_head = true;
        }
        if is_bstart_head {
            block_head = false;
        }
        if is_boundary || is_bstart_head {
            epoch = epoch.wrapping_add(1);
        }
    }
    epoch
}

fn decoded_ref(uop: &CycleUop) -> &DecodedInstruction {
    &uop.decoded
}

pub(crate) fn is_bstart(decoded: &DecodedInstruction) -> bool {
    matches!(
        decoded.mnemonic.as_str(),
        "BSTART"
            | "BSTART.ACCCVT"
            | "BSTART.CALL"
            | "BSTART.CUBE"
            | "BSTART.FIXP"
            | "BSTART.FP"
            | "BSTART.MPAR"
            | "BSTART.MSEQ"
            | "BSTART.PAR"
            | "BSTART.STD"
            | "BSTART.SYS"
            | "BSTART.TEPL"
            | "BSTART.TLOAD"
            | "BSTART.TMA"
            | "BSTART.TMATMUL"
            | "BSTART.TMATMUL.ACC"
            | "BSTART.TMOV"
            | "BSTART.TSTORE"
            | "BSTART.VPAR"
            | "BSTART.VSEQ"
            | "C.BSTART"
            | "C.BSTART.STD"
            | "C.BSTART.SYS"
            | "C.BSTART.VPAR"
            | "C.BSTART.VSEQ"
            | "HL.BSTART CALL"
            | "HL.BSTART.FP"
            | "HL.BSTART.STD"
            | "HL.BSTART.SYS"
    )
}

pub(crate) fn is_bstop(decoded: &DecodedInstruction) -> bool {
    matches!(decoded.mnemonic.as_str(), "BSTOP" | "C.BSTOP")
}

pub(crate) fn is_macro_boundary(decoded: &DecodedInstruction) -> bool {
    matches!(
        decoded.mnemonic.as_str(),
        "FENTRY" | "FEXIT" | "FRET.RA" | "FRET.STK"
    )
}

pub(crate) fn is_start_marker(decoded: &DecodedInstruction) -> bool {
    is_bstart(decoded) || is_macro_boundary(decoded)
}

pub(crate) fn is_boundary_redirect_owner(decoded: &DecodedInstruction) -> bool {
    is_bstart(decoded) || is_bstop(decoded) || is_macro_boundary(decoded)
}

pub(crate) fn is_legal_redirect_restart(decoded: &DecodedInstruction) -> bool {
    is_bstart(decoded) || is_macro_boundary(decoded)
}

pub(crate) fn legal_redirect_restart_seq(
    source_seq: usize,
    target_pc: u64,
    uops: &[CycleUop],
) -> Option<usize> {
    uops.iter()
        .enumerate()
        .skip(source_seq.saturating_add(1))
        .find_map(|(seq, uop)| {
            (uop.commit.pc == target_pc && is_legal_redirect_restart(&uop.decoded)).then_some(seq)
        })
}

fn queue_src_kind(
    commit: &CommitRecord,
    decoded: &DecodedInstruction,
    idx: usize,
) -> Option<QueueWakeKind> {
    let reg = match idx {
        0 if commit.src0_valid != 0 => commit.src0_reg,
        1 if commit.src1_valid != 0 => commit.src1_reg,
        _ => return None,
    };
    let asm = decoded.asm.to_ascii_lowercase();
    match reg {
        REG_T1 if asm.contains("t#") => Some(QueueWakeKind::T),
        REG_U1 if asm.contains("u#") => Some(QueueWakeKind::U),
        _ => None,
    }
}

fn queue_src_rel(commit: &CommitRecord, decoded: &DecodedInstruction, idx: usize) -> Option<usize> {
    let kind = queue_src_kind(commit, decoded, idx)?;
    let asm = decoded.asm.to_ascii_lowercase();
    parse_queue_relative(
        &asm,
        match kind {
            QueueWakeKind::T => "t#",
            QueueWakeKind::U => "u#",
        },
    )
}

fn parse_queue_relative(asm: &str, prefix: &str) -> Option<usize> {
    let start = asm.find(prefix)?;
    let digits = asm[start + prefix.len()..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

fn queue_dst_kind(commit: &CommitRecord, decoded: &DecodedInstruction) -> Option<QueueWakeKind> {
    if commit.wb_valid == 0 {
        return None;
    }
    let asm = decoded.asm.to_ascii_lowercase();
    match commit.wb_rd {
        REG_T1 if asm.contains("->t") => Some(QueueWakeKind::T),
        REG_U1 if asm.contains("->u") => Some(QueueWakeKind::U),
        _ => None,
    }
}

fn allocate_logical_queue_tag(
    kind: Option<QueueWakeKind>,
    next_t_tag: &mut usize,
    next_u_tag: &mut usize,
) -> Option<LogicalQueueTag> {
    match kind {
        Some(QueueWakeKind::T) => {
            let tag = LogicalQueueTag {
                kind: QueueWakeKind::T,
                tag: *next_t_tag,
            };
            *next_t_tag += 1;
            Some(tag)
        }
        Some(QueueWakeKind::U) => {
            let tag = LogicalQueueTag {
                kind: QueueWakeKind::U,
                tag: *next_u_tag,
            };
            *next_u_tag += 1;
            Some(tag)
        }
        None => None,
    }
}

fn resolve_logical_queue_src(
    kind: QueueWakeKind,
    rel: usize,
    t_history: &[(usize, LogicalQueueTag)],
    u_history: &[(usize, LogicalQueueTag)],
) -> Option<(usize, LogicalQueueTag)> {
    if rel == 0 {
        return None;
    }
    match kind {
        QueueWakeKind::T => t_history.iter().rev().nth(rel - 1).copied(),
        QueueWakeKind::U => u_history.iter().rev().nth(rel - 1).copied(),
    }
}

fn logical_tag_producer(
    tag: LogicalQueueTag,
    t_history: &[(usize, LogicalQueueTag)],
    u_history: &[(usize, LogicalQueueTag)],
) -> Option<usize> {
    match tag.kind {
        QueueWakeKind::T => t_history
            .iter()
            .find(|(_, candidate)| *candidate == tag)
            .map(|(seq, _)| *seq),
        QueueWakeKind::U => u_history
            .iter()
            .find(|(_, candidate)| *candidate == tag)
            .map(|(seq, _)| *seq),
    }
}
