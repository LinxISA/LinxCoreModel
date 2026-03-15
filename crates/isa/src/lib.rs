use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

pub const TRACE_SCHEMA_VERSION: &str = "1.0";
pub const LINXTRACE_FORMAT: &str = "linxtrace.v1";
pub const TRAP_ILLEGAL_INST: u64 = 4;
pub const TRAP_BRU_RECOVERY_NOT_BSTART: u64 = 0x0000_B001;
pub const TRAP_DYNAMIC_TARGET_MISSING: u64 = 0x0000_B002;
pub const TRAP_DYNAMIC_TARGET_NOT_BSTART: u64 = 0x0000_B003;
pub const TRAP_SETRET_NOT_ADJACENT: u64 = 0x0000_B004;
pub const TRAP_DYNAMIC_TARGET_STALE: u64 = 0x0000_B005;
pub const TRAP_UNSUPPORTED_PRIVILEGE: u64 = 0x0000_B010;
pub const DEFAULT_MEM_BYTES: u64 = 128 * 1024 * 1024;
pub const DEFAULT_STACK_SIZE: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Func,
    Cycle,
}

impl core::str::FromStr for EngineKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "func" => Ok(Self::Func),
            "cycle" => Ok(Self::Cycle),
            other => Err(format!("unknown engine {other}; expected func|cycle")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockMeta {
    pub bid: u64,
    pub block_uid: u64,
    pub block_kind: String,
    pub lane_id: String,
}

impl Default for BlockMeta {
    fn default() -> Self {
        Self {
            bid: 0,
            block_uid: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchitecturalState {
    pub pc: u64,
    pub regs: [u64; 32],
    pub next_bid: u64,
}

impl ArchitecturalState {
    pub fn new(pc: u64) -> Self {
        Self {
            pc,
            regs: [0; 32],
            next_bid: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitRecord {
    pub schema_version: String,
    pub cycle: u64,
    pub pc: u64,
    pub insn: u64,
    pub len: u8,
    pub next_pc: u64,
    pub src0_valid: u8,
    pub src0_reg: u8,
    pub src0_data: u64,
    pub src1_valid: u8,
    pub src1_reg: u8,
    pub src1_data: u64,
    pub dst_valid: u8,
    pub dst_reg: u8,
    pub dst_data: u64,
    pub wb_valid: u8,
    pub wb_rd: u8,
    pub wb_data: u64,
    pub mem_valid: u8,
    pub mem_is_store: u8,
    pub mem_addr: u64,
    pub mem_wdata: u64,
    pub mem_rdata: u64,
    pub mem_size: u8,
    pub trap_valid: u8,
    pub trap_cause: u64,
    pub traparg0: u64,
    pub block_kind: String,
    pub lane_id: String,
    pub tile_meta: String,
    pub tile_ref_src: u64,
    pub tile_ref_dst: u64,
}

impl CommitRecord {
    pub fn unsupported(cycle: u64, pc: u64, insn: u64, cause: u64, block: &BlockMeta) -> Self {
        Self {
            schema_version: TRACE_SCHEMA_VERSION.to_string(),
            cycle,
            pc,
            insn,
            len: 4,
            next_pc: pc,
            src0_valid: 0,
            src0_reg: 0,
            src0_data: 0,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 0,
            dst_reg: 0,
            dst_data: 0,
            wb_valid: 0,
            wb_rd: 0,
            wb_data: 0,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 1,
            trap_cause: cause,
            traparg0: insn,
            block_kind: block.block_kind.clone(),
            lane_id: block.lane_id.clone(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMetrics {
    pub engine: EngineKind,
    pub cycles: u64,
    pub commits: u64,
    pub exit_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunResult {
    pub image_name: String,
    pub entry_pc: u64,
    pub metrics: RunMetrics,
    pub commits: Vec<CommitRecord>,
    pub decoded: Vec<DecodedInstruction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceCaptureOptions {
    pub commit_window_start: u64,
    pub commit_window_end: Option<u64>,
    pub stage_filter: Vec<String>,
    pub row_filter: Vec<String>,
}

impl Default for TraceCaptureOptions {
    fn default() -> Self {
        Self {
            commit_window_start: 0,
            commit_window_end: None,
            stage_filter: Vec::new(),
            row_filter: Vec::new(),
        }
    }
}

pub fn default_stage_order() -> Vec<&'static str> {
    vec![
        "F0", "F1", "F2", "F3", "IB", "F4", "D1", "D2", "D3", "S1", "S2", "IQ", "P1", "I1", "I2",
        "E1", "E2", "E3", "E4", "W1", "W2", "ROB", "CMT", "FLS",
    ]
}

pub fn default_stage_catalog() -> Vec<StageCatalogEntry> {
    default_stage_order()
        .into_iter()
        .enumerate()
        .map(|(idx, stage)| StageCatalogEntry {
            stage_id: stage.to_string(),
            label: stage.to_string(),
            color: format!("#{:06x}", 0x335577 + idx as u32 * 0x050103),
            group: if stage.starts_with('F') || stage == "IB" {
                "frontend".to_string()
            } else if stage.starts_with('D') || stage.starts_with('S') || stage == "IQ" {
                "dispatch".to_string()
            } else if stage.starts_with('E')
                || stage.starts_with('W')
                || stage.starts_with('P')
                || stage.starts_with('I')
            {
                "execute".to_string()
            } else {
                "retire".to_string()
            },
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageCatalogEntry {
    pub stage_id: String,
    pub label: String,
    pub color: String,
    pub group: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneCatalogEntry {
    pub lane_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowCatalogEntry {
    pub row_id: String,
    pub row_kind: String,
    pub core_id: String,
    pub block_uid: u64,
    pub uop_uid: u64,
    pub left_label: String,
    pub detail_defaults: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageTraceEvent {
    pub cycle: u64,
    pub row_id: String,
    pub stage_id: String,
    pub lane_id: String,
    pub stall: bool,
    pub cause: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trap_cause: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traparg0: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_setup_epoch: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary_epoch: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_source_owner_row_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_source_epoch: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_owner_row_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_producer_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_materialization_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_source_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodedField {
    pub name: String,
    pub width_bits: u8,
    pub value_u64: u64,
    pub value_i64: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodedInstruction {
    pub uid: String,
    pub mnemonic: String,
    pub asm: String,
    pub group: String,
    pub encoding_kind: String,
    pub length_bits: u8,
    pub mask: u64,
    pub match_bits: u64,
    pub instruction_bits: u64,
    pub uop_group: String,
    pub fields: Vec<DecodedField>,
}

impl DecodedInstruction {
    pub fn field(&self, name: &str) -> Option<&DecodedField> {
        self.fields.iter().find(|field| field.name == name)
    }

    pub fn length_bytes(&self) -> u8 {
        self.length_bits / 8
    }
}

#[derive(Debug, Deserialize)]
struct RawIsaBundle {
    instructions: Vec<RawInstruction>,
}

#[derive(Debug, Deserialize)]
struct RawInstruction {
    asm: String,
    encoding: RawEncoding,
    encoding_kind: String,
    group: String,
    id: String,
    length_bits: u8,
    mnemonic: String,
    uop_group: String,
}

#[derive(Debug, Deserialize)]
struct RawEncoding {
    parts: Vec<RawEncodingPart>,
}

#[derive(Debug, Deserialize)]
struct RawEncodingPart {
    fields: Vec<RawField>,
    mask: String,
    #[serde(rename = "match")]
    match_bits: String,
    width_bits: u8,
}

#[derive(Debug, Deserialize)]
struct RawField {
    name: String,
    pieces: Vec<RawFieldPiece>,
    signed: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RawFieldPiece {
    insn_lsb: u8,
    #[serde(rename = "insn_msb")]
    _insn_msb: u8,
    value_lsb: Option<u8>,
    value_msb: Option<u8>,
    width: u8,
}

#[derive(Debug, Clone)]
struct DecodePiece {
    insn_lsb: u8,
    width: u8,
    value_lsb: u8,
}

#[derive(Debug, Clone)]
struct DecodeFieldSpec {
    name: String,
    width_bits: u8,
    signed: bool,
    pieces: Vec<DecodePiece>,
}

#[derive(Debug, Clone)]
struct DecodeForm {
    uid: String,
    mnemonic: String,
    asm: String,
    group: String,
    encoding_kind: String,
    length_bits: u8,
    mask: u64,
    match_bits: u64,
    mask_popcount: u32,
    uop_group: String,
    fields: Vec<DecodeFieldSpec>,
}

#[derive(Debug, Default)]
struct DecodeTables {
    forms16: Vec<DecodeForm>,
    forms32: Vec<DecodeForm>,
    forms48: Vec<DecodeForm>,
    forms64: Vec<DecodeForm>,
    all_forms: Vec<DecodeForm>,
}

static DECODE_TABLES: OnceLock<DecodeTables> = OnceLock::new();

pub fn decode_word(insn_word: u64) -> Option<DecodedInstruction> {
    let tables = decode_tables();
    best_match(&tables.all_forms, insn_word).map(|form| render_decoded(form, insn_word))
}

pub fn decode_form_count() -> usize {
    decode_tables().all_forms.len()
}

fn decode_tables() -> &'static DecodeTables {
    DECODE_TABLES.get_or_init(build_decode_tables)
}

fn build_decode_tables() -> DecodeTables {
    let raw: RawIsaBundle = serde_json::from_str(include_str!("../data/linxisa-v0.4.json"))
        .expect("failed to parse embedded LinxISA v0.4 JSON");

    let mut tables = DecodeTables::default();
    for instruction in raw.instructions {
        let form = build_form(instruction);
        match form.length_bits {
            16 => tables.forms16.push(form.clone()),
            32 => tables.forms32.push(form.clone()),
            48 => tables.forms48.push(form.clone()),
            64 => tables.forms64.push(form.clone()),
            other => panic!("unsupported instruction length {other}"),
        }
        tables.all_forms.push(form);
    }
    tables
}

fn build_form(instruction: RawInstruction) -> DecodeForm {
    let offsets = part_offsets(&instruction.encoding.parts);
    let mut mask = 0u64;
    let mut match_bits = 0u64;
    let mut fields_by_name: BTreeMap<String, DecodeFieldSpec> = BTreeMap::new();

    for (part_idx, part) in instruction.encoding.parts.iter().enumerate() {
        let offset = offsets[part_idx];
        mask |= parse_hex_u64(&part.mask) << offset;
        match_bits |= parse_hex_u64(&part.match_bits) << offset;

        for field in &part.fields {
            let entry =
                fields_by_name
                    .entry(field.name.clone())
                    .or_insert_with(|| DecodeFieldSpec {
                        name: field.name.clone(),
                        width_bits: 0,
                        signed: field.signed.unwrap_or(false),
                        pieces: Vec::new(),
                    });
            entry.signed = entry.signed || field.signed.unwrap_or(false);
            for piece in &field.pieces {
                let value_lsb = piece.value_lsb.unwrap_or(0);
                let candidate_width = piece
                    .value_msb
                    .map(|msb| msb + 1)
                    .unwrap_or(value_lsb.saturating_add(piece.width));
                entry.width_bits = entry.width_bits.max(candidate_width);
                entry.pieces.push(DecodePiece {
                    insn_lsb: offset + piece.insn_lsb,
                    width: piece.width,
                    value_lsb,
                });
            }
        }
    }

    DecodeForm {
        uid: instruction.id,
        mnemonic: instruction.mnemonic,
        asm: instruction.asm,
        group: instruction.group,
        encoding_kind: instruction.encoding_kind,
        length_bits: instruction.length_bits,
        mask,
        match_bits,
        mask_popcount: mask.count_ones(),
        uop_group: instruction.uop_group,
        fields: fields_by_name.into_values().collect(),
    }
}

fn part_offsets(parts: &[RawEncodingPart]) -> Vec<u8> {
    let mut offsets = vec![0u8; parts.len()];
    let mut running = 0u8;
    for (idx, part) in parts.iter().enumerate().rev() {
        offsets[idx] = running;
        running = running.saturating_add(part.width_bits);
    }
    offsets
}

fn parse_hex_u64(text: &str) -> u64 {
    let trimmed = text.trim().trim_start_matches("0x");
    u64::from_str_radix(trimmed, 16).expect("invalid hex value in ISA JSON")
}

fn best_match(forms: &[DecodeForm], word: u64) -> Option<&DecodeForm> {
    forms
        .iter()
        .filter(|form| (word & form.mask) == form.match_bits)
        .max_by(|lhs, rhs| {
            lhs.mask_popcount
                .cmp(&rhs.mask_popcount)
                .then(lhs.length_bits.cmp(&rhs.length_bits))
        })
}

fn render_decoded(form: &DecodeForm, word: u64) -> DecodedInstruction {
    let fields = form
        .fields
        .iter()
        .map(|field| {
            let value = extract_field(word, field);
            DecodedField {
                name: field.name.clone(),
                width_bits: field.width_bits,
                value_u64: value,
                value_i64: field.signed.then(|| sign_extend(value, field.width_bits)),
            }
        })
        .collect();

    DecodedInstruction {
        uid: form.uid.clone(),
        mnemonic: form.mnemonic.clone(),
        asm: form.asm.clone(),
        group: form.group.clone(),
        encoding_kind: form.encoding_kind.clone(),
        length_bits: form.length_bits,
        mask: form.mask,
        match_bits: form.match_bits,
        instruction_bits: truncate_word(word, form.length_bits),
        uop_group: form.uop_group.clone(),
        fields,
    }
}

fn extract_field(word: u64, field: &DecodeFieldSpec) -> u64 {
    let mut value = 0u64;
    for piece in &field.pieces {
        let mask = low_mask(piece.width);
        let piece_bits = (word >> piece.insn_lsb) & mask;
        value |= piece_bits << piece.value_lsb;
    }
    value
}

fn truncate_word(word: u64, length_bits: u8) -> u64 {
    if length_bits >= 64 {
        word
    } else {
        word & low_mask(length_bits)
    }
}

fn low_mask(width: u8) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    }
}

fn sign_extend(value: u64, width_bits: u8) -> i64 {
    if width_bits == 0 {
        return 0;
    }
    if width_bits >= 64 {
        return value as i64;
    }
    let shift = 64 - width_bits;
    ((value << shift) as i64) >> shift
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_record_meets_schema_shape() {
        let rec = CommitRecord::unsupported(
            0,
            0x1000,
            0xDEAD_BEEF,
            TRAP_ILLEGAL_INST,
            &BlockMeta::default(),
        );
        assert_eq!(rec.schema_version, TRACE_SCHEMA_VERSION);
        assert_eq!(rec.trap_valid, 1);
        assert_eq!(rec.trap_cause, TRAP_ILLEGAL_INST);
    }

    #[test]
    fn decode_known_addi_fields() {
        let word = (0x123u64 << 20) | (5u64 << 15) | (7u64 << 7) | 0x15u64;
        let decoded = decode_word(word).expect("expected ADDI decode");
        assert_eq!(decoded.mnemonic, "ADDI");
        assert_eq!(decoded.length_bits, 32);
        assert_eq!(decoded.field("RegDst").unwrap().value_u64, 7);
        assert_eq!(decoded.field("SrcL").unwrap().value_u64, 5);
        assert_eq!(decoded.field("uimm12").unwrap().value_u64, 0x123);
    }

    #[test]
    fn decode_known_jr_split_immediate() {
        let imm = 0x345u64;
        let word = ((imm & 0x7F) << 25)
            | (3u64 << 20)
            | (2u64 << 15)
            | (((imm >> 7) & 0x1F) << 7)
            | 0x6027u64;
        let decoded = decode_word(word).expect("expected JR decode");
        assert_eq!(decoded.mnemonic, "JR");
        assert_eq!(decoded.field("SrcL").unwrap().value_u64, 2);
        assert_eq!(decoded.field("SrcZero").unwrap().value_u64, 3);
        assert_eq!(decoded.field("simm12").unwrap().value_u64, imm);
    }

    #[test]
    fn decode_corpus_covers_all_machine_forms() {
        let tables = decode_tables();
        assert!(tables.all_forms.len() >= 700);
        for form in &tables.all_forms {
            let decoded = decode_word(form.match_bits).unwrap_or_else(|| {
                panic!(
                    "failed to decode match bits for {} ({})",
                    form.uid, form.mnemonic
                )
            });
            assert_eq!(
                decoded.length_bits, form.length_bits,
                "length mismatch for {}",
                form.mnemonic
            );
            assert_eq!(
                decoded.instruction_bits & decoded.mask,
                decoded.match_bits,
                "decoded form does not self-match for {}",
                form.mnemonic
            );
        }
    }
}
