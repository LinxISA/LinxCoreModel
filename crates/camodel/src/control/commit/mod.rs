pub mod cmt;
pub mod rob;

use std::collections::VecDeque;

use isa::{CommitRecord, StageTraceEvent};
use runtime::GuestRuntime;

use crate::{
    COMMIT_WIDTH, CycleUop, ScbEntry, StageQueues, branch_kind_label,
    call_materialization_kind_label, dynamic_target_source_kind_label, insert_ready_table_tag,
    live_boundary_epoch_for_seq, live_branch_kind_for_seq, live_call_materialization_kind_for_seq,
    live_control_target_owner_row_id_for_seq, live_dynamic_target_producer_kind_for_seq,
    live_dynamic_target_setup_epoch_for_seq, live_dynamic_target_source_epoch_for_seq,
    live_dynamic_target_source_kind_for_seq, live_dynamic_target_source_owner_row_id_for_seq,
    live_return_consumer_kind_for_seq, live_rob_checkpoint_id_for_seq, remove_queue_entry,
    return_consumer_kind_label, stage_event_with_meta,
};

pub(crate) fn retire_ready(
    cycle: u64,
    runtime: &GuestRuntime,
    rob: &mut VecDeque<usize>,
    committed: &mut Vec<CommitRecord>,
    retired_seqs: &mut Vec<usize>,
    pipeline: &mut StageQueues,
    uops: &mut [CycleUop],
    stage_events: &mut Vec<StageTraceEvent>,
) -> Option<u64> {
    let mut retired_this_cycle = 0usize;
    let mut trap_retired = None;
    while retired_this_cycle < COMMIT_WIDTH {
        let Some(&seq) = rob.front() else {
            break;
        };
        if uops[seq].done_cycle.is_none() {
            break;
        }
        let mut commit = uops[seq].commit.clone();
        if let Some(pending_trap) = pipeline.pending_trap.filter(|pending| pending.seq == seq) {
            commit.trap_valid = 1;
            commit.trap_cause = pending_trap.cause;
            commit.traparg0 = pending_trap.traparg0;
            trap_retired = Some(pending_trap.cause);
            pipeline.pending_trap = None;
        }
        let trap_cause = (commit.trap_valid != 0).then_some(commit.trap_cause);
        let traparg0 = (commit.trap_valid != 0).then_some(commit.traparg0);
        commit.cycle = cycle;
        committed.push(commit);
        retired_seqs.push(seq);
        stage_events.push(stage_event_with_meta(
            cycle,
            runtime,
            uops,
            seq,
            "CMT",
            "retire",
            Some(live_rob_checkpoint_id_for_seq(seq, pipeline, uops)),
            trap_cause,
            traparg0,
            live_dynamic_target_setup_epoch_for_seq(seq, pipeline, uops),
            live_boundary_epoch_for_seq(seq, pipeline, uops),
            live_dynamic_target_source_owner_row_id_for_seq(seq, pipeline, uops).as_deref(),
            live_dynamic_target_source_epoch_for_seq(seq, pipeline, uops),
            live_control_target_owner_row_id_for_seq(seq, pipeline, uops).as_deref(),
            live_dynamic_target_producer_kind_for_seq(seq, pipeline, uops)
                .map(return_consumer_kind_label),
            live_branch_kind_for_seq(seq, pipeline, uops).and_then(branch_kind_label),
            live_return_consumer_kind_for_seq(seq, pipeline, uops).map(return_consumer_kind_label),
            live_call_materialization_kind_for_seq(seq, pipeline, uops)
                .map(call_materialization_kind_label),
            live_dynamic_target_source_kind_for_seq(seq, pipeline, uops)
                .map(dynamic_target_source_kind_label),
        ));
        rob.pop_front();
        if let Some(tag) = uops[seq].dst_logical_tag {
            insert_ready_table_tag(pipeline, tag);
        }
        if uops[seq].is_store {
            remove_queue_entry(&mut pipeline.stq, seq);
            pipeline.scb.push_back(ScbEntry {
                seq,
                enqueue_cycle: cycle,
            });
        }
        retired_this_cycle += 1;
    }
    trap_retired
}

pub(crate) fn rob_age_rank(seq: usize, rob: &VecDeque<usize>) -> usize {
    rob.iter()
        .position(|&rob_seq| rob_seq == seq)
        .unwrap_or(usize::MAX)
}
