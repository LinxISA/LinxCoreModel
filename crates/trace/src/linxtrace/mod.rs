use anyhow::{Context, Result};
use isa::{
    LINXTRACE_FORMAT, LaneCatalogEntry, RowCatalogEntry, RunResult, StageCatalogEntry,
    StageTraceEvent, default_stage_catalog,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum TraceRecord<'a> {
    META {
        format: &'static str,
        contract_id: &'static str,
        pipeline_schema_id: &'static str,
        stage_order_csv: String,
        stage_catalog: Vec<StageCatalogEntry>,
        lane_catalog: Vec<LaneCatalogEntry>,
        row_catalog: Vec<RowCatalogEntry>,
        render_prefs: serde_json::Value,
    },
    #[serde(rename = "OP_DEF")]
    OpDef {
        row_id: &'a str,
        row_kind: &'a str,
        block_uid: u64,
        uop_uid: u64,
    },
    OCC {
        cycle: u64,
        row_id: &'a str,
        stage_id: &'a str,
        lane_id: &'a str,
        stall: bool,
        cause: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        checkpoint_id: Option<u8>,
        #[serde(skip_serializing_if = "Option::is_none")]
        trap_cause: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        traparg0: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_setup_epoch: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        boundary_epoch: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_source_owner_row_id: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_source_epoch: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_owner_row_id: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_producer_kind: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        branch_kind: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        return_kind: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_materialization_kind: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_source_kind: Option<&'a str>,
    },
    RETIRE {
        cycle: u64,
        row_id: &'a str,
        status: &'a str,
    },
    #[serde(rename = "BLOCK_EVT")]
    BlockEvt {
        cycle: u64,
        row_id: &'a str,
        kind: &'a str,
        detail: &'a str,
    },
}

pub fn write_commit_jsonl(path: impl AsRef<Path>, result: &RunResult) -> Result<()> {
    let mut body = String::new();
    for rec in &result.commits {
        body.push_str(&serde_json::to_string(rec)?);
        body.push('\n');
    }
    fs::write(path.as_ref(), body)
        .with_context(|| format!("failed to write commit trace {}", path.as_ref().display()))
}

