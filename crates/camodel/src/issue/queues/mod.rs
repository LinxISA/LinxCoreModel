pub mod iq;
pub mod qtag;
pub mod ready_tables;

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    CycleUop, IQ_ENQUEUE_PORTS, IqEntry, IqWakeEvent, IqWakeKind, LogicalQueueTag, PHYS_IQ_COUNT,
    PhysIq, QTag, QueueWakeKind, ReadyTableCheckpoint, StageQueues, dep_data_ready_cycle,
    dep_pick_ready_cycle, issue_queue_candidates, source_uses_qtag_wakeup, source_valid,
};

pub(crate) fn route_phys_iq(
    seq: usize,
    iq: &[IqEntry],
    uops: &[CycleUop],
    enqueue_ports_used: &[usize; PHYS_IQ_COUNT],
) -> Option<PhysIq> {
    let candidates = issue_queue_candidates(&uops[seq]);
    candidates.into_iter().find(|&phys_iq| {
        enqueue_ports_used[phys_iq.index()] < IQ_ENQUEUE_PORTS
            && iq_occupancy(iq, phys_iq) < phys_iq.capacity()
    })
}

fn iq_occupancy(iq: &[IqEntry], phys_iq: PhysIq) -> usize {
    iq.iter().filter(|entry| entry.phys_iq == phys_iq).count()
}

pub(crate) fn insert_ready_table_tag(pipeline: &mut StageQueues, tag: LogicalQueueTag) {
    match tag.kind {
        QueueWakeKind::T => {
            pipeline.ready_table_t.insert(tag.tag);
        }
        QueueWakeKind::U => {
            pipeline.ready_table_u.insert(tag.tag);
        }
    }
}

pub(crate) fn snapshot_ready_tables_for_checkpoint(pipeline: &mut StageQueues, checkpoint_id: u8) {
    pipeline.ready_table_checkpoints.insert(
        checkpoint_id,
        ReadyTableCheckpoint {
            ready_table_t: pipeline.ready_table_t.clone(),
            ready_table_u: pipeline.ready_table_u.clone(),
            recovery_epoch: pipeline.active_recovery_epoch,
            block_head: pipeline.active_block_head,
            branch_context: pipeline.active_branch_context,
            dynamic_target_pc: pipeline.active_dynamic_target_pc,
            dynamic_target_owner_seq: pipeline.active_dynamic_target_owner_seq,
            dynamic_target_producer_kind: pipeline.active_dynamic_target_producer_kind,
            dynamic_target_setup_epoch: pipeline.active_dynamic_target_setup_epoch,
            dynamic_target_owner_kind: pipeline.active_dynamic_target_owner_kind,
            dynamic_target_source_owner_seq: pipeline.active_dynamic_target_source_owner_seq,
            dynamic_target_source_epoch: pipeline.active_dynamic_target_source_epoch,
            dynamic_target_source_kind: pipeline.active_dynamic_target_source_kind,
            dynamic_target_call_materialization_kind: pipeline
                .active_dynamic_target_call_materialization_kind,
            call_header_seq: pipeline.active_call_header_seq,
            call_return_target_pc: pipeline.active_call_return_target_pc,
            call_return_target_owner_seq: pipeline.active_call_return_target_owner_seq,
            call_return_target_epoch: pipeline.active_call_return_target_epoch,
            call_return_materialization_kind: pipeline.active_call_return_materialization_kind,
        },
    );
}

