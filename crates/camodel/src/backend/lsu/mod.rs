pub mod l1d;
pub mod lhq;
pub mod liq;
pub mod mdb;
pub mod scb;
pub mod stq;

use std::collections::VecDeque;

use crate::{
    CycleRunOptions, CycleUop, ISSUE_WIDTH, L1D_WIDTH, L1dEntry, L1dTxnKind, LSU_WIDTH, MdbEntry,
    ScbEntry, StageQueues, rob_age_rank, should_inject_load_miss,
};

pub(crate) fn advance_execute(
    cycle: u64,
    pipeline: &mut StageQueues,
    uops: &mut [CycleUop],
    options: &CycleRunOptions,
) {
    pipeline.w2.clear();
    let mut prev_w1 = std::mem::take(&mut pipeline.w1);
    advance_simple(&mut pipeline.w2, &mut prev_w1, ISSUE_WIDTH);
    pipeline.w1 = prev_w1;

    let mut prev_e4 = std::mem::take(&mut pipeline.e4);
    let mut stay_e4 = VecDeque::new();
    while let Some(seq) = prev_e4.pop_front() {
        if should_inject_load_miss(seq, options, uops) {
            let restart_cycle = cycle.saturating_add(options.load_miss_penalty.max(1));
            let uop = &mut uops[seq];
            uop.miss_injected = true;
            uop.miss_pending_until = Some(restart_cycle);
            uop.pick_wakeup_visible = None;
            uop.data_ready_visible = None;
            uop.e1_cycle = None;
            uop.e4_cycle = None;
            uop.w1_cycle = None;
            uop.done_cycle = None;
            pipeline.liq.push_back(crate::LiqEntry {
                seq,
                refill_ready_cycle: restart_cycle,
            });
            pipeline.mdb.push_back(MdbEntry {
                seq,
                refill_ready_cycle: restart_cycle,
            });
            remove_queue_entry(&mut pipeline.lhq, seq);
        } else if lsid_cache_ready(seq, pipeline.lsid_cache_ptr, uops)
            && can_accept_l1d(&pipeline.l1d)
        {
            uops[seq].miss_pending_until = None;
            pipeline.l1d.push_back(L1dEntry {
                seq,
                kind: L1dTxnKind::LoadHit,
                ready_cycle: cycle.saturating_add(1),
            });
        } else {
            stay_e4.push_back(seq);
        }
    }
    pipeline.e4 = stay_e4;

    let mut prev_e3 = std::mem::take(&mut pipeline.e3);
    advance_simple(&mut pipeline.e4, &mut prev_e3, LSU_WIDTH);
    pipeline.e3 = prev_e3;

    let mut prev_e2 = std::mem::take(&mut pipeline.e2);
    advance_simple(&mut pipeline.e3, &mut prev_e2, LSU_WIDTH);
    pipeline.e2 = prev_e2;

    let mut stay_e1 = VecDeque::new();
    let prev_e1 = std::mem::take(&mut pipeline.e1);
    for seq in prev_e1 {
        if uops[seq].is_load {
            if pipeline.e2.len() < LSU_WIDTH {
                pipeline.e2.push_back(seq);
            } else {
                stay_e1.push_back(seq);
            }
        } else if pipeline.w1.len() < ISSUE_WIDTH {
            pipeline.w1.push_back(seq);
        } else {
            stay_e1.push_back(seq);
        }
    }
    pipeline.e1 = stay_e1;

    for entry in &mut pipeline.w2 {
        if uops[*entry].done_cycle.is_none() && uops[*entry].w1_cycle == Some(cycle) {
            // keep order stable; completion is tagged in `tag_stage_cycles`.
        }
    }
}

pub(crate) fn advance_liq(
    cycle: u64,
    pipeline: &mut StageQueues,
    uops: &mut [CycleUop],
    rob: &VecDeque<usize>,
) {
    if !load_slot_available(&pipeline.e1, uops) {
        return;
    }

    let mut entries = std::mem::take(&mut pipeline.liq)
        .into_iter()
        .collect::<Vec<_>>();
    let Some((selected_idx, _)) = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry.refill_ready_cycle <= cycle
                && lsid_cache_ready(entry.seq, pipeline.lsid_cache_ptr, uops)
        })
        .min_by_key(|(_, entry)| rob_age_rank(entry.seq, rob))
    else {
        pipeline.liq = entries.into_iter().collect();
        return;
    };

    let selected = entries.remove(selected_idx);
    uops[selected.seq].miss_pending_until = None;
    lhq_insert(&mut pipeline.lhq, selected.seq);
    remove_mdb_entry(&mut pipeline.mdb, selected.seq);
    pipeline.e1.push_back(selected.seq);
    pipeline.liq = entries.into_iter().collect();
}