pub fn write_linxtrace(
    path: impl AsRef<Path>,
    result: &RunResult,
    stage_events: &[StageTraceEvent],
) -> Result<()> {
    let path = path.as_ref();
    let stage_catalog = default_stage_catalog();
    let stage_order_csv = stage_catalog
        .iter()
        .map(|entry| entry.stage_id.clone())
        .collect::<Vec<_>>()
        .join(",");
    let row_ids = collect_row_ids(stage_events, result.commits.len());
    let row_catalog = row_ids
        .iter()
        .enumerate()
        .map(|(idx, row_id)| RowCatalogEntry {
            row_id: row_id.clone(),
            row_kind: "uop".to_string(),
            core_id: "core0".to_string(),
            block_uid: 0,
            uop_uid: idx as u64,
            left_label: row_label(result, idx),
            detail_defaults: result.metrics.exit_reason.clone(),
        })
        .collect::<Vec<_>>();
    let lane_catalog = collect_lane_catalog(stage_events);

    let mut lines = Vec::new();
    lines.push(serde_json::to_string(&TraceRecord::META {
        format: LINXTRACE_FORMAT,
        contract_id: "LXMODEL-TRACE-BOOTSTRAP",
        pipeline_schema_id: "LC-TRACE1-LXMODEL",
        stage_order_csv,
        stage_catalog,
        lane_catalog,
        row_catalog,
        render_prefs: serde_json::json!({"focus":"bootstrap"}),
    })?);
    for (idx, row_id) in row_ids.iter().enumerate() {
        lines.push(serde_json::to_string(&TraceRecord::OpDef {
            row_id,
            row_kind: "uop",
            block_uid: 0,
            uop_uid: idx as u64,
        })?);
    }

    if stage_events.is_empty() {
        lines.push(serde_json::to_string(&TraceRecord::OCC {
            cycle: 0,
            row_id: "uop0",
            stage_id: "F0",
            lane_id: "scalar0",
            stall: false,
            cause: "bootstrap_fetch",
            checkpoint_id: None,
            trap_cause: None,
            traparg0: None,
            target_setup_epoch: None,
            boundary_epoch: None,
            target_source_owner_row_id: None,
            target_source_epoch: None,
            target_owner_row_id: None,
            target_producer_kind: None,
            branch_kind: None,
            return_kind: None,
            call_materialization_kind: None,
            target_source_kind: None,
        })?);
        let retire_cycle = result.commits.first().map(|rec| rec.cycle).unwrap_or(0);
        lines.push(serde_json::to_string(&TraceRecord::OCC {
            cycle: retire_cycle,
            row_id: "uop0",
            stage_id: "CMT",
            lane_id: "scalar0",
            stall: false,
            cause: "trap_commit",
            checkpoint_id: None,
            trap_cause: None,
            traparg0: None,
            target_setup_epoch: None,
            boundary_epoch: None,
            target_source_owner_row_id: None,
            target_source_epoch: None,
            target_owner_row_id: None,
            target_producer_kind: None,
            branch_kind: None,
            return_kind: None,
            call_materialization_kind: None,
            target_source_kind: None,
        })?);
    } else {
        for event in stage_events {
            lines.push(serde_json::to_string(&TraceRecord::OCC {
                cycle: event.cycle,
                row_id: &event.row_id,
                stage_id: &event.stage_id,
                lane_id: &event.lane_id,
                stall: event.stall,
                cause: &event.cause,
                checkpoint_id: event.checkpoint_id,
                trap_cause: event.trap_cause,
                traparg0: event.traparg0,
                target_setup_epoch: event.target_setup_epoch,
                boundary_epoch: event.boundary_epoch,
                target_source_owner_row_id: event.target_source_owner_row_id.as_deref(),
                target_source_epoch: event.target_source_epoch,
                target_owner_row_id: event.target_owner_row_id.as_deref(),
                target_producer_kind: event.target_producer_kind.as_deref(),
                branch_kind: event.branch_kind.as_deref(),
                return_kind: event.return_kind.as_deref(),
                call_materialization_kind: event.call_materialization_kind.as_deref(),
                target_source_kind: event.target_source_kind.as_deref(),
            })?);
        }
    }

    if result.commits.is_empty() {
        lines.push(serde_json::to_string(&TraceRecord::RETIRE {
            cycle: 0,
            row_id: row_ids.first().map(String::as_str).unwrap_or("uop0"),
            status: &result.metrics.exit_reason,
        })?);
        lines.push(serde_json::to_string(&TraceRecord::BlockEvt {
            cycle: 0,
            row_id: row_ids.first().map(String::as_str).unwrap_or("uop0"),
            kind: "fault",
            detail: &result.metrics.exit_reason,
        })?);
    } else {
        for (idx, commit) in result.commits.iter().enumerate() {
            let status = if idx + 1 == result.commits.len() {
                result.metrics.exit_reason.as_str()
            } else {
                "retired"
            };
            let row_id = row_ids
                .get(idx)
                .map(String::as_str)
                .unwrap_or_else(|| row_ids.last().map(String::as_str).unwrap_or("uop0"));
            lines.push(serde_json::to_string(&TraceRecord::RETIRE {
                cycle: commit.cycle,
                row_id,
                status,
            })?);
        }

        let last_commit = result.commits.last().expect("checked non-empty commits");
        let last_row = row_ids.last().map(String::as_str).unwrap_or("uop0");
        lines.push(serde_json::to_string(&TraceRecord::BlockEvt {
            cycle: last_commit.cycle,
            row_id: last_row,
            kind: "fault",
            detail: &result.metrics.exit_reason,
        })?);
    }

    fs::write(path, lines.join("\n") + "\n")
        .with_context(|| format!("failed to write {}", path.display()))
}

fn collect_row_ids(stage_events: &[StageTraceEvent], commit_count: usize) -> Vec<String> {
    let mut row_ids = Vec::new();
    let mut seen = BTreeSet::new();
    for event in stage_events {
        if seen.insert(event.row_id.clone()) {
            row_ids.push(event.row_id.clone());
        }
    }
    let target_rows = commit_count.max(1);
    for idx in row_ids.len()..target_rows {
        row_ids.push(format!("uop{idx}"));
    }
    row_ids
}

fn collect_lane_catalog(stage_events: &[StageTraceEvent]) -> Vec<LaneCatalogEntry> {
    let mut lane_catalog = Vec::new();
    let mut seen = BTreeSet::new();
    for event in stage_events {
        if seen.insert(event.lane_id.clone()) {
            lane_catalog.push(LaneCatalogEntry {
                lane_id: event.lane_id.clone(),
                label: event.lane_id.clone(),
            });
        }
    }
    if lane_catalog.is_empty() {
        lane_catalog.push(LaneCatalogEntry {
            lane_id: "scalar0".to_string(),
            label: "scalar0".to_string(),
        });
    }
    lane_catalog
}