pub(crate) fn restore_ready_tables_for_checkpoint(
    pipeline: &mut StageQueues,
    checkpoint_id: u8,
) -> bool {
    let Some(snapshot) = pipeline
        .ready_table_checkpoints
        .get(&checkpoint_id)
        .cloned()
    else {
        return false;
    };
    pipeline.ready_table_t = snapshot.ready_table_t;
    pipeline.ready_table_u = snapshot.ready_table_u;
    pipeline.active_recovery_epoch = snapshot.recovery_epoch;
    pipeline.active_block_head = snapshot.block_head;
    pipeline.active_branch_context = snapshot.branch_context;
    pipeline.active_dynamic_target_pc = snapshot.dynamic_target_pc;
    pipeline.active_dynamic_target_owner_seq = snapshot.dynamic_target_owner_seq;
    pipeline.active_dynamic_target_producer_kind = snapshot.dynamic_target_producer_kind;
    pipeline.active_dynamic_target_setup_epoch = snapshot.dynamic_target_setup_epoch;
    pipeline.active_dynamic_target_owner_kind = snapshot.dynamic_target_owner_kind;
    pipeline.active_dynamic_target_source_owner_seq = snapshot.dynamic_target_source_owner_seq;
    pipeline.active_dynamic_target_source_epoch = snapshot.dynamic_target_source_epoch;
    pipeline.active_dynamic_target_source_kind = snapshot.dynamic_target_source_kind;
    pipeline.active_dynamic_target_call_materialization_kind =
        snapshot.dynamic_target_call_materialization_kind;
    pipeline.active_call_header_seq = snapshot.call_header_seq;
    pipeline.active_call_return_target_pc = snapshot.call_return_target_pc;
    pipeline.active_call_return_target_owner_seq = snapshot.call_return_target_owner_seq;
    pipeline.active_call_return_target_epoch = snapshot.call_return_target_epoch;
    pipeline.active_call_return_materialization_kind = snapshot.call_return_materialization_kind;
    true
}

pub(crate) fn logical_tag_ready(
    tag: LogicalQueueTag,
    ready_table_t: &BTreeSet<usize>,
    ready_table_u: &BTreeSet<usize>,
) -> bool {
    match tag.kind {
        QueueWakeKind::T => ready_table_t.contains(&tag.tag),
        QueueWakeKind::U => ready_table_u.contains(&tag.tag),
    }
}

pub(crate) fn annotate_qtag_sources(
    seq: usize,
    iq_tags: &BTreeMap<usize, QTag>,
    ready_table_t: &BTreeSet<usize>,
    ready_table_u: &BTreeSet<usize>,
    uops: &mut [CycleUop],
) {
    for idx in 0..uops[seq].src_qtags.len() {
        let Some(kind) = uops[seq].src_queue_kinds[idx] else {
            uops[seq].src_qtags[idx] = None;
            continue;
        };
        let Some(logical_tag) = uops[seq].src_logical_tags[idx] else {
            uops[seq].src_qtags[idx] = None;
            continue;
        };
        if logical_tag_ready(logical_tag, ready_table_t, ready_table_u) {
            uops[seq].src_qtags[idx] = None;
            continue;
        }
        let Some(producer) = uops[seq].deps[idx] else {
            uops[seq].src_qtags[idx] = None;
            continue;
        };
        if uops[producer].dst_queue_kind != Some(kind) {
            uops[seq].src_qtags[idx] = None;
            continue;
        }
        if uops[producer].dst_logical_tag != Some(logical_tag) {
            uops[seq].src_qtags[idx] = None;
            continue;
        }
        uops[seq].src_qtags[idx] = uops[producer]
            .dst_qtag
            .or_else(|| iq_tags.get(&producer).copied());
    }
}

pub(crate) fn make_iq_entry(
    cycle: u64,
    seq: usize,
    phys_iq: PhysIq,
    ready_table_t: &BTreeSet<usize>,
    ready_table_u: &BTreeSet<usize>,
    uops: &[CycleUop],
) -> IqEntry {
    let mut entry = IqEntry {
        seq,
        phys_iq,
        inflight: false,
        src_valid: [false; 2],
        src_ready_nonspec: [false; 2],
        src_ready_spec: [false; 2],
        src_wait_qtag: [false; 2],
    };
    initialize_iq_entry_source_state(cycle, &mut entry, ready_table_t, ready_table_u, uops);
    entry
}

pub(crate) fn update_iq_entries_for_cycle(
    cycle: u64,
    iq: &mut [IqEntry],
    ready_table_t: &BTreeSet<usize>,
    ready_table_u: &BTreeSet<usize>,
    iq_owner_table: &[Vec<Option<usize>>],
    iq_tags: &BTreeMap<usize, QTag>,
    qtag_wait_crossbar: &[Vec<Vec<(usize, usize)>>],
    uops: &[CycleUop],
) {
    for entry in &mut *iq {
        update_iq_entry_source_state(cycle, entry, ready_table_t, ready_table_u, uops);
    }
    let wake_events = collect_iq_wake_events(cycle, uops);
    for event in wake_events {
        publish_iq_wake_event(event, iq, iq_owner_table, iq_tags, qtag_wait_crossbar, uops);
    }
}

