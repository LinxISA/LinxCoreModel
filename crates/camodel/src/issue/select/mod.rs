pub mod i1;
pub mod i2;
pub mod p1;

use std::collections::{BTreeSet, VecDeque};

use isa::CommitRecord;

#[cfg(test)]
use crate::make_iq_entry;
use crate::{
    CycleRunOptions, CycleUop, ISSUE_WIDTH, IqEntry, LD_GEN_E1, LD_GEN_E2, LD_GEN_E3, LD_GEN_E4,
    PhysIq, READ_PORTS, StageQueues, e1_can_accept, lhq_insert, rebuild_iq_owner_table,
    rob_age_rank, stq_insert, unregister_iq_wait_crossbar_seq,
};

pub(crate) fn dep_pick_ready_cycle(producer: usize, uops: &[CycleUop]) -> u64 {
    uops[producer].pick_wakeup_visible.unwrap_or(u64::MAX)
}

pub(crate) fn dep_data_ready_cycle(producer: usize, uops: &[CycleUop]) -> u64 {
    uops[producer].data_ready_visible.unwrap_or(u64::MAX)
}

pub(crate) fn source_valid(commit: &CommitRecord, idx: usize) -> bool {
    match idx {
        0 => commit.src0_valid != 0,
        1 => commit.src1_valid != 0,
        _ => false,
    }
}

pub(crate) fn source_uses_qtag_wakeup(seq: usize, idx: usize, uops: &[CycleUop]) -> bool {
    let Some(kind) = uops[seq].src_queue_kinds[idx] else {
        return false;
    };
    let Some(producer) = uops[seq].deps[idx] else {
        return false;
    };
    uops[seq].src_qtags[idx].is_some()
        && uops[producer].dst_queue_kind == Some(kind)
        && uops[producer].dst_logical_tag == uops[seq].src_logical_tags[idx]
}

pub(crate) fn arbitrate_i1(
    cycle: u64,
    p1: &mut VecDeque<usize>,
    iq: &mut Vec<IqEntry>,
    uops: &[CycleUop],
    rob: &VecDeque<usize>,
) -> VecDeque<usize> {
    let mut admitted = VecDeque::new();
    let mut used_ports = 0usize;
    let mut used_queues = BTreeSet::new();
    let mut attempts = p1.drain(..).collect::<Vec<_>>();
    attempts.sort_by_key(|&seq| rob_age_rank(seq, rob));
    for seq in attempts {
        let Some(entry) = iq.iter().find(|entry| entry.seq == seq).cloned() else {
            continue;
        };
        let needed = read_ports_needed_from_entry(&entry, cycle, uops);
        if !used_queues.contains(&entry.phys_iq.index()) && used_ports + needed <= READ_PORTS {
            used_ports += needed;
            used_queues.insert(entry.phys_iq.index());
            admitted.push_back(seq);
        } else if let Some(entry) = iq.iter_mut().find(|entry| entry.seq == seq) {
            entry.inflight = false;
        }
    }
    admitted
}

#[cfg(test)]
pub(crate) fn read_ports_needed(seq: usize, cycle: u64, uops: &[CycleUop]) -> usize {
    let entry = make_iq_entry(
        cycle,
        seq,
        uops[seq].phys_iq.unwrap_or(PhysIq::SharedIq1),
        &BTreeSet::new(),
        &BTreeSet::new(),
        uops,
    );
    read_ports_needed_from_entry(&entry, cycle, uops)
}

#[cfg(test)]
pub(crate) fn iq_entry_ready(
    seq: usize,
    cycle: u64,
    lsid_issue_ptr: usize,
    uops: &[CycleUop],
) -> bool {
    let entry = make_iq_entry(
        cycle,
        seq,
        uops[seq].phys_iq.unwrap_or(PhysIq::SharedIq1),
        &BTreeSet::new(),
        &BTreeSet::new(),
        uops,
    );
    iq_entry_ready_from_state(&entry, cycle, lsid_issue_ptr, uops)
}

#[cfg(test)]
pub(crate) fn iq_entry_wait_cause(
    seq: usize,
    cycle: u64,
    lsid_issue_ptr: usize,
    uops: &[CycleUop],
) -> Option<&'static str> {
    let entry = make_iq_entry(
        cycle,
        seq,
        uops[seq].phys_iq.unwrap_or(PhysIq::SharedIq1),
        &BTreeSet::new(),
        &BTreeSet::new(),
        uops,
    );
    iq_entry_wait_cause_from_state(&entry, cycle, lsid_issue_ptr, uops)
}

pub(crate) fn read_ports_needed_from_entry(
    entry: &IqEntry,
    cycle: u64,
    uops: &[CycleUop],
) -> usize {
    let seq = entry.seq;
    uops[seq]
        .deps
        .into_iter()
        .enumerate()
        .filter(|(idx, dep)| {
            source_needs_rf_read_from_entry(entry, &uops[seq].commit, *idx, *dep, cycle, uops)
        })
        .count()
}