pub(crate) fn advance_l1d(cycle: u64, pipeline: &mut StageQueues) {
    let mut remaining = std::mem::take(&mut pipeline.l1d);
    while let Some(entry) = remaining.pop_front() {
        if entry.ready_cycle > cycle {
            pipeline.l1d.push_back(entry);
            continue;
        }

        match entry.kind {
            L1dTxnKind::LoadHit => {
                if pipeline.w1.len() < ISSUE_WIDTH {
                    remove_queue_entry(&mut pipeline.lhq, entry.seq);
                    pipeline.w1.push_back(entry.seq);
                    pipeline.lsid_cache_ptr = pipeline.lsid_cache_ptr.saturating_add(1);
                } else {
                    pipeline.l1d.push_back(entry);
                }
            }
            L1dTxnKind::StoreDrain => {
                pipeline.lsid_cache_ptr = pipeline.lsid_cache_ptr.saturating_add(1);
            }
        }
    }
    for entry in remaining {
        pipeline.l1d.push_back(entry);
    }
}

pub(crate) fn lhq_insert(lhq: &mut VecDeque<usize>, seq: usize) {
    if !lhq.contains(&seq) {
        lhq.push_back(seq);
    }
}

pub(crate) fn stq_insert(stq: &mut VecDeque<usize>, seq: usize) {
    if !stq.contains(&seq) {
        stq.push_back(seq);
    }
}

pub(crate) fn remove_queue_entry(queue: &mut VecDeque<usize>, seq: usize) {
    queue.retain(|&entry_seq| entry_seq != seq);
}

pub(crate) fn remove_mdb_entry(mdb: &mut VecDeque<MdbEntry>, seq: usize) {
    mdb.retain(|entry| entry.seq != seq);
}

pub(crate) fn advance_scb(cycle: u64, pipeline: &mut StageQueues, uops: &[CycleUop]) {
    while can_accept_l1d(&pipeline.l1d) {
        let Some(idx) = pipeline.scb.iter().position(|entry| {
            scb_entry_drain_ready(entry, cycle)
                && lsid_cache_ready(entry.seq, pipeline.lsid_cache_ptr, uops)
        }) else {
            break;
        };
        let entry = pipeline
            .scb
            .remove(idx)
            .expect("ready SCB entry should exist");
        pipeline.l1d.push_back(L1dEntry {
            seq: entry.seq,
            kind: L1dTxnKind::StoreDrain,
            ready_cycle: cycle.saturating_add(1),
        });
    }
}

fn scb_entry_drain_ready(entry: &ScbEntry, cycle: u64) -> bool {
    entry.enqueue_cycle < cycle
}

pub(crate) fn load_forward_visible(seq: usize, pipeline: &StageQueues, uops: &[CycleUop]) -> bool {
    if !uops[seq].is_load {
        return false;
    }
    pipeline
        .stq
        .iter()
        .copied()
        .any(|store_seq| store_seq < seq && store_matches_load(store_seq, seq, uops))
        || pipeline
            .scb
            .iter()
            .any(|entry| entry.seq < seq && store_matches_load(entry.seq, seq, uops))
        || pipeline.l1d.iter().any(|entry| {
            entry.kind == L1dTxnKind::StoreDrain
                && entry.seq < seq
                && store_matches_load(entry.seq, seq, uops)
        })
}

fn store_matches_load(store_seq: usize, load_seq: usize, uops: &[CycleUop]) -> bool {
    let store = &uops[store_seq].commit;
    let load = &uops[load_seq].commit;
    uops[store_seq].is_store
        && uops[load_seq].is_load
        && store.mem_addr == load.mem_addr
        && store.mem_size == load.mem_size
}

pub(crate) fn e1_can_accept(seq: usize, e1: &VecDeque<usize>, uops: &[CycleUop]) -> bool {
    e1.len() < ISSUE_WIDTH && (!uops[seq].is_load || load_slot_available(e1, uops))
}

pub(crate) fn load_slot_available(queue: &VecDeque<usize>, uops: &[CycleUop]) -> bool {
    queue.iter().filter(|&&seq| uops[seq].is_load).count() < LSU_WIDTH
}

pub(crate) fn can_accept_l1d(l1d: &VecDeque<L1dEntry>) -> bool {
    l1d.len() < L1D_WIDTH
}

pub(crate) fn lsid_cache_ready(seq: usize, lsid_cache_ptr: usize, uops: &[CycleUop]) -> bool {
    uops[seq]
        .load_store_id
        .map(|load_store_id| load_store_id == lsid_cache_ptr)
        .unwrap_or(true)
}

fn advance_simple(dst: &mut VecDeque<usize>, src: &mut VecDeque<usize>, capacity: usize) {
    while dst.len() < capacity {
        let Some(seq) = src.pop_front() else {
            break;
        };
        dst.push_back(seq);
    }
}