fn initialize_iq_entry_source_state(
    cycle: u64,
    entry: &mut IqEntry,
    ready_table_t: &BTreeSet<usize>,
    ready_table_u: &BTreeSet<usize>,
    uops: &[CycleUop],
) {
    for idx in 0..2 {
        entry.src_valid[idx] = source_valid(&uops[entry.seq].commit, idx);
        reset_iq_entry_source_state(entry, idx);
        seed_iq_entry_source_state(cycle, entry, idx, ready_table_t, ready_table_u, uops);
    }
}

fn update_iq_entry_source_state(
    cycle: u64,
    entry: &mut IqEntry,
    ready_table_t: &BTreeSet<usize>,
    ready_table_u: &BTreeSet<usize>,
    uops: &[CycleUop],
) {
    for idx in 0..2 {
        if !entry.src_valid[idx] {
            continue;
        }
        revoke_stale_iq_source_state(cycle, entry, idx, ready_table_t, ready_table_u, uops);
        apply_ready_table_wakeup(entry, idx, ready_table_t, ready_table_u, uops);
    }
}

fn seed_iq_entry_source_state(
    cycle: u64,
    entry: &mut IqEntry,
    idx: usize,
    ready_table_t: &BTreeSet<usize>,
    ready_table_u: &BTreeSet<usize>,
    uops: &[CycleUop],
) {
    let logical_ready = source_logical_ready(entry.seq, idx, ready_table_t, ready_table_u, uops);
    let qtag_wait = source_uses_qtag_wakeup(entry.seq, idx, uops);
    match uops[entry.seq].deps[idx] {
        None => {
            entry.src_ready_nonspec[idx] = true;
        }
        Some(producer) if logical_ready || dep_data_ready_cycle(producer, uops) <= cycle => {
            entry.src_ready_nonspec[idx] = true;
        }
        Some(producer) if dep_pick_ready_cycle(producer, uops) <= cycle => {
            entry.src_ready_spec[idx] = true;
            entry.src_wait_qtag[idx] = false;
        }
        Some(_) => {
            entry.src_wait_qtag[idx] = qtag_wait;
        }
    }
}

fn revoke_stale_iq_source_state(
    cycle: u64,
    entry: &mut IqEntry,
    idx: usize,
    ready_table_t: &BTreeSet<usize>,
    ready_table_u: &BTreeSet<usize>,
    uops: &[CycleUop],
) {
    if !entry.src_valid[idx] {
        return;
    }
    if uops[entry.seq].deps[idx].is_none() {
        entry.src_ready_nonspec[idx] = true;
        entry.src_ready_spec[idx] = false;
        entry.src_wait_qtag[idx] = false;
        return;
    }

    let logical_ready = source_logical_ready(entry.seq, idx, ready_table_t, ready_table_u, uops);
    if entry.src_ready_nonspec[idx]
        && !logical_ready
        && uops[entry.seq].deps[idx]
            .map(|producer| dep_data_ready_cycle(producer, uops) > cycle)
            .unwrap_or(false)
    {
        entry.src_ready_nonspec[idx] = false;
    }

    if entry.src_ready_spec[idx]
        && uops[entry.seq].deps[idx]
            .map(|producer| dep_pick_ready_cycle(producer, uops) > cycle)
            .unwrap_or(true)
    {
        entry.src_ready_spec[idx] = false;
    }

    if !(entry.src_ready_nonspec[idx] || entry.src_ready_spec[idx]) {
        entry.src_wait_qtag[idx] = source_uses_qtag_wakeup(entry.seq, idx, uops);
    }
}

fn apply_ready_table_wakeup(
    entry: &mut IqEntry,
    idx: usize,
    ready_table_t: &BTreeSet<usize>,
    ready_table_u: &BTreeSet<usize>,
    uops: &[CycleUop],
) {
    if !entry.src_valid[idx] || entry.src_ready_nonspec[idx] {
        return;
    }

    if source_logical_ready(entry.seq, idx, ready_table_t, ready_table_u, uops) {
        entry.src_ready_nonspec[idx] = true;
        entry.src_ready_spec[idx] = false;
        entry.src_wait_qtag[idx] = false;
    }
}