pub(crate) fn source_needs_rf_read_from_entry(
    entry: &IqEntry,
    commit: &CommitRecord,
    idx: usize,
    dep: Option<usize>,
    cycle: u64,
    uops: &[CycleUop],
) -> bool {
    if !source_valid(commit, idx) {
        return false;
    }
    if source_uses_qtag_wakeup(entry.seq, idx, uops) {
        return false;
    }
    if entry.src_ready_spec[idx] {
        return false;
    }

    match dep {
        None => true,
        Some(producer) => {
            if dep_data_ready_cycle(producer, uops) <= cycle {
                return true;
            }

            !(uops[producer].is_load && dep_pick_ready_cycle(producer, uops) <= cycle)
        }
    }
}

pub(crate) fn i2_ready(seq: usize, cycle: u64, uops: &[CycleUop]) -> bool {
    uops[seq].deps.into_iter().all(|dep| {
        dep.map(|producer| dep_data_ready_cycle(producer, uops) <= cycle)
            .unwrap_or(true)
    })
}

pub(crate) fn lsid_issue_ready(seq: usize, lsid_issue_ptr: usize, uops: &[CycleUop]) -> bool {
    uops[seq]
        .load_store_id
        .map(|load_store_id| load_store_id == lsid_issue_ptr)
        .unwrap_or(true)
}

pub(crate) fn i2_issue_eligible(
    seq: usize,
    cycle: u64,
    lsid_issue_ptr: usize,
    uops: &[CycleUop],
) -> bool {
    i2_ready(seq, cycle, uops) && lsid_issue_ready(seq, lsid_issue_ptr, uops)
}

pub(crate) fn i2_waits_on_lsid(
    seq: usize,
    cycle: u64,
    lsid_issue_ptr: usize,
    uops: &[CycleUop],
) -> bool {
    i2_ready(seq, cycle, uops) && !lsid_issue_ready(seq, lsid_issue_ptr, uops)
}

pub(crate) fn advance_i2(
    cycle: u64,
    i2: &mut VecDeque<usize>,
    e1: &mut VecDeque<usize>,
    lhq: &mut VecDeque<usize>,
    stq: &mut VecDeque<usize>,
    lsid_issue_ptr: &mut usize,
    lsid_complete_ptr: &mut usize,
    uops: &[CycleUop],
) {
    let mut stay = VecDeque::new();
    while let Some(seq) = i2.pop_front() {
        if i2_issue_eligible(seq, cycle, *lsid_issue_ptr, uops) && e1_can_accept(seq, e1, uops) {
            e1.push_back(seq);
            if uops[seq].is_load {
                lhq_insert(lhq, seq);
            } else if uops[seq].is_store {
                stq_insert(stq, seq);
            }
            if uops[seq].load_store_id.is_some() {
                *lsid_issue_ptr += 1;
                *lsid_complete_ptr += 1;
            }
        } else {
            stay.push_back(seq);
        }
    }
    *i2 = stay;
}

pub(crate) fn advance_p1_to_i1(
    i1: &mut VecDeque<usize>,
    admitted_i1: &mut VecDeque<usize>,
    p1: &mut VecDeque<usize>,
) {
    advance_simple(i1, admitted_i1, ISSUE_WIDTH);
    while let Some(seq) = admitted_i1.pop_front() {
        p1.push_back(seq);
    }
}

pub(crate) fn advance_i1_to_i2(pipeline: &mut StageQueues, iq: &mut Vec<IqEntry>) {
    let mut prev_i1 = std::mem::take(&mut pipeline.i1);
    let mut moved = Vec::new();
    let mut stay = VecDeque::new();
    let mut used_queues = BTreeSet::new();
    while let Some(seq) = prev_i1.pop_front() {
        if pipeline.i2.len() >= ISSUE_WIDTH {
            stay.push_back(seq);
            continue;
        }
        let Some(phys_iq) = iq
            .iter()
            .find(|entry| entry.seq == seq)
            .map(|entry| entry.phys_iq)
        else {
            continue;
        };
        if used_queues.contains(&phys_iq.index()) {
            stay.push_back(seq);
            continue;
        }
        used_queues.insert(phys_iq.index());
        pipeline.i2.push_back(seq);
        moved.push(seq);
    }
    stay.extend(prev_i1);
    iq.retain(|entry| !moved.contains(&entry.seq));
    for seq in moved {
        pipeline.iq_tags.remove(&seq);
        unregister_iq_wait_crossbar_seq(&mut pipeline.qtag_wait_crossbar, seq);
    }
    rebuild_iq_owner_table(&mut pipeline.iq_owner_table, iq, &pipeline.iq_tags);
    pipeline.i1 = stay;
}

pub(crate) fn pick_from_iq(
    cycle: u64,
    lsid_issue_ptr: usize,
    iq: &mut [IqEntry],
    uops: &[CycleUop],
    p1: &mut VecDeque<usize>,
    rob: &VecDeque<usize>,
) {
    let queue_winners = ready_iq_winners(cycle, lsid_issue_ptr, iq, uops, rob);

    let mut candidates = queue_winners
        .into_iter()
        .map(|(idx, seq, _)| (idx, seq))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|&(_, seq)| rob_age_rank(seq, rob));

    for (idx, seq) in candidates {
        if p1.len() >= ISSUE_WIDTH {
            break;
        }
        iq[idx].inflight = true;
        p1.push_back(seq);
    }
}

