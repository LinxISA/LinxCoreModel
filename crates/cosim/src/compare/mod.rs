use anyhow::{Context, Result, bail};
use isa::CommitRecord;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum M1Message {
    #[serde(rename = "start")]
    Start {
        boot_pc: u64,
        trigger_pc: u64,
        terminate_pc: u64,
        snapshot_path: String,
        seq_base: u64,
    },
    #[serde(rename = "commit")]
    Commit {
        seq: u64,
        #[serde(flatten)]
        commit: CommitRecord,
    },
    #[serde(rename = "end")]
    End { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CosimMismatch {
    pub index: usize,
    pub field: String,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CosimReport {
    pub matched_commits: usize,
    pub mismatch: Option<CosimMismatch>,
}

pub fn load_commit_jsonl(path: impl AsRef<Path>) -> Result<Vec<CommitRecord>> {
    let text = fs::read_to_string(path.as_ref())
        .with_context(|| format!("failed to read {}", path.as_ref().display()))?;
    let mut out = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let rec: CommitRecord = serde_json::from_str(trimmed)
            .with_context(|| format!("invalid commit JSON at line {}", lineno + 1))?;
        out.push(rec);
    }
    Ok(out)
}

pub fn compare_commit_streams(expected: &[CommitRecord], actual: &[CommitRecord]) -> CosimReport {
    let count = expected.len().min(actual.len());
    for idx in 0..count {
        let lhs = &expected[idx];
        let rhs = &actual[idx];
        if lhs.pc != rhs.pc {
            return mismatch(idx, "pc", lhs.pc, rhs.pc);
        }
        if lhs.insn != rhs.insn {
            return mismatch(idx, "insn", lhs.insn, rhs.insn);
        }
        if lhs.len != rhs.len {
            return mismatch(idx, "len", lhs.len, rhs.len);
        }
        if lhs.wb_valid != rhs.wb_valid {
            return mismatch(idx, "wb_valid", lhs.wb_valid, rhs.wb_valid);
        }
        if lhs.mem_valid != rhs.mem_valid {
            return mismatch(idx, "mem_valid", lhs.mem_valid, rhs.mem_valid);
        }
        if lhs.trap_valid != rhs.trap_valid {
            return mismatch(idx, "trap_valid", lhs.trap_valid, rhs.trap_valid);
        }
        if lhs.next_pc != rhs.next_pc {
            return mismatch(idx, "next_pc", lhs.next_pc, rhs.next_pc);
        }
    }

    if expected.len() != actual.len() {
        return CosimReport {
            matched_commits: count,
            mismatch: Some(CosimMismatch {
                index: count,
                field: "commit_count".to_string(),
                expected: expected.len().to_string(),
                actual: actual.len().to_string(),
            }),
        };
    }

    CosimReport {
        matched_commits: count,
        mismatch: None,
    }
}

pub fn require_cosim_match(report: &CosimReport) -> Result<()> {
    if let Some(mismatch) = &report.mismatch {
        bail!(
            "cosim mismatch at commit {} field {}: expected={} actual={}",
            mismatch.index,
            mismatch.field,
            mismatch.expected,
            mismatch.actual
        );
    }
    Ok(())
}

fn mismatch<T: ToString, U: ToString>(
    index: usize,
    field: &str,
    expected: T,
    actual: U,
) -> CosimReport {
    CosimReport {
        matched_commits: index,
        mismatch: Some(CosimMismatch {
            index,
            field: field.to_string(),
            expected: expected.to_string(),
            actual: actual.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use isa::{BlockMeta, CommitRecord};

    #[test]
    fn comparison_finds_pc_mismatch() {
        let lhs = CommitRecord::unsupported(0, 1, 2, 4, &BlockMeta::default());
        let rhs = CommitRecord::unsupported(0, 3, 2, 4, &BlockMeta::default());
        let report = compare_commit_streams(&[lhs], &[rhs]);
        assert_eq!(report.mismatch.unwrap().field, "pc");
    }
}