fn collect_iq_wake_events(cycle: u64, uops: &[CycleUop]) -> Vec<IqWakeEvent> {
    let mut out = Vec::new();
    for (producer, uop) in uops.iter().enumerate() {
        let queue_kind = uop.dst_queue_kind;
        let logical_tag = uop.dst_logical_tag;
        let qtag = uop.dst_qtag;
        if producer_publishes_nonspec_wakeup(cycle, producer, uops) {
            out.push(IqWakeEvent {
                producer,
                wake_kind: IqWakeKind::Nonspec,
                queue_kind,
                logical_tag,
                qtag,
            });
        } else if producer_publishes_spec_wakeup(cycle, producer, uops) {
            out.push(IqWakeEvent {
                producer,
                wake_kind: IqWakeKind::Spec,
                queue_kind,
                logical_tag,
                qtag,
            });
        }
    }
    out
}

fn publish_iq_wake_event(
    event: IqWakeEvent,
    iq: &mut [IqEntry],
    iq_owner_table: &[Vec<Option<usize>>],
    iq_tags: &BTreeMap<usize, QTag>,
    qtag_wait_table: &[Vec<Vec<(usize, usize)>>],
    uops: &[CycleUop],
) {
    if let (Some(_queue_kind), Some(_logical_tag), Some(qtag)) =
        (event.queue_kind, event.logical_tag, event.qtag)
    {
        for &(seq, src_idx) in &qtag_wait_table[qtag.phys_iq.index()][qtag.entry_id] {
            if let Some(entry) = iq_owner_entry_mut(seq, iq, iq_owner_table, iq_tags) {
                publish_wake_into_entry_source(entry, src_idx, event, uops);
            }
        }
        for entry in iq {
            publish_nonqueue_wake_into_entry(entry, event, uops);
        }
    } else {
        for entry in iq {
            publish_wake_into_entry(entry, event, uops);
        }
    }
}

fn publish_wake_into_entry(entry: &mut IqEntry, event: IqWakeEvent, uops: &[CycleUop]) {
    for idx in 0..2 {
        publish_wake_into_entry_source(entry, idx, event, uops);
    }
}

fn publish_nonqueue_wake_into_entry(entry: &mut IqEntry, event: IqWakeEvent, uops: &[CycleUop]) {
    for idx in 0..2 {
        if uops[entry.seq].src_queue_kinds[idx].is_none() {
            publish_wake_into_entry_source(entry, idx, event, uops);
        }
    }
}

fn publish_wake_into_entry_source(
    entry: &mut IqEntry,
    idx: usize,
    event: IqWakeEvent,
    uops: &[CycleUop],
) {
    if !iq_source_matches_wake_event(entry.seq, idx, event, uops) {
        return;
    }
    match event.wake_kind {
        IqWakeKind::Nonspec => {
            entry.src_ready_nonspec[idx] = true;
            entry.src_ready_spec[idx] = false;
            entry.src_wait_qtag[idx] = false;
        }
        IqWakeKind::Spec => {
            if !entry.src_ready_nonspec[idx] {
                entry.src_ready_spec[idx] = true;
                entry.src_wait_qtag[idx] = false;
            }
        }
    }
}

fn iq_source_matches_wake_event(
    seq: usize,
    idx: usize,
    event: IqWakeEvent,
    uops: &[CycleUop],
) -> bool {
    if !source_valid(&uops[seq].commit, idx) || uops[seq].deps[idx] != Some(event.producer) {
        return false;
    }

    match uops[seq].src_queue_kinds[idx] {
        Some(queue_kind) => {
            Some(queue_kind) == event.queue_kind
                && uops[seq].src_logical_tags[idx] == event.logical_tag
                && uops[seq].src_qtags[idx].is_some()
                && uops[seq].src_qtags[idx] == event.qtag
        }
        None => true,
    }
}

fn producer_publishes_spec_wakeup(cycle: u64, producer: usize, uops: &[CycleUop]) -> bool {
    let uop = &uops[producer];
    if !uop.is_load {
        return false;
    }

    if uop.e1_cycle == Some(cycle.saturating_sub(1)) {
        return true;
    }

    dep_pick_ready_cycle(producer, uops) == cycle
}