pub(crate) fn iq_entry_ready_from_state(
    entry: &IqEntry,
    cycle: u64,
    lsid_issue_ptr: usize,
    uops: &[CycleUop],
) -> bool {
    iq_entry_wait_cause_from_state(entry, cycle, lsid_issue_ptr, uops).is_none()
}

pub(crate) fn ready_iq_winners(
    cycle: u64,
    lsid_issue_ptr: usize,
    iq: &[IqEntry],
    uops: &[CycleUop],
    rob: &VecDeque<usize>,
) -> Vec<(usize, usize, PhysIq)> {
    let mut queue_winners = iq
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            (!entry.inflight && iq_entry_ready_from_state(entry, cycle, lsid_issue_ptr, uops))
                .then_some((idx, entry.seq, entry.phys_iq))
        })
        .collect::<Vec<_>>();
    queue_winners.sort_by_key(|&(_, seq, phys_iq)| (phys_iq.index(), rob_age_rank(seq, rob)));
    queue_winners.dedup_by_key(|entry| entry.2);
    queue_winners
}

pub(crate) fn iq_entry_wait_cause_from_state(
    entry: &IqEntry,
    cycle: u64,
    lsid_issue_ptr: usize,
    uops: &[CycleUop],
) -> Option<&'static str> {
    let seq = entry.seq;
    if !lsid_issue_ready(seq, lsid_issue_ptr, uops) {
        return Some("wait_lsid");
    }

    let miss_pending = miss_pending_active(cycle, uops);
    for (idx, dep) in uops[seq].deps.into_iter().enumerate() {
        if let Some(dep) = dep {
            if miss_pending && (dep_load_gen_vec(dep, cycle, uops) & LD_GEN_E4 != 0) {
                return Some("wait_miss");
            }
        }
        if entry.src_valid[idx] && !(entry.src_ready_nonspec[idx] || entry.src_ready_spec[idx]) {
            return Some(if entry.src_wait_qtag[idx] {
                "wait_qtag"
            } else {
                "wait_dep"
            });
        }
    }

    None
}

pub(crate) fn miss_pending_active(cycle: u64, uops: &[CycleUop]) -> bool {
    let _ = cycle;
    uops.iter().any(|uop| {
        uop.is_load
            && uop.miss_injected
            && uop.done_cycle.is_none()
            && (uop.miss_pending_until.is_some()
                || uop.e1_cycle.is_some()
                || uop.e4_cycle.is_some()
                || uop.w1_cycle.is_some())
    })
}

pub(crate) fn dep_load_gen_vec(seq: usize, cycle: u64, uops: &[CycleUop]) -> u8 {
    let mut memo = vec![None; uops.len()];
    dep_load_gen_vec_inner(seq, cycle, uops, &mut memo)
}

fn dep_load_gen_vec_inner(
    seq: usize,
    cycle: u64,
    uops: &[CycleUop],
    memo: &mut [Option<u8>],
) -> u8 {
    if let Some(mask) = memo[seq] {
        return mask;
    }

    let mut mask = current_load_stage_mask(seq, cycle, uops);
    for dep in uops[seq].deps.into_iter().flatten() {
        mask |= dep_load_gen_vec_inner(dep, cycle, uops, memo);
    }
    memo[seq] = Some(mask);
    mask
}

fn current_load_stage_mask(seq: usize, cycle: u64, uops: &[CycleUop]) -> u8 {
    let uop = &uops[seq];
    if !uop.is_load {
        return 0;
    }

    if uop.miss_pending_until.is_some() {
        return LD_GEN_E4;
    }

    match (uop.e1_cycle, uop.e4_cycle) {
        (_, Some(e4_cycle)) if cycle == e4_cycle => LD_GEN_E4,
        (Some(e1_cycle), _) if cycle >= e1_cycle => match cycle - e1_cycle {
            0 => LD_GEN_E1,
            1 => LD_GEN_E2,
            2 => LD_GEN_E3,
            _ => 0,
        },
        _ => 0,
    }
}

pub(crate) fn should_inject_load_miss(
    seq: usize,
    options: &CycleRunOptions,
    uops: &[CycleUop],
) -> bool {
    let Some(every) = options.load_miss_every else {
        return false;
    };
    if every == 0 {
        return false;
    }

    let uop = &uops[seq];
    uop.is_load
        && !uop.miss_injected
        && uop
            .load_ordinal
            .is_some_and(|ordinal| ((ordinal as u64) + 1) % every == 0)
}

fn advance_simple(dst: &mut VecDeque<usize>, src: &mut VecDeque<usize>, capacity: usize) {
    while dst.len() < capacity {
        let Some(seq) = src.pop_front() else {
            break;
        };
        dst.push_back(seq);
    }
}
