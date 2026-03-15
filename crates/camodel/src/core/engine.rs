use anyhow::Result;
use funcmodel::{FuncEngine, FuncRunOptions};
use isa::{CommitRecord, EngineKind, RunMetrics, RunResult};
use runtime::GuestRuntime;
use std::collections::VecDeque;

use crate::*;

impl CycleEngine {
    pub fn run(&self, runtime: &GuestRuntime, options: &CycleRunOptions) -> Result<CycleRunBundle> {
        let func_limit = options
            .max_cycles
            .saturating_mul(COMMIT_WIDTH as u64)
            .max(1);
        let func_bundle = FuncEngine.run(
            runtime,
            &FuncRunOptions {
                max_steps: func_limit,
            },
        )?;

        let mut uops = build_uops(&func_bundle.result.commits, &func_bundle.result.decoded);
        if uops.is_empty() {
            return Ok(CycleRunBundle {
                result: RunResult {
                    image_name: runtime.image.image_name(),
                    entry_pc: runtime.state.pc,
                    metrics: RunMetrics {
                        engine: EngineKind::Cycle,
                        cycles: 0,
                        commits: 0,
                        exit_reason: func_bundle.result.metrics.exit_reason,
                    },
                    commits: Vec::new(),
                    decoded: Vec::new(),
                },
                stage_events: Vec::new(),
            });
        }

        let mut stage_events = Vec::new();
        let mut pipeline = StageQueues::default();
        let mut iq = Vec::<IqEntry>::new();
        let mut rob = VecDeque::<usize>::new();
        let mut committed = Vec::<CommitRecord>::new();
        let mut retired_seqs = Vec::<usize>::new();
        let mut next_fetch_seq = 0usize;
        let mut exit_reason = "cycle_limit".to_string();
        let target_commit_count = func_bundle.result.commits.len();

        for cycle in 0..options.max_cycles {
            apply_pending_flush(cycle, &mut pipeline, &mut iq, &mut rob, &uops);
            fill_fetch(cycle, &mut pipeline, &mut next_fetch_seq, &uops);

            tag_stage_cycles(cycle, &pipeline, &mut uops);
            publish_bru_correction_state(cycle, &mut pipeline, &uops);
            publish_dynamic_boundary_target_fault_state(cycle, &mut pipeline, &uops);
            publish_call_header_fault_state(cycle, &mut pipeline, &uops);
            update_iq_entries_for_cycle(
                cycle,
                &mut iq,
                &pipeline.ready_table_t,
                &pipeline.ready_table_u,
                &pipeline.iq_owner_table,
                &pipeline.iq_tags,
                &pipeline.qtag_wait_crossbar,
                &uops,
            );
            emit_stage_events(
                cycle,
                runtime,
                &pipeline,
                &iq,
                &rob,
                &uops,
                &mut stage_events,
            );
            advance_scb(cycle, &mut pipeline, &uops);

            let trap_retired = retire_ready(
                cycle,
                runtime,
                &mut rob,
                &mut committed,
                &mut retired_seqs,
                &mut pipeline,
                &mut uops,
                &mut stage_events,
            );
            if let Some(cause) = trap_retired {
                exit_reason = format!("trap(0x{cause:08x})");
                break;
            }
            schedule_frontend_redirect_recovery(cycle, &mut pipeline, &uops);

            if committed.len() == target_commit_count {
                exit_reason = func_bundle.result.metrics.exit_reason.clone();
                break;
            }

            advance_execute(cycle, &mut pipeline, &mut uops, options);
            advance_l1d(cycle, &mut pipeline);
            advance_liq(cycle, &mut pipeline, &mut uops, &rob);
            advance_i1_to_i2(&mut pipeline, &mut iq);
            let mut admitted_i1 = arbitrate_i1(cycle, &mut pipeline.p1, &mut iq, &uops, &rob);
            advance_p1_to_i1(&mut pipeline.i1, &mut admitted_i1, &mut pipeline.p1);
            advance_i2(
                cycle,
                &mut pipeline.i2,
                &mut pipeline.e1,
                &mut pipeline.lhq,
                &mut pipeline.stq,
                &mut pipeline.lsid_issue_ptr,
                &mut pipeline.lsid_complete_ptr,
                &uops,
            );
            pick_from_iq(
                cycle,
                pipeline.lsid_issue_ptr,
                &mut iq,
                &uops,
                &mut pipeline.p1,
                &rob,
            );
            dispatch_to_iq_and_bypass(cycle, &mut pipeline, &mut iq, &mut rob, &mut uops);
            advance_frontend(&mut pipeline, &mut rob);

            if committed.len() == target_commit_count {
                exit_reason = func_bundle.result.metrics.exit_reason.clone();
                break;
            }
        }

        if committed.len() == target_commit_count {
            exit_reason = func_bundle.result.metrics.exit_reason;
        }

        let decoded = retired_seqs
            .into_iter()
            .filter_map(|seq| uops.get(seq).map(|uop| uop.decoded.clone()))
            .collect::<Vec<_>>();

        let result = RunResult {
            image_name: runtime.image.image_name(),
            entry_pc: runtime.state.pc,
            metrics: RunMetrics {
                engine: EngineKind::Cycle,
                cycles: if exit_reason == "cycle_limit" {
                    options.max_cycles
                } else {
                    committed
                        .last()
                        .map(|commit| commit.cycle.saturating_add(1))
                        .unwrap_or(options.max_cycles)
                },
                commits: committed.len() as u64,
                exit_reason,
            },
            commits: committed,
            decoded,
        };
        Ok(CycleRunBundle {
            result,
            stage_events,
        })
    }
}