fn producer_publishes_nonspec_wakeup(cycle: u64, producer: usize, uops: &[CycleUop]) -> bool {
    let uop = &uops[producer];
    if uop.is_load {
        if uop.e4_cycle == Some(cycle.saturating_sub(1)) {
            return true;
        }
    } else if uop.w1_cycle == Some(cycle.saturating_sub(1)) {
        return true;
    }

    dep_data_ready_cycle(producer, uops) == cycle
}

fn source_logical_ready(
    seq: usize,
    idx: usize,
    ready_table_t: &BTreeSet<usize>,
    ready_table_u: &BTreeSet<usize>,
    uops: &[CycleUop],
) -> bool {
    uops[seq].src_logical_tags[idx]
        .map(|tag| logical_tag_ready(tag, ready_table_t, ready_table_u))
        .unwrap_or(false)
}

fn reset_iq_entry_source_state(entry: &mut IqEntry, idx: usize) {
    entry.src_ready_nonspec[idx] = false;
    entry.src_ready_spec[idx] = false;
    entry.src_wait_qtag[idx] = false;
}

pub(crate) fn rebuild_iq_owner_table(
    iq_owner_table: &mut [Vec<Option<usize>>],
    iq: &[IqEntry],
    iq_tags: &BTreeMap<usize, QTag>,
) {
    for phys_iq in iq_owner_table.iter_mut() {
        for owner in phys_iq.iter_mut() {
            *owner = None;
        }
    }
    for (idx, entry) in iq.iter().enumerate() {
        let Some(qtag) = iq_tags.get(&entry.seq).copied() else {
            continue;
        };
        iq_owner_table[qtag.phys_iq.index()][qtag.entry_id] = Some(idx);
    }
}

fn iq_owner_entry_mut<'a>(
    seq: usize,
    iq: &'a mut [IqEntry],
    iq_owner_table: &[Vec<Option<usize>>],
    iq_tags: &BTreeMap<usize, QTag>,
) -> Option<&'a mut IqEntry> {
    let qtag = iq_tags.get(&seq).copied()?;
    let idx = iq_owner_table[qtag.phys_iq.index()][qtag.entry_id]?;
    (iq.get(idx).map(|entry| entry.seq == seq).unwrap_or(false)).then(|| &mut iq[idx])
}

pub(crate) fn register_iq_wait_crossbar_entry(
    qtag_wait_crossbar: &mut [Vec<Vec<(usize, usize)>>],
    seq: usize,
    uop: &CycleUop,
) {
    for (src_idx, qtag) in uop.src_qtags.iter().copied().enumerate() {
        let Some(qtag) = qtag else {
            continue;
        };
        let waiters = &mut qtag_wait_crossbar[qtag.phys_iq.index()][qtag.entry_id];
        if !waiters.contains(&(seq, src_idx)) {
            waiters.push((seq, src_idx));
        }
    }
}

pub(crate) fn unregister_iq_wait_crossbar_seq(
    qtag_wait_crossbar: &mut [Vec<Vec<(usize, usize)>>],
    seq: usize,
) {
    for phys_iq in qtag_wait_crossbar.iter_mut() {
        for waiters in phys_iq.iter_mut() {
            waiters.retain(|(entry_seq, _)| *entry_seq != seq);
        }
    }
}

pub(crate) fn prune_iq_wait_crossbar_on_redirect(
    qtag_wait_crossbar: &mut [Vec<Vec<(usize, usize)>>],
    flush_seq: usize,
) {
    for phys_iq in qtag_wait_crossbar.iter_mut() {
        for waiters in phys_iq.iter_mut() {
            waiters.retain(|(entry_seq, _)| *entry_seq <= flush_seq);
        }
    }
}

pub(crate) fn allocate_qtag(iq_tags: &BTreeMap<usize, QTag>, phys_iq: PhysIq) -> Option<QTag> {
    (0..phys_iq.capacity())
        .find(|&entry_id| {
            !iq_tags
                .values()
                .any(|tag| tag.phys_iq == phys_iq && tag.entry_id == entry_id)
        })
        .map(|entry_id| QTag { phys_iq, entry_id })
}