fn row_label(result: &RunResult, idx: usize) -> String {
    if let Some(commit) = result.commits.get(idx) {
        format!("{}@0x{:x}", result.image_name, commit.pc)
    } else {
        format!("{}@0x{:x}", result.image_name, result.entry_pc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use isa::{CommitRecord, EngineKind, RunMetrics};

    #[test]
    fn linxtrace_meta_is_first_record() {
        let result = RunResult {
            image_name: "sample.elf".to_string(),
            entry_pc: 0x1000,
            metrics: RunMetrics {
                engine: EngineKind::Func,
                cycles: 1,
                commits: 1,
                exit_reason: "bootstrap".to_string(),
            },
            commits: vec![CommitRecord::unsupported(
                0,
                0x1000,
                0,
                4,
                &isa::BlockMeta::default(),
            )],
            decoded: Vec::new(),
        };
        let tmp = tempfile::NamedTempFile::new().unwrap();
        write_linxtrace(tmp.path(), &result, &[]).unwrap();
        let text = fs::read_to_string(tmp.path()).unwrap();
        assert!(text.lines().next().unwrap().contains("\"type\":\"META\""));
    }

    #[test]
    fn linxtrace_emits_multi_row_defs_and_retires() {
        let result = RunResult {
            image_name: "sample.elf".to_string(),
            entry_pc: 0x1000,
            metrics: RunMetrics {
                engine: EngineKind::Cycle,
                cycles: 4,
                commits: 2,
                exit_reason: "guest_exit(0)".to_string(),
            },
            commits: vec![
                CommitRecord::unsupported(2, 0x1000, 0x15, 0, &isa::BlockMeta::default()),
                CommitRecord::unsupported(3, 0x1004, 0x302b, 0, &isa::BlockMeta::default()),
            ],
            decoded: Vec::new(),
        };
        let stage_events = vec![
            StageTraceEvent {
                cycle: 0,
                row_id: "uop0".to_string(),
                stage_id: "F0".to_string(),
                lane_id: "scalar0".to_string(),
                stall: false,
                cause: "resident".to_string(),
                checkpoint_id: Some(7),
                trap_cause: Some(0x0000_B001),
                traparg0: Some(0x1004),
                target_setup_epoch: Some(5),
                boundary_epoch: Some(7),
                target_source_owner_row_id: Some("uop3".to_string()),
                target_source_epoch: Some(4),
                target_owner_row_id: Some("uop7".to_string()),
                target_producer_kind: Some("setc_tgt".to_string()),
                branch_kind: Some("cond".to_string()),
                return_kind: Some("fret_stk".to_string()),
                call_materialization_kind: Some("adjacent_setret".to_string()),
                target_source_kind: Some("call_return_adjacent_setret".to_string()),
            },
            StageTraceEvent {
                cycle: 1,
                row_id: "uop1".to_string(),
                stage_id: "F0".to_string(),
                lane_id: "scalar0".to_string(),
                stall: false,
                cause: "resident".to_string(),
                checkpoint_id: None,
                trap_cause: None,
                traparg0: None,
                target_setup_epoch: None,
                boundary_epoch: None,
                target_source_owner_row_id: None,
                target_source_epoch: None,
                target_owner_row_id: None,
                target_producer_kind: None,
                branch_kind: None,
                return_kind: None,
                call_materialization_kind: None,
                target_source_kind: None,
            },
        ];

        let tmp = tempfile::NamedTempFile::new().unwrap();
        write_linxtrace(tmp.path(), &result, &stage_events).unwrap();
        let text = fs::read_to_string(tmp.path()).unwrap();

        assert!(text.contains("\"row_id\":\"uop0\""));
        assert!(text.contains("\"row_id\":\"uop1\""));
        assert_eq!(text.matches("\"type\":\"OP_DEF\"").count(), 2);
        assert_eq!(text.matches("\"type\":\"RETIRE\"").count(), 2);
        assert!(text.contains("\"status\":\"retired\""));
        assert!(text.contains("\"status\":\"guest_exit(0)\""));
        assert!(text.contains("\"checkpoint_id\":7"));
        assert!(text.contains("\"trap_cause\":45057"));
        assert!(text.contains("\"traparg0\":4100"));
        assert!(text.contains("\"target_setup_epoch\":5"));
        assert!(text.contains("\"boundary_epoch\":7"));
        assert!(text.contains("\"target_source_owner_row_id\":\"uop3\""));
        assert!(text.contains("\"target_source_epoch\":4"));
        assert!(text.contains("\"target_owner_row_id\":\"uop7\""));
        assert!(text.contains("\"target_producer_kind\":\"setc_tgt\""));
        assert!(text.contains("\"branch_kind\":\"cond\""));
        assert!(text.contains("\"return_kind\":\"fret_stk\""));
        assert!(text.contains("\"call_materialization_kind\":\"adjacent_setret\""));
        assert!(text.contains("\"target_source_kind\":\"call_return_adjacent_setret\""));
    }
}
