use super::*;
use elf::{LoadedElf, SegmentImage};
use isa::CommitRecord;
use runtime::GuestRuntime;
use runtime::{BootInfo, GuestMemory, MemoryRegion, RuntimeConfig};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;

fn test_qtag_wait_crossbar(iq: &[IqEntry], uops: &[CycleUop]) -> Vec<Vec<Vec<(usize, usize)>>> {
    let mut crossbar = empty_qtag_wait_crossbar();
    for entry in iq {
        register_iq_wait_crossbar_entry(&mut crossbar, entry.seq, &uops[entry.seq]);
    }
    crossbar
}

fn test_iq_tags(iq: &[IqEntry]) -> BTreeMap<usize, QTag> {
    let mut next_entry_ids = [0usize; PHYS_IQ_COUNT];
    let mut out = BTreeMap::new();
    for entry in iq {
        let entry_id = next_entry_ids[entry.phys_iq.index()];
        next_entry_ids[entry.phys_iq.index()] += 1;
        out.insert(
            entry.seq,
            QTag {
                phys_iq: entry.phys_iq,
                entry_id,
            },
        );
    }
    out
}

fn test_iq_owner_table(iq: &[IqEntry], iq_tags: &BTreeMap<usize, QTag>) -> Vec<Vec<Option<usize>>> {
    let mut owner_table = empty_iq_owner_table();
    rebuild_iq_owner_table(&mut owner_table, iq, iq_tags);
    owner_table
}

#[test]
fn cycle_engine_retires_multiple_uops() {
    let program = vec![
        enc_addi(2, 0, 1),
        enc_addi(3, 0, 2),
        enc_addi(4, 0, 3),
        enc_addi(5, 0, 4),
        enc_addi(9, 0, 93),
        enc_acrc(1),
    ];
    let runtime = sample_runtime(&program, &[]);
    let bundle = CycleEngine
        .run(
            &runtime,
            &CycleRunOptions {
                max_cycles: 64,
                ..CycleRunOptions::default()
            },
        )
        .unwrap();
    assert_eq!(bundle.result.metrics.exit_reason, "guest_exit(1)");
    assert_eq!(bundle.result.commits.len(), 6);
    assert!(
        bundle
            .stage_events
            .iter()
            .any(|event| event.stage_id == "IQ")
    );
    assert!(
        bundle
            .stage_events
            .iter()
            .any(|event| event.stage_id == "ROB")
    );
    assert!(
        bundle
            .stage_events
            .iter()
            .filter(|event| event.stage_id == "CMT")
            .count()
            > 0
    );
}

#[test]
fn dependent_uop_picks_after_producer_wakeup_window() {
    let program = vec![
        enc_addi(2, 0, 5),
        enc_addi(3, 2, 6),
        enc_addi(9, 0, 93),
        enc_acrc(1),
    ];
    let runtime = sample_runtime(&program, &[]);
    let bundle = CycleEngine
        .run(
            &runtime,
            &CycleRunOptions {
                max_cycles: 64,
                ..CycleRunOptions::default()
            },
        )
        .unwrap();

    let mut first_w1 = None;
    let mut second_p1 = None;
    for event in &bundle.stage_events {
        if event.row_id == "uop0" && event.stage_id == "W1" && first_w1.is_none() {
            first_w1 = Some(event.cycle);
        }
        if event.row_id == "uop1" && event.stage_id == "P1" && second_p1.is_none() {
            second_p1 = Some(event.cycle);
        }
    }

    assert!(first_w1.is_some());
    assert!(second_p1.is_some());
    assert!(second_p1.unwrap() > first_w1.unwrap());
}

#[test]
fn s2_captures_qtag_for_implicit_t_consumer() {
    let mut pipeline = StageQueues::default();
    pipeline.frontend[10].extend([0, 1]);
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    let producer = isa::decode_word(enc_addi(31, 0, 5) as u64).expect("decode implicit-t producer");
    let consumer =
        isa::decode_word(enc_addi(2, REG_T1 as u32, 6) as u64).expect("decode t1 consumer");
    let mut uops = vec![
        CycleUop {
            decoded: producer,
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: Some(QueueWakeKind::T),
            dst_logical_tag: Some(LogicalQueueTag {
                kind: QueueWakeKind::T,
                tag: 0,
            }),
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x2000),
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: consumer,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: REG_T1,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [Some(QueueWakeKind::T), None],
            src_logical_tags: [
                Some(LogicalQueueTag {
                    kind: QueueWakeKind::T,
                    tag: 0,
                }),
                None,
            ],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(
        uops[1].src_qtags[0],
        Some(QTag {
            phys_iq: PhysIq::AluIq0,
            entry_id: 0,
        })
    );
}

#[test]
fn later_implicit_t_consumer_captures_persistent_producer_qtag() {
    let producer = isa::decode_word(enc_addi(31, 0, 5) as u64).expect("decode implicit-t producer");
    let consumer =
        isa::decode_word(enc_addi(2, REG_T1 as u32, 6) as u64).expect("decode t1 consumer");
    let mut uops = vec![
        CycleUop {
            decoded: producer,
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: Some(QueueWakeKind::T),
            dst_logical_tag: Some(LogicalQueueTag {
                kind: QueueWakeKind::T,
                tag: 1,
            }),
            dst_qtag: Some(QTag {
                phys_iq: PhysIq::AluIq0,
                entry_id: 1,
            }),
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: Some(5),
        },
        CycleUop {
            decoded: consumer,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: REG_T1,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [Some(QueueWakeKind::T), None],
            src_logical_tags: [
                Some(LogicalQueueTag {
                    kind: QueueWakeKind::T,
                    tag: 1,
                }),
                None,
            ],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    annotate_qtag_sources(
        1,
        &BTreeMap::new(),
        &BTreeSet::new(),
        &BTreeSet::new(),
        &mut uops,
    );

    assert_eq!(
        uops[1].src_qtags[0],
        Some(QTag {
            phys_iq: PhysIq::AluIq0,
            entry_id: 1,
        })
    );
}

#[test]
fn build_uops_resolves_t_rel_to_latest_logical_t_tag() {
    let producer0 = isa::decode_word(16_474).expect("decode c.ldi ->t producer0");
    let producer1 = isa::decode_word(14_426).expect("decode c.ldi ->t producer1");
    let consumer = isa::decode_word(8_314).expect("decode c.sdi t#1 consumer");
    let commits = vec![
        CommitRecord {
            wb_valid: 1,
            wb_rd: REG_T1,
            mem_valid: 1,
            mem_is_store: 0,
            ..CommitRecord::unsupported(0, 0x1000, 16_474, 4, &isa::BlockMeta::default())
        },
        CommitRecord {
            wb_valid: 1,
            wb_rd: REG_T1,
            mem_valid: 1,
            mem_is_store: 0,
            ..CommitRecord::unsupported(0, 0x1002, 14_426, 4, &isa::BlockMeta::default())
        },
        CommitRecord {
            src0_valid: 1,
            src0_reg: REG_T1,
            mem_valid: 1,
            mem_is_store: 1,
            ..CommitRecord::unsupported(0, 0x1004, 8_314, 4, &isa::BlockMeta::default())
        },
    ];
    let decoded = vec![producer0, producer1, consumer];

    let uops = build_uops(&commits, &decoded);

    assert_eq!(
        uops[0].dst_logical_tag,
        Some(LogicalQueueTag {
            kind: QueueWakeKind::T,
            tag: 0,
        })
    );
    assert_eq!(
        uops[1].dst_logical_tag,
        Some(LogicalQueueTag {
            kind: QueueWakeKind::T,
            tag: 1,
        })
    );
    assert_eq!(
        uops[2].src_logical_tags[0],
        Some(LogicalQueueTag {
            kind: QueueWakeKind::T,
            tag: 1,
        })
    );
    assert_eq!(uops[2].deps[0], Some(1));
}

#[test]
fn ready_table_t_makes_late_consumer_ready_without_qtag() {
    let producer = isa::decode_word(16_474).expect("decode c.ldi ->t producer");
    let consumer = isa::decode_word(8_314).expect("decode c.sdi t#1 consumer");
    let mut uops = vec![
        CycleUop {
            decoded: producer,
            commit: CommitRecord {
                wb_valid: 1,
                wb_rd: REG_T1,
                mem_valid: 1,
                mem_is_store: 0,
                ..CommitRecord::unsupported(0, 0x1000, 16_474, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: Some(QueueWakeKind::T),
            dst_logical_tag: Some(LogicalQueueTag {
                kind: QueueWakeKind::T,
                tag: 0,
            }),
            dst_qtag: Some(QTag {
                phys_iq: PhysIq::AguIq0,
                entry_id: 3,
            }),
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(0),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: Some(2),
            e4_cycle: Some(3),
            w1_cycle: Some(3),
            done_cycle: Some(4),
        },
        CycleUop {
            decoded: consumer,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: REG_T1,
                mem_valid: 1,
                mem_is_store: 1,
                ..CommitRecord::unsupported(0, 0x1002, 8_314, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [Some(QueueWakeKind::T), None],
            src_logical_tags: [
                Some(LogicalQueueTag {
                    kind: QueueWakeKind::T,
                    tag: 0,
                }),
                None,
            ],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: true,
            load_ordinal: None,
            load_store_id: Some(1),
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut ready_table_t = BTreeSet::new();
    ready_table_t.insert(0);

    annotate_qtag_sources(
        1,
        &BTreeMap::new(),
        &ready_table_t,
        &BTreeSet::new(),
        &mut uops,
    );

    assert_eq!(uops[1].src_qtags[0], None);
    assert_eq!(iq_entry_wait_cause(1, 5, 1, &uops), None);
}

#[test]
fn implicit_t_consumer_needs_no_rf_read_ports() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord {
                wb_valid: 1,
                wb_rd: REG_T1,
                ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: Some(QueueWakeKind::T),
            dst_logical_tag: Some(LogicalQueueTag {
                kind: QueueWakeKind::T,
                tag: 0,
            }),
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: Some(6),
            data_ready_visible: Some(6),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: REG_T1,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [Some(QueueWakeKind::T), None],
            src_logical_tags: [
                Some(LogicalQueueTag {
                    kind: QueueWakeKind::T,
                    tag: 0,
                }),
                None,
            ],
            src_qtags: [
                Some(QTag {
                    phys_iq: PhysIq::AluIq0,
                    entry_id: 0,
                }),
                None,
            ],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    assert_eq!(read_ports_needed(1, 5, &uops), 0);
}

#[test]
fn iq_wait_cause_reports_wait_qtag_for_implicit_t_dependency() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord {
                wb_valid: 1,
                wb_rd: REG_T1,
                ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: Some(QueueWakeKind::T),
            dst_logical_tag: Some(LogicalQueueTag {
                kind: QueueWakeKind::T,
                tag: 0,
            }),
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: Some(6),
            data_ready_visible: Some(6),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: REG_T1,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [Some(QueueWakeKind::T), None],
            src_logical_tags: [
                Some(LogicalQueueTag {
                    kind: QueueWakeKind::T,
                    tag: 0,
                }),
                None,
            ],
            src_qtags: [
                Some(QTag {
                    phys_iq: PhysIq::AluIq0,
                    entry_id: 0,
                }),
                None,
            ],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    assert_eq!(iq_entry_wait_cause(1, 5, 0, &uops), Some("wait_qtag"));
    assert_eq!(iq_entry_wait_cause(1, 6, 0, &uops), None);
}

#[test]
fn completed_uops_leave_iq_and_w2() {
    let program = vec![
        enc_addi(2, 0, 1),
        enc_addi(3, 2, 2),
        enc_addi(9, 0, 93),
        enc_acrc(1),
    ];
    let runtime = sample_runtime(&program, &[]);
    let bundle = CycleEngine
        .run(
            &runtime,
            &CycleRunOptions {
                max_cycles: 64,
                ..CycleRunOptions::default()
            },
        )
        .unwrap();

    let retire_cycle = bundle
        .stage_events
        .iter()
        .find(|event| event.row_id == "uop0" && event.stage_id == "CMT")
        .map(|event| event.cycle)
        .expect("expected retirement");

    assert!(!bundle.stage_events.iter().any(|event| {
        event.row_id == "uop0" && event.stage_id == "IQ" && event.cycle > retire_cycle
    }));
    assert_eq!(
        bundle
            .stage_events
            .iter()
            .filter(|event| event.row_id == "uop0" && event.stage_id == "W2")
            .count(),
        1
    );
}

#[test]
fn fetch_serializes_at_unresolved_redirects() {
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 0,
            len: 4,
            next_pc: 0x1100,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord::unsupported(0, 0x1100, 0, 4, &isa::BlockMeta::default()),
    ];
    let decoded = vec![
        isa::decode_word(2048).expect("decode c.bstart.std"),
        isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi"),
    ];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut next_fetch_seq = 0usize;

    fill_fetch(0, &mut pipeline, &mut next_fetch_seq, &uops);
    assert_eq!(
        pipeline.frontend[0].iter().copied().collect::<Vec<_>>(),
        vec![0]
    );

    pipeline.frontend[0].clear();
    fill_fetch(1, &mut pipeline, &mut next_fetch_seq, &uops);
    assert!(pipeline.frontend[0].is_empty());

    uops[0].w1_cycle = Some(4);
    fill_fetch(4, &mut pipeline, &mut next_fetch_seq, &uops);
    assert_eq!(
        pipeline.frontend[0].iter().copied().collect::<Vec<_>>(),
        vec![1]
    );
}

#[test]
fn fetch_does_not_serialize_on_bru_correction_before_boundary() {
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 0,
            len: 4,
            next_pc: 0x2000,
            src0_valid: 1,
            src0_reg: 2,
            src0_data: 1,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 2,
            dst_data: 1,
            wb_valid: 1,
            wb_rd: 2,
            wb_data: 1,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1004,
            insn: 2048,
            len: 2,
            next_pc: 0x2000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord::unsupported(0, 0x2000, 0, 4, &isa::BlockMeta::default()),
    ];
    let decoded = vec![
        isa::decode_word(30_478_677).expect("decode cmp.nei"),
        isa::decode_word(2048).expect("decode c.bstart.std"),
        isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi"),
    ];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut next_fetch_seq = 0usize;

    assert_eq!(uops[0].redirect_target, None);
    assert_eq!(uops[1].redirect_target, Some(0x2000));

    fill_fetch(0, &mut pipeline, &mut next_fetch_seq, &uops);
    assert_eq!(
        pipeline.frontend[0].iter().copied().collect::<Vec<_>>(),
        vec![0, 1]
    );

    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    uops[0].w1_cycle = Some(7);
    uops[1].w1_cycle = Some(7);
    let mut out = Vec::new();
    emit_stage_events(
        7,
        &runtime,
        &StageQueues::default(),
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );
    assert!(
        !out.iter()
            .any(|event| event.row_id == "uop0" && event.stage_id == "FLS")
    );
    assert!(
        out.iter()
            .any(|event| event.row_id == "uop1" && event.stage_id == "FLS")
    );
}

#[test]
fn boundary_consumes_pending_bru_correction_before_local_target() {
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 0,
            len: 4,
            next_pc: 0x2000,
            src0_valid: 1,
            src0_reg: 2,
            src0_data: 1,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 2,
            dst_data: 1,
            wb_valid: 1,
            wb_rd: 2,
            wb_data: 1,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1004,
            insn: 0,
            len: 2,
            next_pc: 0x1006,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord::unsupported(0, 0x2000, 0, 4, &isa::BlockMeta::default()),
    ];
    let decoded = vec![
        isa::decode_word(30_478_677).expect("decode cmp.nei"),
        isa::decode_word(0).expect("decode c.bstop"),
        isa::decode_word(2048).expect("decode c.bstart.std"),
    ];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();

    uops[0].w1_cycle = Some(5);
    uops[1].w1_cycle = Some(7);
    publish_bru_correction_state(5, &mut pipeline, &uops);
    assert_eq!(
        pipeline.pending_bru_correction,
        Some(BruCorrectionState {
            source_seq: 0,
            epoch: 0,
            actual_take: true,
            target_pc: 0x2000,
            checkpoint_id: checkpoint_id_for_seq(0, &uops),
            visible_cycle: 5,
        })
    );

    schedule_frontend_redirect_recovery(7, &mut pipeline, &uops);
    assert_eq!(
        pipeline.frontend_redirect,
        Some(FrontendRedirectState {
            source_seq: 1,
            target_pc: 0x2000,
            restart_seq: 2,
            checkpoint_id: checkpoint_id_for_seq(0, &uops),
            from_correction: true,
            resume_cycle: 8,
        })
    );
    assert_eq!(
        pipeline.flush_checkpoint_id,
        Some(checkpoint_id_for_seq(0, &uops))
    );
    assert_eq!(pipeline.pending_bru_correction, None);
}

#[test]
fn boundary_consumes_not_taken_bru_correction_to_fallthrough() {
    let start = isa::decode_word(4).expect("decode c.bstart cond");
    let bru = isa::decode_word(30_478_677).expect("decode cmp.nei");
    let bstop = isa::decode_word(0).expect("decode c.bstop");
    let target = isa::decode_word(2048).expect("decode target c.bstart.std");
    let mut uops = vec![
        CycleUop {
            decoded: start.clone(),
            commit: CommitRecord {
                schema_version: "1.0".to_string(),
                cycle: 0,
                pc: 0x1000,
                insn: 4,
                len: 2,
                next_pc: 0x0ff0,
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
                trap_valid: 0,
                trap_cause: 0,
                traparg0: 0,
                block_kind: "sys".to_string(),
                lane_id: "scalar0".to_string(),
                tile_meta: String::new(),
                tile_ref_src: 0,
                tile_ref_dst: 0,
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x0ff0),
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: bru,
            commit: CommitRecord {
                schema_version: "1.0".to_string(),
                cycle: 1,
                pc: 0x1002,
                insn: 30_478_677,
                len: 4,
                next_pc: 0x1006,
                src0_valid: 1,
                src0_reg: 2,
                src0_data: 0,
                src1_valid: 0,
                src1_reg: 0,
                src1_data: 0,
                dst_valid: 1,
                dst_reg: 2,
                dst_data: 0,
                wb_valid: 1,
                wb_rd: 2,
                wb_data: 0,
                mem_valid: 0,
                mem_is_store: 0,
                mem_addr: 0,
                mem_wdata: 0,
                mem_rdata: 0,
                mem_size: 0,
                trap_valid: 0,
                trap_cause: 0,
                traparg0: 0,
                block_kind: "sys".to_string(),
                lane_id: "scalar0".to_string(),
                tile_meta: String::new(),
                tile_ref_src: 0,
                tile_ref_dst: 0,
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(5),
            done_cycle: None,
        },
        CycleUop {
            decoded: bstop,
            commit: CommitRecord {
                schema_version: "1.0".to_string(),
                cycle: 2,
                pc: 0x1006,
                insn: 0,
                len: 2,
                next_pc: 0x1008,
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
                trap_valid: 0,
                trap_cause: 0,
                traparg0: 0,
                block_kind: "sys".to_string(),
                lane_id: "scalar0".to_string(),
                tile_meta: String::new(),
                tile_ref_src: 0,
                tile_ref_dst: 0,
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(7),
            done_cycle: None,
        },
        CycleUop {
            decoded: target,
            commit: CommitRecord::unsupported(0, 0x2000, 2048, 2, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1, 2]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);
    pipeline.seq_checkpoint_ids.insert(2, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);
    assert_eq!(
        pipeline.seq_branch_contexts.get(&1).copied(),
        Some(BranchOwnerContext {
            kind: BranchOwnerKind::Cond,
            base_pc: 0x1000,
            target_pc: 0x0ff0,
            off: 0xfffffffffffffff0,
            pred_take: true,
            epoch: 1,
        })
    );
    publish_bru_correction_state(5, &mut pipeline, &uops);

    assert_eq!(
        pipeline.pending_bru_correction,
        Some(BruCorrectionState {
            source_seq: 1,
            epoch: 1,
            actual_take: false,
            target_pc: 0x0ff0,
            checkpoint_id: 0,
            visible_cycle: 5,
        })
    );

    schedule_frontend_redirect_recovery(7, &mut pipeline, &uops);
    assert_eq!(
        pipeline.frontend_redirect,
        Some(FrontendRedirectState {
            source_seq: 2,
            target_pc: 0x1008,
            restart_seq: 3,
            checkpoint_id: 0,
            from_correction: true,
            resume_cycle: 8,
        })
    );
    assert_eq!(pipeline.pending_bru_correction, None);
}

#[test]
fn later_boundary_epoch_does_not_consume_stale_bru_correction() {
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 0,
            len: 4,
            next_pc: 0x3000,
            src0_valid: 1,
            src0_reg: 2,
            src0_data: 1,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 2,
            dst_data: 1,
            wb_valid: 1,
            wb_rd: 2,
            wb_data: 1,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1004,
            insn: 0,
            len: 2,
            next_pc: 0x1006,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord::unsupported(0, 0x2000, 0, 2, &isa::BlockMeta::default()),
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 3,
            pc: 0x2002,
            insn: 0,
            len: 2,
            next_pc: 0x4000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord::unsupported(0, 0x4000, 0, 2, &isa::BlockMeta::default()),
        CommitRecord::unsupported(0, 0x3000, 0, 2, &isa::BlockMeta::default()),
    ];
    let decoded = vec![
        isa::decode_word(30_478_677).expect("decode cmp.nei"),
        isa::decode_word(0).expect("decode c.bstop"),
        isa::decode_word(2048).expect("decode c.bstart.std"),
        isa::decode_word(0).expect("decode c.bstop"),
        isa::decode_word(2048).expect("decode target c.bstart.std"),
        isa::decode_word(2048).expect("decode stale-target c.bstart.std"),
    ];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();

    uops[0].w1_cycle = Some(5);
    uops[3].w1_cycle = Some(9);
    publish_bru_correction_state(5, &mut pipeline, &uops);
    assert_eq!(
        pipeline.pending_bru_correction,
        Some(BruCorrectionState {
            source_seq: 0,
            epoch: 0,
            actual_take: true,
            target_pc: 0x3000,
            checkpoint_id: checkpoint_id_for_seq(0, &uops),
            visible_cycle: 5,
        })
    );

    schedule_frontend_redirect_recovery(9, &mut pipeline, &uops);
    assert_eq!(
        pipeline.frontend_redirect,
        Some(FrontendRedirectState {
            source_seq: 3,
            target_pc: 0x4000,
            restart_seq: 4,
            checkpoint_id: checkpoint_id_for_seq(3, &uops),
            from_correction: false,
            resume_cycle: 10,
        })
    );
    assert_eq!(
        pipeline.flush_checkpoint_id,
        Some(checkpoint_id_for_seq(3, &uops))
    );
    assert_eq!(pipeline.pending_bru_correction, None);
}

#[test]
fn invalid_bru_recovery_target_raises_pending_precise_trap() {
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 0,
            len: 4,
            next_pc: 0x3000,
            src0_valid: 1,
            src0_reg: 2,
            src0_data: 1,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 2,
            dst_data: 1,
            wb_valid: 1,
            wb_rd: 2,
            wb_data: 1,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1004,
            insn: 0,
            len: 2,
            next_pc: 0x1006,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
    ];
    let decoded = vec![
        isa::decode_word(30_478_677).expect("decode cmp.nei"),
        isa::decode_word(0).expect("decode c.bstop"),
    ];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();

    uops[0].w1_cycle = Some(5);
    publish_bru_correction_state(5, &mut pipeline, &uops);

    assert_eq!(pipeline.pending_bru_correction, None);
    assert_eq!(
        pipeline.pending_trap,
        Some(PendingTrapState {
            seq: 0,
            cause: isa::TRAP_BRU_RECOVERY_NOT_BSTART,
            traparg0: 0x1000,
            checkpoint_id: checkpoint_id_for_seq(0, &uops),
            visible_cycle: 5,
        })
    );
}

#[test]
fn retire_ready_attaches_bru_recovery_trap_to_offending_uop() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let mut pipeline = StageQueues::default();
    pipeline.pending_trap = Some(PendingTrapState {
        seq: 0,
        cause: isa::TRAP_BRU_RECOVERY_NOT_BSTART,
        traparg0: 0x1000,
        checkpoint_id: 0,
        visible_cycle: 3,
    });
    let mut committed = Vec::new();
    let mut retired = Vec::new();
    let mut stage_events = Vec::new();
    let mut rob = VecDeque::from([0usize]);
    let mut uops = vec![CycleUop {
        decoded,
        commit: CommitRecord::unsupported(
            0,
            0x1000,
            enc_addi(2, 0, 1) as u64,
            0,
            &isa::BlockMeta::default(),
        ),
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: false,
        load_ordinal: None,
        load_store_id: None,
        miss_injected: false,
        redirect_target: None,
        phys_iq: None,
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: Some(3),
        done_cycle: Some(3),
    }];

    let trap = retire_ready(
        3,
        &runtime,
        &mut rob,
        &mut committed,
        &mut retired,
        &mut pipeline,
        &mut uops,
        &mut stage_events,
    );

    assert_eq!(trap, Some(isa::TRAP_BRU_RECOVERY_NOT_BSTART));
    assert_eq!(committed.len(), 1);
    assert_eq!(committed[0].trap_valid, 1);
    assert_eq!(committed[0].trap_cause, isa::TRAP_BRU_RECOVERY_NOT_BSTART);
    assert_eq!(committed[0].traparg0, 0x1000);
    assert_eq!(stage_events.len(), 1);
    assert_eq!(stage_events[0].checkpoint_id, Some(0));
    assert_eq!(
        stage_events[0].trap_cause,
        Some(isa::TRAP_BRU_RECOVERY_NOT_BSTART)
    );
    assert_eq!(stage_events[0].traparg0, Some(0x1000));
    assert_eq!(pipeline.pending_trap, None);
}

#[test]
fn frontend_redirect_restart_waits_until_next_cycle() {
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 0,
            len: 4,
            next_pc: 0x1100,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord::unsupported(0, 0x1100, 0, 4, &isa::BlockMeta::default()),
    ];
    let decoded = vec![
        isa::decode_word(2048).expect("decode c.bstart.std"),
        isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi"),
    ];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    let mut next_fetch_seq = 1usize;

    uops[0].w1_cycle = Some(7);
    schedule_frontend_redirect_recovery(7, &mut pipeline, &uops);
    assert_eq!(
        pipeline.frontend_redirect,
        Some(FrontendRedirectState {
            source_seq: 0,
            target_pc: 0x1100,
            restart_seq: 1,
            checkpoint_id: checkpoint_id_for_seq(0, &uops),
            from_correction: false,
            resume_cycle: 8,
        }),
        "redirect restart should be delayed to the next cycle"
    );
    assert_eq!(
        pipeline.flush_checkpoint_id,
        Some(checkpoint_id_for_seq(0, &uops))
    );
    assert_eq!(
        pipeline.pending_flush,
        Some(PendingFlushState {
            flush_seq: 0,
            checkpoint_id: checkpoint_id_for_seq(0, &uops),
            apply_cycle: 8,
        })
    );

    fill_fetch(7, &mut pipeline, &mut next_fetch_seq, &uops);
    assert!(pipeline.frontend[0].is_empty());

    apply_pending_flush(7, &mut pipeline, &mut iq, &mut rob, &uops);
    assert_eq!(
        pipeline.pending_flush,
        Some(PendingFlushState {
            flush_seq: 0,
            checkpoint_id: checkpoint_id_for_seq(0, &uops),
            apply_cycle: 8,
        })
    );
    apply_pending_flush(8, &mut pipeline, &mut iq, &mut rob, &uops);
    assert_eq!(pipeline.pending_flush, None);
    fill_fetch(8, &mut pipeline, &mut next_fetch_seq, &uops);
    assert_eq!(
        pipeline.frontend[0].iter().copied().collect::<Vec<_>>(),
        vec![1]
    );
}

#[test]
fn pending_flush_prunes_speculative_state_on_registered_cycle() {
    let boundary = isa::decode_word(2048).expect("decode c.bstart.std");
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: boundary,
            commit: CommitRecord {
                schema_version: "1.0".to_string(),
                cycle: 0,
                pc: 0x1000,
                insn: 2048,
                len: 2,
                next_pc: 0x1100,
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
                trap_valid: 0,
                trap_cause: 0,
                traparg0: 0,
                block_kind: "sys".to_string(),
                lane_id: "scalar0".to_string(),
                tile_meta: String::new(),
                tile_ref_src: 0,
                tile_ref_dst: 0,
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x1100),
            phys_iq: Some(PhysIq::BruIq),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(7),
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord::unsupported(
                0,
                0x1002,
                enc_addi(2, 0, 1) as u64,
                4,
                &isa::BlockMeta::default(),
            ),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::SharedIq1),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut pipeline = StageQueues::default();
    let mut iq = vec![IqEntry {
        seq: 1,
        phys_iq: PhysIq::SharedIq1,
        inflight: false,
        src_valid: [false, false],
        src_ready_nonspec: [false, false],
        src_ready_spec: [false, false],
        src_wait_qtag: [false, false],
    }];
    let mut rob = VecDeque::from([0usize, 1usize]);
    pipeline.frontend[0].push_back(1);
    pipeline.iq_tags.insert(
        1,
        QTag {
            phys_iq: PhysIq::SharedIq1,
            entry_id: 0,
        },
    );
    rebuild_iq_owner_table(&mut pipeline.iq_owner_table, &iq, &pipeline.iq_tags);

    schedule_frontend_redirect_recovery(7, &mut pipeline, &uops);
    assert_eq!(
        pipeline.frontend[0].iter().copied().collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(
        iq.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(rob.iter().copied().collect::<Vec<_>>(), vec![0, 1]);

    apply_pending_flush(7, &mut pipeline, &mut iq, &mut rob, &uops);
    assert_eq!(
        pipeline.frontend[0].iter().copied().collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(
        iq.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(rob.iter().copied().collect::<Vec<_>>(), vec![0, 1]);

    apply_pending_flush(8, &mut pipeline, &mut iq, &mut rob, &uops);
    assert!(pipeline.frontend[0].is_empty());
    assert!(iq.is_empty());
    assert_eq!(rob.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(pipeline.pending_flush, None);
}

#[test]
fn pending_flush_restores_ready_tables_from_checkpoint_snapshot() {
    let boundary = isa::decode_word(2048).expect("decode c.bstart.std");
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: boundary,
            commit: CommitRecord {
                schema_version: "1.0".to_string(),
                cycle: 0,
                pc: 0x1000,
                insn: 2048,
                len: 2,
                next_pc: 0x1100,
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
                trap_valid: 0,
                trap_cause: 0,
                traparg0: 0,
                block_kind: "sys".to_string(),
                lane_id: "scalar0".to_string(),
                tile_meta: String::new(),
                tile_ref_src: 0,
                tile_ref_dst: 0,
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x1100),
            phys_iq: Some(PhysIq::BruIq),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(7),
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord::unsupported(
                0,
                0x1002,
                enc_addi(2, 0, 1) as u64,
                4,
                &isa::BlockMeta::default(),
            ),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::from([0usize]);
    pipeline.ready_table_t = BTreeSet::from([1, 3]);
    pipeline.ready_table_u = BTreeSet::from([2]);
    pipeline.ready_table_checkpoints.insert(
        5,
        ReadyTableCheckpoint {
            ready_table_t: pipeline.ready_table_t.clone(),
            ready_table_u: pipeline.ready_table_u.clone(),
            recovery_epoch: 7,
            block_head: false,
            branch_context: BranchOwnerContext {
                kind: BranchOwnerKind::Cond,
                base_pc: 0x1000,
                target_pc: 0x2000,
                off: 0x1000,
                pred_take: true,
                epoch: 7,
            },
            dynamic_target_pc: Some(0x2222),
            dynamic_target_owner_seq: Some(6),
            dynamic_target_producer_kind: Some(ReturnConsumerKind::SetcTgt),
            dynamic_target_setup_epoch: Some(7),
            dynamic_target_owner_kind: Some(ReturnConsumerKind::SetcTgt),
            dynamic_target_source_owner_seq: Some(4),
            dynamic_target_source_epoch: Some(6),
            dynamic_target_source_kind: Some(DynamicTargetSourceKind::ArchTargetSetup),
            dynamic_target_call_materialization_kind: Some(CallMaterializationKind::AdjacentSetret),
            call_header_seq: Some(11),
            call_return_target_pc: Some(0x3333),
            call_return_target_owner_seq: Some(12),
            call_return_target_epoch: Some(6),
            call_return_materialization_kind: Some(CallMaterializationKind::AdjacentSetret),
        },
    );
    pipeline.ready_table_t.insert(9);
    pipeline.ready_table_u.insert(10);
    pipeline.active_recovery_epoch = 12;
    pipeline.active_block_head = true;
    pipeline.active_branch_context = BranchOwnerContext {
        kind: BranchOwnerKind::Ret,
        base_pc: 0x3000,
        target_pc: 0x3010,
        off: 0x10,
        pred_take: true,
        epoch: 12,
    };
    pipeline.active_dynamic_target_pc = Some(0x9999);
    pipeline.active_dynamic_target_owner_seq = Some(23);
    pipeline.active_dynamic_target_producer_kind = Some(ReturnConsumerKind::SetcTgt);
    pipeline.active_dynamic_target_setup_epoch = Some(12);
    pipeline.active_dynamic_target_owner_kind = Some(ReturnConsumerKind::FretStk);
    pipeline.active_dynamic_target_source_owner_seq = Some(21);
    pipeline.active_dynamic_target_source_epoch = Some(11);
    pipeline.active_dynamic_target_source_kind = Some(DynamicTargetSourceKind::ArchTargetSetup);
    pipeline.active_dynamic_target_call_materialization_kind =
        Some(CallMaterializationKind::FusedCall);
    pipeline.active_call_header_seq = Some(22);
    pipeline.active_call_return_target_pc = Some(0x4444);
    pipeline.active_call_return_target_owner_seq = Some(24);
    pipeline.active_call_return_target_epoch = Some(11);
    pipeline.active_call_return_materialization_kind = Some(CallMaterializationKind::FusedCall);
    pipeline.pending_flush = Some(PendingFlushState {
        flush_seq: 0,
        checkpoint_id: 5,
        apply_cycle: 8,
    });

    apply_pending_flush(8, &mut pipeline, &mut iq, &mut rob, &uops);

    assert_eq!(pipeline.ready_table_t, BTreeSet::from([1, 3]));
    assert_eq!(pipeline.ready_table_u, BTreeSet::from([2]));
    assert_eq!(pipeline.active_recovery_epoch, 7);
    assert!(!pipeline.active_block_head);
    assert_eq!(
        pipeline.active_branch_context,
        BranchOwnerContext {
            kind: BranchOwnerKind::Cond,
            base_pc: 0x1000,
            target_pc: 0x2000,
            off: 0x1000,
            pred_take: true,
            epoch: 7,
        }
    );
    assert_eq!(pipeline.active_dynamic_target_pc, Some(0x2222));
    assert_eq!(pipeline.active_dynamic_target_owner_seq, Some(6));
    assert_eq!(
        pipeline.active_dynamic_target_producer_kind,
        Some(ReturnConsumerKind::SetcTgt)
    );
    assert_eq!(pipeline.active_dynamic_target_setup_epoch, Some(7));
    assert_eq!(
        pipeline.active_dynamic_target_owner_kind,
        Some(ReturnConsumerKind::SetcTgt)
    );
    assert_eq!(pipeline.active_dynamic_target_source_owner_seq, Some(4));
    assert_eq!(pipeline.active_dynamic_target_source_epoch, Some(6));
    assert_eq!(
        pipeline.active_dynamic_target_source_kind,
        Some(DynamicTargetSourceKind::ArchTargetSetup)
    );
    assert_eq!(
        pipeline.active_dynamic_target_call_materialization_kind,
        Some(CallMaterializationKind::AdjacentSetret)
    );
    assert_eq!(pipeline.active_call_header_seq, Some(11));
    assert_eq!(pipeline.active_call_return_target_pc, Some(0x3333));
    assert_eq!(pipeline.active_call_return_target_owner_seq, Some(12));
    assert_eq!(pipeline.active_call_return_target_epoch, Some(6));
    assert_eq!(
        pipeline.active_call_return_materialization_kind,
        Some(CallMaterializationKind::AdjacentSetret)
    );
    assert_eq!(pipeline.pending_flush, None);
}

#[test]
fn frontend_redirect_restart_uses_legal_block_start_target_seq() {
    let redirect = isa::decode_word(2048).expect("decode c.bstart.std");
    let wrong_path = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let target_head = isa::decode_word(2048).expect("decode target c.bstart.std");
    let target_body = isa::decode_word(enc_addi(3, 0, 2) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: redirect,
            commit: CommitRecord {
                schema_version: "1.0".to_string(),
                cycle: 0,
                pc: 0x1000,
                insn: 2048,
                len: 2,
                next_pc: 0x2000,
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
                trap_valid: 0,
                trap_cause: 0,
                traparg0: 0,
                block_kind: "sys".to_string(),
                lane_id: "scalar0".to_string(),
                tile_meta: String::new(),
                tile_ref_src: 0,
                tile_ref_dst: 0,
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x2000),
            phys_iq: Some(PhysIq::CmdIq),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(5),
            done_cycle: None,
        },
        CycleUop {
            decoded: wrong_path,
            commit: CommitRecord::unsupported(0, 0x1002, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: target_head,
            commit: CommitRecord {
                pc: 0x2000,
                insn: 2048,
                len: 2,
                next_pc: 0x2002,
                ..CommitRecord::unsupported(0, 0x2000, 0, 2, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: target_body,
            commit: CommitRecord::unsupported(0, 0x2002, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut pipeline = StageQueues::default();
    let mut next_fetch_seq = 4usize;

    schedule_frontend_redirect_recovery(5, &mut pipeline, &uops);
    assert_eq!(
        pipeline.frontend_redirect,
        Some(FrontendRedirectState {
            source_seq: 0,
            target_pc: 0x2000,
            restart_seq: 2,
            checkpoint_id: checkpoint_id_for_seq(0, &uops),
            from_correction: false,
            resume_cycle: 6,
        })
    );
    assert_eq!(
        pipeline.flush_checkpoint_id,
        Some(checkpoint_id_for_seq(0, &uops))
    );

    fill_fetch(6, &mut pipeline, &mut next_fetch_seq, &uops);
    assert_eq!(next_fetch_seq, 4);
    assert_eq!(
        pipeline.frontend[0].iter().copied().collect::<Vec<_>>(),
        vec![2, 3]
    );
}

#[test]
fn fill_fetch_assigns_packet_checkpoint_from_head_pc() {
    let uops = (0..5usize)
        .map(|idx| CycleUop {
            decoded: isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi"),
            commit: CommitRecord {
                schema_version: "1.0".to_string(),
                cycle: 0,
                pc: 0x1000 + (idx as u64) * 4,
                insn: enc_addi(2, 0, 1) as u64,
                len: 4,
                next_pc: 0x1004 + (idx as u64) * 4,
                src0_valid: 0,
                src0_reg: 0,
                src0_data: 0,
                src1_valid: 0,
                src1_reg: 0,
                src1_data: 0,
                dst_valid: 1,
                dst_reg: 2,
                dst_data: 1,
                wb_valid: 1,
                wb_rd: 2,
                wb_data: 1,
                mem_valid: 0,
                mem_is_store: 0,
                mem_addr: 0,
                mem_wdata: 0,
                mem_rdata: 0,
                mem_size: 0,
                trap_valid: 0,
                trap_cause: 0,
                traparg0: 0,
                block_kind: "sys".to_string(),
                lane_id: "scalar0".to_string(),
                tile_meta: String::new(),
                tile_ref_src: 0,
                tile_ref_dst: 0,
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        })
        .collect::<Vec<_>>();
    let mut pipeline = StageQueues::default();
    let mut next_fetch_seq = 0usize;

    fill_fetch(0, &mut pipeline, &mut next_fetch_seq, &uops);
    assert_eq!(pipeline.seq_checkpoint_ids.get(&0).copied(), Some(0));
    assert_eq!(pipeline.seq_checkpoint_ids.get(&1).copied(), Some(0));
    assert_eq!(pipeline.seq_checkpoint_ids.get(&2).copied(), Some(0));
    assert_eq!(pipeline.seq_checkpoint_ids.get(&3).copied(), Some(0));

    pipeline.frontend[0].clear();
    fill_fetch(1, &mut pipeline, &mut next_fetch_seq, &uops);
    assert_eq!(pipeline.seq_checkpoint_ids.get(&4).copied(), Some(4));
    assert_eq!(live_checkpoint_id_for_seq(4, &pipeline, &uops), 4);
}

#[test]
fn start_marker_dispatch_snapshots_ready_tables_for_checkpoint() {
    let decoded = isa::decode_word(2048).expect("decode c.bstart.std");
    let mut uops = vec![CycleUop {
        decoded,
        commit: CommitRecord::unsupported(0, 0x1000, 2048, 2, &isa::BlockMeta::default()),
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: false,
        load_ordinal: None,
        load_store_id: None,
        miss_injected: false,
        redirect_target: None,
        phys_iq: None,
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: None,
        done_cycle: None,
    }];
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].push_back(0);
    pipeline.seq_checkpoint_ids.insert(0, 5);
    pipeline.ready_table_t.extend([1, 3]);
    pipeline.ready_table_u.insert(2);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(
        pipeline.ready_table_checkpoints.get(&5),
        Some(&ReadyTableCheckpoint {
            ready_table_t: BTreeSet::from([1, 3]),
            ready_table_u: BTreeSet::from([2]),
            recovery_epoch: 0,
            block_head: true,
            branch_context: BranchOwnerContext::default(),
            dynamic_target_pc: None,
            dynamic_target_owner_seq: None,
            dynamic_target_producer_kind: None,
            dynamic_target_setup_epoch: None,
            dynamic_target_owner_kind: None,
            dynamic_target_source_owner_seq: None,
            dynamic_target_source_epoch: None,
            dynamic_target_source_kind: None,
            dynamic_target_call_materialization_kind: None,
            call_header_seq: None,
            call_return_target_pc: None,
            call_return_target_owner_seq: None,
            call_return_target_epoch: None,
            call_return_materialization_kind: None,
        })
    );
}

#[test]
fn start_marker_rob_checkpoint_id_uses_packet_slot_offset() {
    let plain = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let start = isa::decode_word(2048).expect("decode c.bstart.std");
    let mut uops = vec![
        CycleUop {
            decoded: plain,
            commit: CommitRecord::unsupported(
                0,
                0x1000,
                enc_addi(2, 0, 1) as u64,
                4,
                &isa::BlockMeta::default(),
            ),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: start.clone(),
            commit: CommitRecord {
                schema_version: "1.0".to_string(),
                cycle: 1,
                pc: 0x1004,
                insn: 2048,
                len: 2,
                next_pc: 0x2000,
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
                trap_valid: 0,
                trap_cause: 0,
                traparg0: 0,
                block_kind: "sys".to_string(),
                lane_id: "scalar0".to_string(),
                tile_meta: String::new(),
                tile_ref_src: 0,
                tile_ref_dst: 0,
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(pipeline.seq_rob_checkpoint_ids.get(&1).copied(), Some(1));
    assert_eq!(live_rob_checkpoint_id_for_seq(1, &pipeline, &uops), 1);
}

#[test]
fn dispatch_assigns_bru_recovery_checkpoint_from_backend_context() {
    let plain = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let start = isa::decode_word(2048).expect("decode c.bstart.std");
    let bru = isa::decode_word(30_478_677).expect("decode cmp.nei");
    let mut uops = vec![
        CycleUop {
            decoded: plain,
            commit: CommitRecord::unsupported(
                0,
                0x1000,
                enc_addi(2, 0, 1) as u64,
                4,
                &isa::BlockMeta::default(),
            ),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: start.clone(),
            commit: CommitRecord {
                schema_version: "1.0".to_string(),
                cycle: 1,
                pc: 0x1004,
                insn: 2048,
                len: 2,
                next_pc: 0x2000,
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
                trap_valid: 0,
                trap_cause: 0,
                traparg0: 0,
                block_kind: "sys".to_string(),
                lane_id: "scalar0".to_string(),
                tile_meta: String::new(),
                tile_ref_src: 0,
                tile_ref_dst: 0,
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x2000),
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: bru,
            commit: CommitRecord {
                schema_version: "1.0".to_string(),
                cycle: 2,
                pc: 0x1006,
                insn: 30_478_677,
                len: 4,
                next_pc: 0x2000,
                src0_valid: 1,
                src0_reg: 2,
                src0_data: 1,
                src1_valid: 0,
                src1_reg: 0,
                src1_data: 0,
                dst_valid: 1,
                dst_reg: 2,
                dst_data: 1,
                wb_valid: 1,
                wb_rd: 2,
                wb_data: 1,
                mem_valid: 0,
                mem_is_store: 0,
                mem_addr: 0,
                mem_wdata: 0,
                mem_rdata: 0,
                mem_size: 0,
                trap_valid: 0,
                trap_cause: 0,
                traparg0: 0,
                block_kind: "sys".to_string(),
                lane_id: "scalar0".to_string(),
                tile_meta: String::new(),
                tile_ref_src: 0,
                tile_ref_dst: 0,
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(5),
            done_cycle: None,
        },
        CycleUop {
            decoded: start,
            commit: CommitRecord::unsupported(0, 0x2000, 2048, 2, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1, 2]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);
    pipeline.seq_checkpoint_ids.insert(2, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    publish_bru_correction_state(5, &mut pipeline, &uops);

    assert_eq!(pipeline.seq_rob_checkpoint_ids.get(&1).copied(), Some(1));
    assert_eq!(
        pipeline.seq_recovery_checkpoint_ids.get(&2).copied(),
        Some(1)
    );
    assert_eq!(pipeline.seq_recovery_epochs.get(&1).copied(), Some(0));
    assert_eq!(pipeline.seq_recovery_epochs.get(&2).copied(), Some(1));
    assert_eq!(
        pipeline.seq_branch_contexts.get(&2).copied(),
        Some(BranchOwnerContext {
            kind: BranchOwnerKind::Fall,
            base_pc: 0x1004,
            target_pc: 0x2000,
            off: 0xffc,
            pred_take: false,
            epoch: 1,
        })
    );
    assert_eq!(recovery_checkpoint_id_for_seq(2, &pipeline, &uops), 1);
    assert_eq!(
        pipeline.pending_bru_correction, None,
        "matching actual_take/pred_take should not publish a deferred correction"
    );
}

#[test]
fn dispatch_assigns_cond_boundary_prediction_from_target_direction() {
    fn run_case(pc: u64, target_pc: u64, expected_pred_take: bool) {
        let cond = isa::decode_word(4).expect("decode c.bstart cond");
        let plain = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
        let mut uops = vec![
            CycleUop {
                decoded: cond,
                commit: CommitRecord {
                    schema_version: "1.0".to_string(),
                    cycle: 0,
                    pc,
                    insn: 4,
                    len: 2,
                    next_pc: target_pc,
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
                    trap_valid: 0,
                    trap_cause: 0,
                    traparg0: 0,
                    block_kind: "sys".to_string(),
                    lane_id: "scalar0".to_string(),
                    tile_meta: String::new(),
                    tile_ref_src: 0,
                    tile_ref_dst: 0,
                },
                deps: [None, None],
                src_queue_kinds: [None, None],
                src_logical_tags: [None, None],
                src_qtags: [None, None],
                dst_queue_kind: None,
                dst_logical_tag: None,
                dst_qtag: None,
                bypass_d2: false,
                is_load: false,
                is_store: false,
                load_ordinal: None,
                load_store_id: None,
                miss_injected: false,
                redirect_target: Some(target_pc),
                phys_iq: None,
                pick_wakeup_visible: None,
                data_ready_visible: None,
                miss_pending_until: None,
                e1_cycle: None,
                e4_cycle: None,
                w1_cycle: None,
                done_cycle: None,
            },
            CycleUop {
                decoded: plain,
                commit: CommitRecord::unsupported(
                    0,
                    pc.wrapping_add(2),
                    enc_addi(2, 0, 1) as u64,
                    4,
                    &isa::BlockMeta::default(),
                ),
                deps: [None, None],
                src_queue_kinds: [None, None],
                src_logical_tags: [None, None],
                src_qtags: [None, None],
                dst_queue_kind: None,
                dst_logical_tag: None,
                dst_qtag: None,
                bypass_d2: false,
                is_load: false,
                is_store: false,
                load_ordinal: None,
                load_store_id: None,
                miss_injected: false,
                redirect_target: None,
                phys_iq: None,
                pick_wakeup_visible: None,
                data_ready_visible: None,
                miss_pending_until: None,
                e1_cycle: None,
                e4_cycle: None,
                w1_cycle: None,
                done_cycle: None,
            },
        ];
        let mut pipeline = StageQueues::default();
        let mut iq = Vec::new();
        let mut rob = VecDeque::new();
        pipeline.frontend[10].extend([0, 1]);
        pipeline.seq_checkpoint_ids.insert(0, 0);
        pipeline.seq_checkpoint_ids.insert(1, 0);

        dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

        assert_eq!(
            pipeline.seq_branch_contexts.get(&1).copied(),
            Some(BranchOwnerContext {
                kind: BranchOwnerKind::Cond,
                base_pc: pc,
                target_pc,
                off: target_pc.wrapping_sub(pc),
                pred_take: expected_pred_take,
                epoch: 1,
            })
        );
    }

    run_case(0x1000, 0x1100, false);
    run_case(0x1100, 0x1000, true);
}

#[test]
fn dispatch_maps_c_bstart_std_brtype_to_full_boundary_kind_taxonomy() {
    fn run_case(insn: u64, target_pc: u64, expected_kind: BranchOwnerKind) {
        let bstart = isa::decode_word(insn).expect("decode c.bstart.std variant");
        let plain = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
        let mut uops = vec![
            CycleUop {
                decoded: bstart,
                commit: CommitRecord {
                    schema_version: "1.0".to_string(),
                    cycle: 0,
                    pc: 0x1000,
                    insn,
                    len: 2,
                    next_pc: target_pc,
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
                    trap_valid: 0,
                    trap_cause: 0,
                    traparg0: 0,
                    block_kind: "sys".to_string(),
                    lane_id: "scalar0".to_string(),
                    tile_meta: String::new(),
                    tile_ref_src: 0,
                    tile_ref_dst: 0,
                },
                deps: [None, None],
                src_queue_kinds: [None, None],
                src_logical_tags: [None, None],
                src_qtags: [None, None],
                dst_queue_kind: None,
                dst_logical_tag: None,
                dst_qtag: None,
                bypass_d2: false,
                is_load: false,
                is_store: false,
                load_ordinal: None,
                load_store_id: None,
                miss_injected: false,
                redirect_target: Some(target_pc),
                phys_iq: None,
                pick_wakeup_visible: None,
                data_ready_visible: None,
                miss_pending_until: None,
                e1_cycle: None,
                e4_cycle: None,
                w1_cycle: None,
                done_cycle: None,
            },
            CycleUop {
                decoded: plain,
                commit: CommitRecord::unsupported(
                    0,
                    0x1002,
                    enc_addi(2, 0, 1) as u64,
                    4,
                    &isa::BlockMeta::default(),
                ),
                deps: [None, None],
                src_queue_kinds: [None, None],
                src_logical_tags: [None, None],
                src_qtags: [None, None],
                dst_queue_kind: None,
                dst_logical_tag: None,
                dst_qtag: None,
                bypass_d2: false,
                is_load: false,
                is_store: false,
                load_ordinal: None,
                load_store_id: None,
                miss_injected: false,
                redirect_target: None,
                phys_iq: None,
                pick_wakeup_visible: None,
                data_ready_visible: None,
                miss_pending_until: None,
                e1_cycle: None,
                e4_cycle: None,
                w1_cycle: None,
                done_cycle: None,
            },
        ];
        let mut pipeline = StageQueues::default();
        let mut iq = Vec::new();
        let mut rob = VecDeque::new();
        pipeline.frontend[10].extend([0, 1]);
        pipeline.seq_checkpoint_ids.insert(0, 0);
        pipeline.seq_checkpoint_ids.insert(1, 0);

        dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

        assert_eq!(
            pipeline.seq_branch_contexts.get(&1).copied(),
            Some(BranchOwnerContext {
                kind: expected_kind,
                base_pc: 0x1000,
                target_pc,
                off: target_pc.wrapping_sub(0x1000),
                pred_take: matches!(expected_kind, BranchOwnerKind::Cond) && target_pc < 0x1000,
                epoch: 1,
            })
        );
    }

    run_case(2048, 0x1010, BranchOwnerKind::Fall);
    run_case(4096, 0x1010, BranchOwnerKind::Direct);
    run_case(6144, 0x0ff0, BranchOwnerKind::Cond);
    run_case(8192, 0x1010, BranchOwnerKind::Call);
    run_case(10240, 0x1010, BranchOwnerKind::Ind);
    run_case(12288, 0x1010, BranchOwnerKind::ICall);
    run_case(14336, 0x1010, BranchOwnerKind::Ret);
}

#[test]
fn ret_start_marker_preserves_live_kind_before_target_setup() {
    let start = isa::decode_word(14336).expect("decode c.bstart.std ret");
    let plain = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let commits = vec![
        CommitRecord::unsupported(0, 0x1000, 14336, 2, &isa::BlockMeta::default()),
        CommitRecord::unsupported(
            0,
            0x1002,
            enc_addi(2, 0, 1) as u64,
            4,
            &isa::BlockMeta::default(),
        ),
    ];
    let decoded = vec![start, plain];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(pipeline.active_branch_context.kind, BranchOwnerKind::Ret);
    assert_eq!(
        pipeline.seq_branch_contexts.get(&1).copied(),
        Some(BranchOwnerContext {
            kind: BranchOwnerKind::Ret,
            base_pc: 0x1000,
            target_pc: 0,
            off: 0,
            pred_take: false,
            epoch: 1,
        })
    );
}

#[test]
fn ret_block_redirect_uses_live_setc_tgt_owner_not_row_surrogate() {
    let start = isa::decode_word(14336).expect("decode c.bstart.std ret");
    let setc_tgt_ra = isa::decode_word(0x029c).expect("decode c.setc.tgt ra");
    let bstop = isa::decode_word(0).expect("decode c.bstop");
    let target = isa::decode_word(2048).expect("decode target c.bstart.std");
    let commits = vec![
        CommitRecord::unsupported(0, 0x1000, 14336, 2, &isa::BlockMeta::default()),
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1002,
            insn: 0x029c,
            len: 2,
            next_pc: 0x1004,
            src0_valid: 1,
            src0_reg: 10,
            src0_data: 0x2000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 2,
            pc: 0x1004,
            insn: 0,
            len: 2,
            next_pc: 0x2000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord::unsupported(0, 0x2000, 2048, 2, &isa::BlockMeta::default()),
    ];
    let decoded = vec![start, setc_tgt_ra, bstop, target];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1, 2]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);
    pipeline.seq_checkpoint_ids.insert(2, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(
        pipeline.seq_dynamic_target_pcs.get(&1).copied(),
        Some(0x2000)
    );
    assert_eq!(
        pipeline.seq_boundary_target_pcs.get(&2).copied(),
        Some(0x2000)
    );
    assert_eq!(
        pipeline.seq_boundary_target_owner_seqs.get(&2).copied(),
        Some(1)
    );
    assert_eq!(
        pipeline.seq_branch_contexts.get(&2).copied(),
        Some(BranchOwnerContext {
            kind: BranchOwnerKind::Ret,
            base_pc: 0x1000,
            target_pc: 0x2000,
            off: 0x1000,
            pred_take: false,
            epoch: 1,
        })
    );

    pipeline.active_dynamic_target_pc = Some(0x3333);
    pipeline.active_dynamic_target_owner_seq = Some(99);
    uops[2].redirect_target = None;
    uops[2].w1_cycle = Some(7);

    schedule_frontend_redirect_recovery(7, &mut pipeline, &uops);

    assert_eq!(
        pipeline.frontend_redirect,
        Some(FrontendRedirectState {
            source_seq: 2,
            target_pc: 0x2000,
            restart_seq: 3,
            checkpoint_id: 0,
            from_correction: false,
            resume_cycle: 8,
        })
    );
}

#[test]
fn ret_block_missing_setc_tgt_raises_precise_dynamic_target_trap() {
    let start = isa::decode_word(14336).expect("decode c.bstart.std ret");
    let bstop = isa::decode_word(0).expect("decode c.bstop");
    let target = isa::decode_word(2048).expect("decode target c.bstart.std");
    let commits = vec![
        CommitRecord::unsupported(0, 0x1000, 14336, 2, &isa::BlockMeta::default()),
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1002,
            insn: 0,
            len: 2,
            next_pc: 0x2000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord::unsupported(0, 0x2000, 2048, 2, &isa::BlockMeta::default()),
    ];
    let decoded = vec![start, bstop, target];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);
    assert_eq!(pipeline.seq_dynamic_target_pcs.get(&1), None);

    uops[1].redirect_target = None;
    uops[1].w1_cycle = Some(7);

    publish_dynamic_boundary_target_fault_state(7, &mut pipeline, &uops);
    assert_eq!(
        pipeline.pending_trap,
        Some(PendingTrapState {
            seq: 1,
            cause: isa::TRAP_DYNAMIC_TARGET_MISSING,
            traparg0: 0x1002,
            checkpoint_id: 0,
            visible_cycle: 7,
        })
    );

    schedule_frontend_redirect_recovery(7, &mut pipeline, &uops);
    assert_eq!(pipeline.frontend_redirect, None);
}

#[test]
fn ret_block_non_block_target_raises_precise_dynamic_target_trap() {
    let start = isa::decode_word(14336).expect("decode c.bstart.std ret");
    let setc_tgt_ra = isa::decode_word(0x029c).expect("decode c.setc.tgt ra");
    let bstop = isa::decode_word(0).expect("decode c.bstop");
    let illegal_target = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let commits = vec![
        CommitRecord::unsupported(0, 0x1000, 14336, 2, &isa::BlockMeta::default()),
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1002,
            insn: 0x029c,
            len: 2,
            next_pc: 0x1004,
            src0_valid: 1,
            src0_reg: 10,
            src0_data: 0x2000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 2,
            pc: 0x1004,
            insn: 0,
            len: 2,
            next_pc: 0x2000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord::unsupported(
            0,
            0x2000,
            enc_addi(2, 0, 1) as u64,
            4,
            &isa::BlockMeta::default(),
        ),
    ];
    let decoded = vec![start, setc_tgt_ra, bstop, illegal_target];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1, 2]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);
    pipeline.seq_checkpoint_ids.insert(2, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);
    uops[2].redirect_target = None;
    uops[2].w1_cycle = Some(7);

    publish_dynamic_boundary_target_fault_state(7, &mut pipeline, &uops);
    assert_eq!(
        pipeline.pending_trap,
        Some(PendingTrapState {
            seq: 2,
            cause: isa::TRAP_DYNAMIC_TARGET_NOT_BSTART,
            traparg0: 0x1004,
            checkpoint_id: 0,
            visible_cycle: 7,
        })
    );

    schedule_frontend_redirect_recovery(7, &mut pipeline, &uops);
    assert_eq!(pipeline.frontend_redirect, None);
}

#[test]
fn ret_block_stale_setc_tgt_epoch_raises_precise_dynamic_target_trap() {
    let bstop = isa::decode_word(0).expect("decode c.bstop");
    let commits = vec![CommitRecord::unsupported(
        0,
        0x1004,
        0,
        2,
        &isa::BlockMeta::default(),
    )];
    let mut uops = build_uops(&commits, &[bstop]);
    uops[0].redirect_target = Some(0x2000);
    uops[0].w1_cycle = Some(7);

    let mut pipeline = StageQueues::default();
    pipeline.seq_branch_contexts.insert(
        0,
        BranchOwnerContext {
            kind: BranchOwnerKind::Ret,
            base_pc: 0x1004,
            target_pc: 0x2000,
            off: 0x0ffc,
            pred_take: false,
            epoch: 2,
        },
    );
    pipeline.seq_recovery_epochs.insert(0, 2);
    pipeline.seq_boundary_target_pcs.insert(0, 0x2000);
    pipeline.seq_boundary_target_owner_seqs.insert(0, 3);
    pipeline
        .seq_boundary_target_producer_kinds
        .insert(0, ReturnConsumerKind::SetcTgt);
    pipeline.seq_boundary_target_setup_epochs.insert(0, 1);
    pipeline.seq_boundary_target_source_owner_seqs.insert(0, 3);
    pipeline.seq_boundary_target_source_epochs.insert(0, 1);
    pipeline
        .seq_boundary_target_source_kinds
        .insert(0, DynamicTargetSourceKind::ArchTargetSetup);

    publish_dynamic_boundary_target_fault_state(7, &mut pipeline, &uops);
    assert_eq!(
        pipeline.pending_trap,
        Some(PendingTrapState {
            seq: 0,
            cause: isa::TRAP_DYNAMIC_TARGET_STALE,
            traparg0: 0x1004,
            checkpoint_id: 0,
            visible_cycle: 7,
        })
    );
}

#[test]
fn fused_bstart_call_materializes_return_target_without_setret() {
    let call_header = isa::decode_word(1_589_249).expect("decode bstart.std call header");
    let target = isa::decode_word(2048).expect("decode c.bstart.std");
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 1_589_249,
            len: 4,
            next_pc: 0x3000,
            src0_valid: 0,
            src0_reg: 0,
            src0_data: 0,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 10,
            dst_data: 0x2000,
            wb_valid: 1,
            wb_rd: 10,
            wb_data: 0x2000,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord::unsupported(0, 0x2000, 2048, 2, &isa::BlockMeta::default()),
    ];
    let decoded = vec![call_header, target];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(
        pipeline.seq_call_return_target_pcs.get(&0).copied(),
        Some(0x2000)
    );
    assert_eq!(
        pipeline.seq_call_return_target_owner_seqs.get(&0).copied(),
        Some(0)
    );
    assert_eq!(pipeline.active_call_header_seq, None);
    assert_eq!(pipeline.active_call_return_target_pc, Some(0x2000));
    assert_eq!(pipeline.active_call_return_target_owner_seq, Some(0));
    assert_eq!(
        pipeline.active_call_return_materialization_kind,
        Some(CallMaterializationKind::FusedCall)
    );
    assert!(pipeline.seq_call_header_faults.is_empty());
}

#[test]
fn call_header_without_setret_stays_non_returning() {
    let call_header = isa::decode_word(1_589_249).expect("decode bstart.std call header");
    let plain = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 1_589_249,
            len: 4,
            next_pc: 0x3000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1004,
            insn: enc_addi(2, 0, 1) as u64,
            len: 4,
            next_pc: 0x1008,
            src0_valid: 0,
            src0_reg: 0,
            src0_data: 0,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 2,
            dst_data: 1,
            wb_valid: 1,
            wb_rd: 2,
            wb_data: 1,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
    ];
    let decoded = vec![call_header, plain];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(pipeline.seq_call_return_target_pcs.get(&0), None);
    assert_eq!(pipeline.seq_call_return_target_owner_seqs.get(&0), None);
    assert_eq!(pipeline.active_call_header_seq, None);
    assert_eq!(pipeline.active_call_return_target_pc, None);
    assert_eq!(pipeline.active_call_return_target_owner_seq, None);
    assert!(pipeline.seq_call_header_faults.is_empty());
}

#[test]
fn adjacent_setret_materializes_call_header_owner_seq() {
    let call_header = isa::decode_word(1_589_249).expect("decode bstart.std call header");
    let setret = isa::decode_word(0x5056).expect("decode c.setret");
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 1_589_249,
            len: 4,
            next_pc: 0x3000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1004,
            insn: 0x5056,
            len: 2,
            next_pc: 0x1006,
            src0_valid: 0,
            src0_reg: 0,
            src0_data: 0,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 10,
            dst_data: 0x2000,
            wb_valid: 1,
            wb_rd: 10,
            wb_data: 0x2000,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
    ];
    let decoded = vec![call_header, setret];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(
        pipeline.seq_call_return_target_pcs.get(&0).copied(),
        Some(0x2000)
    );
    assert_eq!(
        pipeline.seq_call_return_target_owner_seqs.get(&0).copied(),
        Some(1)
    );
    assert_eq!(
        pipeline.seq_call_return_target_owner_seqs.get(&1).copied(),
        Some(1)
    );
    assert_eq!(pipeline.active_call_return_target_pc, Some(0x2000));
    assert_eq!(pipeline.active_call_return_target_owner_seq, Some(1));
    assert_eq!(
        pipeline.active_call_return_materialization_kind,
        Some(CallMaterializationKind::AdjacentSetret)
    );
    assert!(pipeline.seq_call_header_faults.is_empty());
}

#[test]
fn fret_ra_inherits_call_return_source_owner_and_materialization_kind() {
    let decoded = isa::decode_word(346369857).expect("decode fret.ra");
    let commits = vec![CommitRecord::unsupported(
        0,
        0x1000,
        346369857,
        4,
        &isa::BlockMeta::default(),
    )];
    let mut uops = build_uops(&commits, &[decoded]);
    uops[0].redirect_target = Some(0x2000);

    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].push_back(0);
    pipeline.active_call_return_target_pc = Some(0x2000);
    pipeline.active_call_return_target_owner_seq = Some(7);
    pipeline.active_call_return_materialization_kind = Some(CallMaterializationKind::FusedCall);
    pipeline.active_branch_context = BranchOwnerContext {
        kind: BranchOwnerKind::Ret,
        base_pc: 0x1000,
        target_pc: 0x2000,
        off: 0x1000,
        pred_take: false,
        epoch: 1,
    };

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(
        pipeline.seq_boundary_target_pcs.get(&0).copied(),
        Some(0x2000)
    );
    assert_eq!(
        pipeline.seq_boundary_target_owner_seqs.get(&0).copied(),
        Some(7)
    );
    assert_eq!(
        pipeline.seq_return_consumer_kinds.get(&0).copied(),
        Some(ReturnConsumerKind::FretRa)
    );
    assert_eq!(
        pipeline.seq_call_materialization_kinds.get(&0).copied(),
        Some(CallMaterializationKind::FusedCall)
    );
    assert_eq!(
        pipeline.seq_boundary_target_source_kinds.get(&0).copied(),
        Some(DynamicTargetSourceKind::CallReturnFused)
    );
}

#[test]
fn non_adjacent_setret_raises_precise_call_header_trap() {
    let call_header = isa::decode_word(1_589_249).expect("decode bstart.std call header");
    let plain = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let setret = isa::decode_word(0x5056).expect("decode c.setret");
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 1_589_249,
            len: 4,
            next_pc: 0x3000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1004,
            insn: enc_addi(2, 0, 1) as u64,
            len: 4,
            next_pc: 0x1008,
            src0_valid: 0,
            src0_reg: 0,
            src0_data: 0,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 2,
            dst_data: 1,
            wb_valid: 1,
            wb_rd: 2,
            wb_data: 1,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 2,
            pc: 0x1008,
            insn: 0x5056,
            len: 2,
            next_pc: 0x100a,
            src0_valid: 0,
            src0_reg: 0,
            src0_data: 0,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 10,
            dst_data: 0x2000,
            wb_valid: 1,
            wb_rd: 10,
            wb_data: 0x2000,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
    ];
    let decoded = vec![call_header, plain, setret];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1, 2]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);
    pipeline.seq_checkpoint_ids.insert(2, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);
    assert_eq!(
        pipeline.seq_call_header_faults.get(&2).copied(),
        Some(isa::TRAP_SETRET_NOT_ADJACENT)
    );

    uops[2].w1_cycle = Some(7);
    publish_call_header_fault_state(7, &mut pipeline, &uops);

    assert_eq!(
        pipeline.pending_trap,
        Some(PendingTrapState {
            seq: 2,
            cause: isa::TRAP_SETRET_NOT_ADJACENT,
            traparg0: 0x1008,
            checkpoint_id: 0,
            visible_cycle: 7,
        })
    );
}

#[test]
fn call_header_fault_flush_emits_attempted_owner_row_id() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let call_header = isa::decode_word(1_589_249).expect("decode bstart.std call header");
    let plain = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let setret = isa::decode_word(0x5056).expect("decode c.setret");
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 1_589_249,
            len: 4,
            next_pc: 0x3000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1004,
            insn: enc_addi(2, 0, 1) as u64,
            len: 4,
            next_pc: 0x1008,
            src0_valid: 0,
            src0_reg: 0,
            src0_data: 0,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 2,
            dst_data: 1,
            wb_valid: 1,
            wb_rd: 2,
            wb_data: 1,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 2,
            pc: 0x1008,
            insn: 0x5056,
            len: 2,
            next_pc: 0x100a,
            src0_valid: 0,
            src0_reg: 0,
            src0_data: 0,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 10,
            dst_data: 0x2000,
            wb_valid: 1,
            wb_rd: 10,
            wb_data: 0x2000,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
    ];
    let decoded = vec![call_header, plain, setret];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1, 2]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);
    pipeline.seq_checkpoint_ids.insert(2, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);
    uops[2].w1_cycle = Some(7);
    publish_call_header_fault_state(7, &mut pipeline, &uops);

    let mut out = Vec::new();
    emit_stage_events(7, &runtime, &pipeline, &iq, &rob, &uops, &mut out);

    assert!(out.iter().any(|event| {
        event.stage_id == "FLS"
            && event.row_id == "uop2"
            && event.cause == "call_header_fault"
            && event.trap_cause == Some(isa::TRAP_SETRET_NOT_ADJACENT)
            && event.traparg0 == Some(0x1008)
            && event.branch_kind.as_deref() == Some("call")
            && event.target_owner_row_id.as_deref() == Some("uop2")
            && event.call_materialization_kind.as_deref() == Some("adjacent_setret")
    }));
}

#[test]
fn redirecting_fused_call_emits_call_materialization_kind_on_fls() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(1_589_249).expect("decode bstart.std call header");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 1_589_249,
            len: 4,
            next_pc: 0x3000,
            src0_valid: 0,
            src0_reg: 0,
            src0_data: 0,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 10,
            dst_data: 0x2000,
            wb_valid: 1,
            wb_rd: 10,
            wb_data: 0x2000,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: false,
        load_ordinal: None,
        load_store_id: None,
        miss_injected: false,
        redirect_target: Some(0x3000),
        phys_iq: Some(PhysIq::CmdIq),
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: Some(7),
        done_cycle: None,
    }];
    let mut pipeline = StageQueues::default();
    pipeline
        .seq_call_materialization_kinds
        .insert(0, CallMaterializationKind::FusedCall);

    let mut out = Vec::new();
    emit_stage_events(
        7,
        &runtime,
        &pipeline,
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );

    assert!(out.iter().any(|event| {
        event.stage_id == "FLS"
            && event.row_id == "uop0"
            && event.branch_kind.as_deref() == Some("call")
            && event.call_materialization_kind.as_deref() == Some("fused_call")
    }));
}

#[test]
fn redirecting_control_emits_flush_stage() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(2048).expect("decode c.bstart.std");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 2048,
            len: 2,
            next_pc: 0x1010,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: false,
        load_ordinal: None,
        load_store_id: None,
        miss_injected: false,
        redirect_target: Some(0x1010),
        phys_iq: Some(PhysIq::CmdIq),
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: Some(7),
        done_cycle: None,
    }];
    let mut out = Vec::new();

    emit_stage_events(
        7,
        &runtime,
        &StageQueues::default(),
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );
    assert!(out.iter().any(|event| {
        event.stage_id == "FLS"
            && event.cause == "redirect_boundary"
            && event.branch_kind.as_deref() == Some("fall")
    }));
}

#[test]
fn ret_dynamic_target_fault_emits_precise_flush_cause_and_return_kind() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(346370113).expect("decode fret.stk");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord::unsupported(0, 0x1000, 346370113, 4, &isa::BlockMeta::default()),
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: false,
        load_ordinal: None,
        load_store_id: None,
        miss_injected: false,
        redirect_target: None,
        phys_iq: None,
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: Some(7),
        done_cycle: None,
    }];
    let mut pipeline = StageQueues::default();
    pipeline.seq_branch_contexts.insert(
        0,
        BranchOwnerContext {
            kind: BranchOwnerKind::Ret,
            base_pc: 0x1000,
            target_pc: 0x2000,
            off: 0x1000,
            pred_take: false,
            epoch: 1,
        },
    );
    pipeline
        .seq_return_consumer_kinds
        .insert(0, ReturnConsumerKind::FretStk);
    pipeline.pending_trap = Some(PendingTrapState {
        seq: 0,
        cause: isa::TRAP_DYNAMIC_TARGET_MISSING,
        traparg0: 0x1000,
        checkpoint_id: 3,
        visible_cycle: 7,
    });

    let mut out = Vec::new();
    emit_stage_events(
        7,
        &runtime,
        &pipeline,
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );

    assert!(out.iter().any(|event| {
        event.stage_id == "FLS"
            && event.row_id == "uop0"
            && event.cause == "dynamic_target_missing"
            && event.trap_cause == Some(isa::TRAP_DYNAMIC_TARGET_MISSING)
            && event.traparg0 == Some(0x1000)
            && event.branch_kind.as_deref() == Some("ret")
            && event.return_kind.as_deref() == Some("fret_stk")
    }));
}

#[test]
fn ret_fault_emits_call_materialization_kind_from_live_return_source() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(346369857).expect("decode fret.ra");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord::unsupported(0, 0x1000, 346369857, 4, &isa::BlockMeta::default()),
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: false,
        load_ordinal: None,
        load_store_id: None,
        miss_injected: false,
        redirect_target: Some(0x2000),
        phys_iq: None,
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: Some(7),
        done_cycle: None,
    }];
    let mut pipeline = StageQueues::default();
    pipeline.seq_branch_contexts.insert(
        0,
        BranchOwnerContext {
            kind: BranchOwnerKind::Ret,
            base_pc: 0x1000,
            target_pc: 0x2000,
            off: 0x1000,
            pred_take: false,
            epoch: 1,
        },
    );
    pipeline
        .seq_return_consumer_kinds
        .insert(0, ReturnConsumerKind::FretRa);
    pipeline
        .seq_call_materialization_kinds
        .insert(0, CallMaterializationKind::AdjacentSetret);
    pipeline
        .seq_boundary_target_source_kinds
        .insert(0, DynamicTargetSourceKind::CallReturnAdjacentSetret);
    pipeline.pending_trap = Some(PendingTrapState {
        seq: 0,
        cause: isa::TRAP_DYNAMIC_TARGET_MISSING,
        traparg0: 0x1000,
        checkpoint_id: 3,
        visible_cycle: 7,
    });

    let mut out = Vec::new();
    emit_stage_events(
        7,
        &runtime,
        &pipeline,
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );

    assert!(out.iter().any(|event| {
        event.stage_id == "FLS"
            && event.row_id == "uop0"
            && event.cause == "dynamic_target_missing"
            && event.branch_kind.as_deref() == Some("ret")
            && event.return_kind.as_deref() == Some("fret_ra")
            && event.call_materialization_kind.as_deref() == Some("adjacent_setret")
            && event.target_source_kind.as_deref() == Some("call_return_adjacent_setret")
    }));
}

#[test]
fn ret_fault_emits_stale_dynamic_target_cause_and_setup_provenance() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(0).expect("decode c.bstop");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord::unsupported(0, 0x1004, 0, 2, &isa::BlockMeta::default()),
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: false,
        load_ordinal: None,
        load_store_id: None,
        miss_injected: false,
        redirect_target: Some(0x2000),
        phys_iq: None,
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: Some(7),
        done_cycle: None,
    }];
    let mut pipeline = StageQueues::default();
    pipeline.seq_branch_contexts.insert(
        0,
        BranchOwnerContext {
            kind: BranchOwnerKind::Ret,
            base_pc: 0x1004,
            target_pc: 0x2000,
            off: 0x0ffc,
            pred_take: false,
            epoch: 2,
        },
    );
    pipeline.seq_recovery_epochs.insert(0, 2);
    pipeline.seq_boundary_target_owner_seqs.insert(0, 3);
    pipeline
        .seq_boundary_target_producer_kinds
        .insert(0, ReturnConsumerKind::SetcTgt);
    pipeline.seq_boundary_target_setup_epochs.insert(0, 1);
    pipeline.seq_boundary_target_source_owner_seqs.insert(0, 3);
    pipeline.seq_boundary_target_source_epochs.insert(0, 1);
    pipeline
        .seq_boundary_target_source_kinds
        .insert(0, DynamicTargetSourceKind::ArchTargetSetup);
    pipeline.pending_trap = Some(PendingTrapState {
        seq: 0,
        cause: isa::TRAP_DYNAMIC_TARGET_STALE,
        traparg0: 0x1004,
        checkpoint_id: 0,
        visible_cycle: 7,
    });

    let mut out = Vec::new();
    emit_stage_events(
        7,
        &runtime,
        &pipeline,
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );

    let event = out
        .iter()
        .find(|event| {
            event.stage_id == "FLS"
                && event.row_id == "uop0"
                && event.cause == "dynamic_target_stale_setup"
        })
        .expect("missing stale setup FLS event");
    assert_eq!(event.target_owner_row_id.as_deref(), Some("uop3"));
    assert_eq!(event.target_producer_kind.as_deref(), Some("setc_tgt"));
    assert_eq!(event.branch_kind.as_deref(), Some("ret"));
    assert_eq!(event.target_setup_epoch, Some(1));
    assert_eq!(event.boundary_epoch, Some(2));
    assert_eq!(event.target_source_owner_row_id.as_deref(), Some("uop3"));
    assert_eq!(event.target_source_epoch, Some(1));
    assert_eq!(
        event.target_source_kind.as_deref(),
        Some("arch_target_setup")
    );
}

#[test]
fn ind_fault_emits_stale_return_dynamic_target_cause_and_provenance() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(0).expect("decode c.bstop");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord::unsupported(0, 0x1006, 0, 2, &isa::BlockMeta::default()),
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: false,
        load_ordinal: None,
        load_store_id: None,
        miss_injected: false,
        redirect_target: Some(0x2000),
        phys_iq: None,
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: Some(7),
        done_cycle: None,
    }];
    let mut pipeline = StageQueues::default();
    pipeline.seq_branch_contexts.insert(
        0,
        BranchOwnerContext {
            kind: BranchOwnerKind::Ind,
            base_pc: 0x1006,
            target_pc: 0x2000,
            off: 0x0ffa,
            pred_take: false,
            epoch: 2,
        },
    );
    pipeline.seq_recovery_epochs.insert(0, 2);
    pipeline.seq_boundary_target_owner_seqs.insert(0, 4);
    pipeline
        .seq_boundary_target_producer_kinds
        .insert(0, ReturnConsumerKind::SetcTgt);
    pipeline.seq_boundary_target_setup_epochs.insert(0, 1);
    pipeline.seq_boundary_target_source_owner_seqs.insert(0, 9);
    pipeline.seq_boundary_target_source_epochs.insert(0, 0);
    pipeline
        .seq_boundary_target_source_kinds
        .insert(0, DynamicTargetSourceKind::CallReturnFused);
    pipeline
        .seq_call_materialization_kinds
        .insert(0, CallMaterializationKind::FusedCall);
    pipeline.pending_trap = Some(PendingTrapState {
        seq: 0,
        cause: isa::TRAP_DYNAMIC_TARGET_STALE,
        traparg0: 0x1006,
        checkpoint_id: 0,
        visible_cycle: 7,
    });

    let mut out = Vec::new();
    emit_stage_events(
        7,
        &runtime,
        &pipeline,
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );

    assert!(out.iter().any(|event| {
        event.stage_id == "FLS"
            && event.row_id == "uop0"
            && event.cause == "dynamic_target_stale_return"
            && event.target_owner_row_id.as_deref() == Some("uop4")
            && event.target_producer_kind.as_deref() == Some("setc_tgt")
            && event.branch_kind.as_deref() == Some("ind")
            && event.target_setup_epoch == Some(1)
            && event.boundary_epoch == Some(2)
            && event.target_source_owner_row_id.as_deref() == Some("uop9")
            && event.target_source_epoch == Some(0)
            && event.call_materialization_kind.as_deref() == Some("fused_call")
            && event.target_source_kind.as_deref() == Some("call_return_fused")
    }));
}

#[test]
fn ret_boundary_flush_emits_dynamic_target_owner_row_id() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(0).expect("decode c.bstop");
    let uops = vec![
        CycleUop {
            decoded: isa::decode_word(0x029c).expect("decode c.setc.tgt ra"),
            commit: CommitRecord::unsupported(0, 0x1000, 0x029c, 2, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(6),
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord::unsupported(0, 0x1002, 0, 2, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x2000),
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(7),
            done_cycle: None,
        },
    ];
    let mut pipeline = StageQueues::default();
    pipeline.seq_branch_contexts.insert(
        1,
        BranchOwnerContext {
            kind: BranchOwnerKind::Ret,
            base_pc: 0x1002,
            target_pc: 0x2000,
            off: 0x0ffe,
            pred_take: false,
            epoch: 1,
        },
    );
    pipeline.seq_boundary_target_pcs.insert(1, 0x2000);
    pipeline.seq_boundary_target_owner_seqs.insert(1, 0);
    pipeline
        .seq_boundary_target_source_kinds
        .insert(1, DynamicTargetSourceKind::ArchTargetSetup);
    pipeline
        .seq_return_consumer_kinds
        .insert(1, ReturnConsumerKind::SetcTgt);

    let mut out = Vec::new();
    emit_stage_events(
        7,
        &runtime,
        &pipeline,
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );

    assert!(out.iter().any(|event| {
        event.stage_id == "FLS"
            && event.row_id == "uop1"
            && event.cause == "redirect_boundary"
            && event.target_owner_row_id.as_deref() == Some("uop0")
            && event.branch_kind.as_deref() == Some("ret")
            && event.return_kind.as_deref() == Some("setc_tgt")
            && event.target_source_kind.as_deref() == Some("arch_target_setup")
    }));
}

#[test]
fn ind_boundary_preserves_call_return_target_source_kind_via_setc_tgt() {
    let call_header = isa::decode_word(1_589_249).expect("decode bstart.std call header");
    let setc_tgt = isa::decode_word(0x029c).expect("decode c.setc.tgt ra");
    let bstop = isa::decode_word(0).expect("decode c.bstop");
    let commits = vec![
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 0,
            pc: 0x1000,
            insn: 1_589_249,
            len: 4,
            next_pc: 0x3000,
            src0_valid: 0,
            src0_reg: 0,
            src0_data: 0,
            src1_valid: 0,
            src1_reg: 0,
            src1_data: 0,
            dst_valid: 1,
            dst_reg: 10,
            dst_data: 0x2000,
            wb_valid: 1,
            wb_rd: 10,
            wb_data: 0x2000,
            mem_valid: 0,
            mem_is_store: 0,
            mem_addr: 0,
            mem_wdata: 0,
            mem_rdata: 0,
            mem_size: 0,
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord {
            schema_version: "1.0".to_string(),
            cycle: 1,
            pc: 0x1004,
            insn: 0x029c,
            len: 2,
            next_pc: 0x1006,
            src0_valid: 1,
            src0_reg: 10,
            src0_data: 0x2000,
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
            trap_valid: 0,
            trap_cause: 0,
            traparg0: 0,
            block_kind: "sys".to_string(),
            lane_id: "scalar0".to_string(),
            tile_meta: String::new(),
            tile_ref_src: 0,
            tile_ref_dst: 0,
        },
        CommitRecord::unsupported(2, 0x1006, 0, 2, &isa::BlockMeta::default()),
    ];
    let decoded = vec![call_header, setc_tgt, bstop];
    let mut uops = build_uops(&commits, &decoded);
    let mut pipeline = StageQueues::default();
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    pipeline.frontend[10].extend([0, 1]);
    pipeline.seq_checkpoint_ids.insert(0, 0);
    pipeline.seq_checkpoint_ids.insert(1, 0);
    pipeline.seq_checkpoint_ids.insert(2, 0);

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);
    pipeline.frontend[10].push_back(2);
    pipeline.active_branch_context = BranchOwnerContext {
        kind: BranchOwnerKind::Ind,
        base_pc: 0x1006,
        target_pc: 0x2000,
        off: 0x0ffa,
        pred_take: false,
        epoch: 1,
    };
    dispatch_to_iq_and_bypass(1, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(
        pipeline.seq_boundary_target_owner_seqs.get(&2).copied(),
        Some(1)
    );
    assert_eq!(
        pipeline.seq_boundary_target_producer_kinds.get(&2).copied(),
        Some(ReturnConsumerKind::SetcTgt)
    );
    assert_eq!(
        pipeline.seq_boundary_target_source_kinds.get(&2).copied(),
        Some(DynamicTargetSourceKind::CallReturnFused)
    );
    assert_eq!(
        pipeline
            .seq_boundary_target_source_owner_seqs
            .get(&2)
            .copied(),
        Some(0)
    );
    assert_eq!(
        pipeline.seq_boundary_target_source_epochs.get(&2).copied(),
        Some(0)
    );
    assert_eq!(
        pipeline.seq_call_materialization_kinds.get(&2).copied(),
        Some(CallMaterializationKind::FusedCall)
    );
}

#[test]
fn ind_boundary_flush_emits_call_materialization_kind_from_dynamic_target_source() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(0).expect("decode c.bstop");
    let uops = vec![
        CycleUop {
            decoded: isa::decode_word(0x029c).expect("decode c.setc.tgt ra"),
            commit: CommitRecord::unsupported(0, 0x1000, 0x029c, 2, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(6),
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord::unsupported(0, 0x1002, 0, 2, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x2000),
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(7),
            done_cycle: None,
        },
    ];
    let mut pipeline = StageQueues::default();
    pipeline.seq_branch_contexts.insert(
        1,
        BranchOwnerContext {
            kind: BranchOwnerKind::Ind,
            base_pc: 0x1002,
            target_pc: 0x2000,
            off: 0x0ffe,
            pred_take: false,
            epoch: 1,
        },
    );
    pipeline.seq_boundary_target_pcs.insert(1, 0x2000);
    pipeline.seq_boundary_target_owner_seqs.insert(1, 0);
    pipeline
        .seq_boundary_target_producer_kinds
        .insert(1, ReturnConsumerKind::SetcTgt);
    pipeline.seq_boundary_target_source_owner_seqs.insert(1, 0);
    pipeline.seq_boundary_target_source_epochs.insert(1, 0);
    pipeline
        .seq_boundary_target_source_kinds
        .insert(1, DynamicTargetSourceKind::CallReturnFused);
    pipeline
        .seq_call_materialization_kinds
        .insert(1, CallMaterializationKind::FusedCall);

    let mut out = Vec::new();
    emit_stage_events(
        7,
        &runtime,
        &pipeline,
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );

    assert!(out.iter().any(|event| {
        event.stage_id == "FLS"
            && event.row_id == "uop1"
            && event.cause == "redirect_boundary"
            && event.target_owner_row_id.as_deref() == Some("uop0")
            && event.target_producer_kind.as_deref() == Some("setc_tgt")
            && event.branch_kind.as_deref() == Some("ind")
            && event.target_source_owner_row_id.as_deref() == Some("uop0")
            && event.target_source_epoch == Some(0)
            && event.call_materialization_kind.as_deref() == Some("fused_call")
            && event.target_source_kind.as_deref() == Some("call_return_fused")
    }));
}

#[test]
fn retire_emits_live_branch_kind_on_cmt_event() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(30_478_677).expect("decode cmp.nei");
    let mut uops = vec![CycleUop {
        decoded,
        commit: CommitRecord::unsupported(0, 0x1004, 30_478_677, 4, &isa::BlockMeta::default()),
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: false,
        load_ordinal: None,
        load_store_id: None,
        miss_injected: false,
        redirect_target: None,
        phys_iq: None,
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: Some(3),
        done_cycle: Some(3),
    }];
    let mut pipeline = StageQueues::default();
    let mut rob = VecDeque::from([0usize]);
    let mut committed = Vec::new();
    let mut retired = Vec::new();
    let mut stage_events = Vec::new();
    pipeline.seq_branch_contexts.insert(
        0,
        BranchOwnerContext {
            kind: BranchOwnerKind::Cond,
            base_pc: 0x1000,
            target_pc: 0x0ff0,
            off: 0xfffffffffffffff0,
            pred_take: true,
            epoch: 1,
        },
    );

    let trap = retire_ready(
        3,
        &runtime,
        &mut rob,
        &mut committed,
        &mut retired,
        &mut pipeline,
        &mut uops,
        &mut stage_events,
    );

    assert_eq!(trap, None);
    assert_eq!(committed.len(), 1);
    assert_eq!(stage_events.len(), 1);
    assert_eq!(stage_events[0].stage_id, "CMT");
    assert_eq!(stage_events[0].branch_kind.as_deref(), Some("cond"));
}

#[test]
fn retire_emits_distinct_return_kind_on_cmt_event() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let fret_ra = isa::DecodedInstruction {
        uid: "test_fret_ra".to_string(),
        mnemonic: "FRET.RA".to_string(),
        asm: "FRET.RA [x10], sp!, 16".to_string(),
        group: "Block Split".to_string(),
        encoding_kind: "L32".to_string(),
        length_bits: 32,
        mask: 0,
        match_bits: 0,
        instruction_bits: 0,
        uop_group: "CMD".to_string(),
        fields: Vec::new(),
    };
    let fret_stk = isa::decode_word(346370113).expect("decode fret.stk");
    let mut uops = vec![
        CycleUop {
            decoded: fret_ra,
            commit: CommitRecord::unsupported(0, 0x1000, 0xfeed0001, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x2000),
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(3),
            done_cycle: Some(3),
        },
        CycleUop {
            decoded: fret_stk,
            commit: CommitRecord::unsupported(0, 0x1004, 346370113, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x3000),
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(3),
            done_cycle: Some(3),
        },
    ];
    let mut pipeline = StageQueues::default();
    let mut rob = VecDeque::from([0usize, 1usize]);
    let mut committed = Vec::new();
    let mut retired = Vec::new();
    let mut stage_events = Vec::new();
    pipeline.seq_branch_contexts.insert(
        0,
        BranchOwnerContext {
            kind: BranchOwnerKind::Ret,
            base_pc: 0x1000,
            target_pc: 0x2000,
            off: 0x1000,
            pred_take: false,
            epoch: 1,
        },
    );
    pipeline.seq_branch_contexts.insert(
        1,
        BranchOwnerContext {
            kind: BranchOwnerKind::Ret,
            base_pc: 0x1004,
            target_pc: 0x3000,
            off: 0x1ffc,
            pred_take: false,
            epoch: 1,
        },
    );
    pipeline
        .seq_return_consumer_kinds
        .insert(0, ReturnConsumerKind::FretRa);
    pipeline
        .seq_return_consumer_kinds
        .insert(1, ReturnConsumerKind::FretStk);

    let trap = retire_ready(
        3,
        &runtime,
        &mut rob,
        &mut committed,
        &mut retired,
        &mut pipeline,
        &mut uops,
        &mut stage_events,
    );

    assert_eq!(trap, None);
    assert_eq!(stage_events.len(), 2);
    assert_eq!(stage_events[0].return_kind.as_deref(), Some("fret_ra"));
    assert_eq!(stage_events[1].return_kind.as_deref(), Some("fret_stk"));
    assert_eq!(stage_events[0].target_owner_row_id.as_deref(), Some("uop0"));
    assert_eq!(stage_events[1].target_owner_row_id.as_deref(), Some("uop1"));
}

#[test]
fn retire_emits_call_header_target_owner_row_id_on_cmt_event() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let call_header = isa::decode_word(1_589_249).expect("decode bstart.std call header");
    let setret = isa::decode_word(0x5056).expect("decode c.setret");
    let mut uops = vec![
        CycleUop {
            decoded: call_header,
            commit: CommitRecord::unsupported(0, 0x1000, 1_589_249, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x3000),
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(3),
            done_cycle: Some(3),
        },
        CycleUop {
            decoded: setret,
            commit: CommitRecord::unsupported(0, 0x1004, 0x5056, 2, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(3),
            done_cycle: Some(3),
        },
    ];
    let mut pipeline = StageQueues::default();
    pipeline.seq_branch_contexts.insert(
        0,
        BranchOwnerContext {
            kind: BranchOwnerKind::Call,
            base_pc: 0x1000,
            target_pc: 0x3000,
            off: 0x2000,
            pred_take: false,
            epoch: 1,
        },
    );
    pipeline.seq_call_return_target_owner_seqs.insert(0, 1);
    pipeline.seq_call_return_target_owner_seqs.insert(1, 1);
    pipeline
        .seq_call_materialization_kinds
        .insert(0, CallMaterializationKind::AdjacentSetret);
    pipeline
        .seq_call_materialization_kinds
        .insert(1, CallMaterializationKind::AdjacentSetret);
    let mut rob = VecDeque::from([0usize, 1usize]);
    let mut committed = Vec::new();
    let mut retired = Vec::new();
    let mut stage_events = Vec::new();

    let trap = retire_ready(
        3,
        &runtime,
        &mut rob,
        &mut committed,
        &mut retired,
        &mut pipeline,
        &mut uops,
        &mut stage_events,
    );

    assert_eq!(trap, None);
    assert_eq!(stage_events.len(), 2);
    assert_eq!(stage_events[0].branch_kind.as_deref(), Some("call"));
    assert_eq!(stage_events[0].target_owner_row_id.as_deref(), Some("uop1"));
    assert_eq!(stage_events[1].target_owner_row_id.as_deref(), Some("uop1"));
    assert_eq!(
        stage_events[0].call_materialization_kind.as_deref(),
        Some("adjacent_setret")
    );
    assert_eq!(
        stage_events[1].call_materialization_kind.as_deref(),
        Some("adjacent_setret")
    );
}

#[test]
fn cycle_limit_reports_requested_cycle_budget() {
    let program = vec![
        enc_addi(2, 0, 1),
        enc_addi(3, 2, 2),
        enc_addi(4, 3, 3),
        enc_addi(5, 4, 4),
        enc_addi(6, 5, 5),
        enc_addi(7, 6, 6),
        enc_addi(8, 7, 7),
        enc_addi(9, 8, 8),
        enc_addi(10, 9, 9),
        enc_addi(11, 10, 10),
        enc_addi(12, 11, 11),
        enc_acrc(1),
    ];
    let runtime = sample_runtime(&program, &[]);
    let bundle = CycleEngine
        .run(
            &runtime,
            &CycleRunOptions {
                max_cycles: 4,
                ..CycleRunOptions::default()
            },
        )
        .unwrap();

    assert_eq!(bundle.result.metrics.exit_reason, "cycle_limit");
    assert_eq!(bundle.result.metrics.cycles, 4);
}

#[test]
fn advance_frontend_preserves_overflowing_uops() {
    let mut pipeline = StageQueues::default();
    pipeline.frontend[8].extend([10, 11, 12, 13, 14]);
    pipeline.frontend[9].extend([20, 21, 22]);
    let mut rob = VecDeque::new();

    advance_frontend(&mut pipeline, &mut rob);

    assert_eq!(
        pipeline.frontend[10].iter().copied().collect::<Vec<_>>(),
        vec![20, 21, 22]
    );
    assert_eq!(
        pipeline.frontend[9].iter().copied().collect::<Vec<_>>(),
        vec![10, 11, 12, 13]
    );
    assert_eq!(
        pipeline.frontend[8].iter().copied().collect::<Vec<_>>(),
        vec![14]
    );
}

#[test]
fn d2_bypass_matches_documented_immediate_only_path() {
    let setret = isa::decode_word(281474524250110).expect("decode hl.setret");
    let fentry = isa::decode_word(178585665).expect("decode fentry");

    assert!(d2_bypass(&setret));
    assert!(!d2_bypass(&fentry));
}

#[test]
fn iq_entry_remains_inflight_until_i2_dealloc() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: false,
        load_ordinal: None,
        load_store_id: None,
        miss_injected: false,
        redirect_target: None,
        phys_iq: Some(PhysIq::AluIq0),
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: None,
        done_cycle: None,
    }];
    let mut iq = vec![IqEntry {
        seq: 0,
        phys_iq: PhysIq::AluIq0,
        inflight: true,
        src_valid: [false; 2],
        src_ready_nonspec: [false; 2],
        src_ready_spec: [false; 2],
        src_wait_qtag: [false; 2],
    }];
    let mut p1 = VecDeque::from([0usize]);
    let rob = VecDeque::from([0usize]);
    let admitted = arbitrate_i1(0, &mut p1, &mut iq, &uops, &rob);

    assert_eq!(admitted.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(iq.len(), 1);
    assert!(iq[0].inflight);

    let mut pipeline = StageQueues::default();
    pipeline.i1 = admitted;
    advance_i1_to_i2(&mut pipeline, &mut iq);

    assert_eq!(pipeline.i2.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert!(iq.is_empty());
}

#[test]
fn pick_uses_oldest_ready_rob_order() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = [PhysIq::BruIq, PhysIq::AluIq0, PhysIq::SharedIq1]
        .into_iter()
        .map(|phys_iq| CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(phys_iq),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        })
        .collect::<Vec<_>>();
    let mut iq = vec![
        IqEntry {
            seq: 2,
            phys_iq: PhysIq::SharedIq1,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
        IqEntry {
            seq: 0,
            phys_iq: PhysIq::BruIq,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
        IqEntry {
            seq: 1,
            phys_iq: PhysIq::AluIq0,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
    ];
    let rob = VecDeque::from([0usize, 1, 2]);
    let mut p1 = VecDeque::new();

    pick_from_iq(0, 0, &mut iq, &uops, &mut p1, &rob);

    assert_eq!(p1.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
    assert!(iq.iter().all(|entry| entry.inflight));
}

#[test]
fn pick_limits_to_one_winner_per_physical_iq() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = (0..3)
        .map(|_| CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        })
        .collect::<Vec<_>>();
    let mut iq = vec![
        IqEntry {
            seq: 2,
            phys_iq: PhysIq::AluIq0,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
        IqEntry {
            seq: 0,
            phys_iq: PhysIq::AluIq0,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
        IqEntry {
            seq: 1,
            phys_iq: PhysIq::AluIq0,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
    ];
    let rob = VecDeque::from([0usize, 1, 2]);
    let mut p1 = VecDeque::new();

    pick_from_iq(0, 0, &mut iq, &uops, &mut p1, &rob);

    assert_eq!(p1.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert!(iq.iter().find(|entry| entry.seq == 0).unwrap().inflight);
    assert!(!iq.iter().find(|entry| entry.seq == 1).unwrap().inflight);
    assert!(!iq.iter().find(|entry| entry.seq == 2).unwrap().inflight);
}

#[test]
fn i1_to_i2_limits_one_admit_per_physical_iq() {
    let mut pipeline = StageQueues::default();
    pipeline.i1 = VecDeque::from([0usize, 1, 2]);
    let mut iq = vec![
        IqEntry {
            seq: 0,
            phys_iq: PhysIq::AluIq0,
            inflight: true,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
        IqEntry {
            seq: 1,
            phys_iq: PhysIq::AluIq0,
            inflight: true,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
        IqEntry {
            seq: 2,
            phys_iq: PhysIq::BruIq,
            inflight: true,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
    ];

    advance_i1_to_i2(&mut pipeline, &mut iq);

    assert_eq!(pipeline.i2.iter().copied().collect::<Vec<_>>(), vec![0, 2]);
    assert_eq!(pipeline.i1.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(
        iq.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        vec![1]
    );
}

#[test]
fn emit_stage_events_reports_ready_vs_wait_iq_age_per_queue() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let pipeline = StageQueues::default();
    let iq = vec![
        IqEntry {
            seq: 1,
            phys_iq: PhysIq::AluIq0,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
        IqEntry {
            seq: 0,
            phys_iq: PhysIq::AluIq0,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
    ];
    let rob = VecDeque::from([0usize, 1]);
    let mut out = Vec::new();

    emit_stage_events(0, &runtime, &pipeline, &iq, &rob, &uops, &mut out);

    assert!(out.iter().any(|event| {
        event.stage_id == "IQ" && event.row_id == "uop0" && event.cause == "ready"
    }));
    assert!(out.iter().any(|event| {
        event.stage_id == "IQ" && event.row_id == "uop1" && event.cause == "wait_iq_age"
    }));
}

#[test]
fn i1_arbitration_uses_oldest_first_read_port_policy() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let mut uops = (0..3)
        .map(|seq| CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: 1,
                src1_valid: 1,
                src1_reg: 2,
                ..CommitRecord::unsupported(
                    0,
                    0x1000 + (seq as u64) * 4,
                    0,
                    4,
                    &isa::BlockMeta::default(),
                )
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        })
        .collect::<Vec<_>>();
    for uop in &mut uops {
        uop.commit.src0_data = 1;
        uop.commit.src1_data = 2;
    }
    let mut iq = vec![
        IqEntry {
            seq: 2,
            phys_iq: PhysIq::AluIq0,
            inflight: true,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
        IqEntry {
            seq: 0,
            phys_iq: PhysIq::AluIq0,
            inflight: true,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
        IqEntry {
            seq: 1,
            phys_iq: PhysIq::AluIq0,
            inflight: true,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
    ];
    let mut p1 = VecDeque::from([2usize, 0, 1]);
    let rob = VecDeque::from([0usize, 1, 2]);

    let admitted = arbitrate_i1(0, &mut p1, &mut iq, &uops, &rob);

    assert_eq!(admitted.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert!(!iq.iter().find(|entry| entry.seq == 1).unwrap().inflight);
    assert!(!iq.iter().find(|entry| entry.seq == 2).unwrap().inflight);
}

#[test]
fn load_spec_ready_source_skips_rf_read_ports() {
    let load = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let consumer = isa::decode_word(enc_addi(3, 2, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: load,
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(0),
            miss_injected: true,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(5),
            data_ready_visible: Some(8),
            miss_pending_until: None,
            e1_cycle: Some(4),
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: consumer,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: 2,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    assert_eq!(read_ports_needed(1, 5, &uops), 0);
    assert!(iq_entry_ready(1, 5, 0, &uops));
    assert!(!i2_ready(1, 5, &uops));
    assert!(i2_ready(1, 8, &uops));
}

#[test]
fn load_consumer_waits_in_i2_for_e4_forward() {
    let load = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let consumer = isa::decode_word(enc_addi(3, 2, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: load,
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(0),
            miss_injected: true,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(5),
            data_ready_visible: Some(8),
            miss_pending_until: None,
            e1_cycle: Some(4),
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: consumer,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: 2,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut i2 = VecDeque::from([1usize]);
    let mut e1 = VecDeque::new();
    let mut lhq = VecDeque::new();
    let mut stq = VecDeque::new();

    let mut lsid_issue_ptr = 0usize;
    let mut lsid_complete_ptr = 0usize;
    advance_i2(
        5,
        &mut i2,
        &mut e1,
        &mut lhq,
        &mut stq,
        &mut lsid_issue_ptr,
        &mut lsid_complete_ptr,
        &uops,
    );
    assert_eq!(i2.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert!(e1.is_empty());

    advance_i2(
        8,
        &mut i2,
        &mut e1,
        &mut lhq,
        &mut stq,
        &mut lsid_issue_ptr,
        &mut lsid_complete_ptr,
        &uops,
    );
    assert!(i2.is_empty());
    assert_eq!(e1.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert!(lhq.is_empty());
}

#[test]
fn iq_spec_ready_revokes_on_replay_reset_and_rewakes_later() {
    let load = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let consumer = isa::decode_word(enc_addi(3, 2, 1) as u64).expect("decode addi");
    let mut uops = vec![
        CycleUop {
            decoded: load,
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(0),
            miss_injected: true,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(5),
            data_ready_visible: Some(8),
            miss_pending_until: None,
            e1_cycle: Some(4),
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: consumer,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: 2,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut iq = vec![make_iq_entry(
        5,
        1,
        PhysIq::AluIq0,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &uops,
    )];

    assert!(iq[0].src_ready_spec[0]);
    assert!(!iq[0].src_ready_nonspec[0]);

    uops[0].pick_wakeup_visible = None;
    uops[0].data_ready_visible = None;
    uops[0].e1_cycle = None;
    uops[0].miss_pending_until = Some(12);
    let iq_tags = test_iq_tags(&iq);
    let iq_owner_table = test_iq_owner_table(&iq, &iq_tags);
    let crossbar = test_qtag_wait_crossbar(&iq, &uops);
    update_iq_entries_for_cycle(
        6,
        &mut iq,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &iq_owner_table,
        &iq_tags,
        &crossbar,
        &uops,
    );

    assert!(!iq[0].src_ready_spec[0]);
    assert!(!iq[0].src_ready_nonspec[0]);
    assert_eq!(
        iq_entry_wait_cause_from_state(&iq[0], 6, 0, &uops),
        Some("wait_miss")
    );

    uops[0].miss_pending_until = None;
    uops[0].pick_wakeup_visible = Some(9);
    let iq_tags = test_iq_tags(&iq);
    let iq_owner_table = test_iq_owner_table(&iq, &iq_tags);
    let crossbar = test_qtag_wait_crossbar(&iq, &uops);
    update_iq_entries_for_cycle(
        9,
        &mut iq,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &iq_owner_table,
        &iq_tags,
        &crossbar,
        &uops,
    );

    assert!(iq[0].src_ready_spec[0]);
    assert!(!iq[0].src_ready_nonspec[0]);
}

#[test]
fn iq_load_spec_wakeup_can_come_from_prior_e1_stage() {
    let load = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let consumer = isa::decode_word(enc_addi(3, 2, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: load,
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(0),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: Some(4),
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: consumer,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: 2,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut iq = vec![make_iq_entry(
        4,
        1,
        PhysIq::AluIq0,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &uops,
    )];

    assert!(!iq[0].src_ready_spec[0]);
    let iq_tags = test_iq_tags(&iq);
    let iq_owner_table = test_iq_owner_table(&iq, &iq_tags);
    let crossbar = test_qtag_wait_crossbar(&iq, &uops);
    update_iq_entries_for_cycle(
        5,
        &mut iq,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &iq_owner_table,
        &iq_tags,
        &crossbar,
        &uops,
    );
    assert!(iq[0].src_ready_spec[0]);
    assert!(!iq[0].src_ready_nonspec[0]);
}

#[test]
fn iq_nonspec_wakeup_can_come_from_prior_w1_stage() {
    let producer = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let consumer = isa::decode_word(enc_addi(3, 2, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: producer,
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(4),
            done_cycle: None,
        },
        CycleUop {
            decoded: consumer,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: 2,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut iq = vec![make_iq_entry(
        4,
        1,
        PhysIq::AluIq0,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &uops,
    )];

    assert!(!iq[0].src_ready_nonspec[0]);
    let iq_tags = test_iq_tags(&iq);
    let iq_owner_table = test_iq_owner_table(&iq, &iq_tags);
    let crossbar = test_qtag_wait_crossbar(&iq, &uops);
    update_iq_entries_for_cycle(
        5,
        &mut iq,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &iq_owner_table,
        &iq_tags,
        &crossbar,
        &uops,
    );
    assert!(iq[0].src_ready_nonspec[0]);
    assert!(!iq[0].src_ready_spec[0]);
}

#[test]
fn qtag_wakeup_fanout_only_wakes_matching_queue_consumer() {
    let producer = isa::decode_word(enc_addi(31, 0, 1) as u64).expect("decode implicit-t producer");
    let consumer =
        isa::decode_word(enc_addi(2, REG_T1 as u32, 1) as u64).expect("decode implicit-t consumer");
    let uops = vec![
        CycleUop {
            decoded: producer,
            commit: CommitRecord {
                wb_valid: 1,
                wb_rd: REG_T1,
                ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: Some(QueueWakeKind::T),
            dst_logical_tag: Some(LogicalQueueTag {
                kind: QueueWakeKind::T,
                tag: 0,
            }),
            dst_qtag: Some(QTag {
                phys_iq: PhysIq::AluIq0,
                entry_id: 3,
            }),
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(4),
            done_cycle: None,
        },
        CycleUop {
            decoded: consumer.clone(),
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: REG_T1,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [Some(QueueWakeKind::T), None],
            src_logical_tags: [
                Some(LogicalQueueTag {
                    kind: QueueWakeKind::T,
                    tag: 0,
                }),
                None,
            ],
            src_qtags: [
                Some(QTag {
                    phys_iq: PhysIq::AluIq0,
                    entry_id: 3,
                }),
                None,
            ],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: consumer,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: REG_T1,
                ..CommitRecord::unsupported(0, 0x1008, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [Some(QueueWakeKind::T), None],
            src_logical_tags: [
                Some(LogicalQueueTag {
                    kind: QueueWakeKind::T,
                    tag: 0,
                }),
                None,
            ],
            src_qtags: [
                Some(QTag {
                    phys_iq: PhysIq::SharedIq1,
                    entry_id: 3,
                }),
                None,
            ],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: isa::decode_word(enc_addi(2, REG_T1 as u32, 1) as u64)
                .expect("decode implicit-t consumer same queue wrong slot"),
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: REG_T1,
                ..CommitRecord::unsupported(0, 0x100c, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [Some(QueueWakeKind::T), None],
            src_logical_tags: [
                Some(LogicalQueueTag {
                    kind: QueueWakeKind::T,
                    tag: 0,
                }),
                None,
            ],
            src_qtags: [
                Some(QTag {
                    phys_iq: PhysIq::AluIq0,
                    entry_id: 2,
                }),
                None,
            ],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut iq = vec![
        make_iq_entry(
            4,
            1,
            PhysIq::AluIq0,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &uops,
        ),
        make_iq_entry(
            4,
            2,
            PhysIq::AluIq0,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &uops,
        ),
        make_iq_entry(
            4,
            3,
            PhysIq::AluIq0,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &uops,
        ),
    ];

    let iq_tags = test_iq_tags(&iq);
    let iq_owner_table = test_iq_owner_table(&iq, &iq_tags);
    let crossbar = test_qtag_wait_crossbar(&iq, &uops);
    update_iq_entries_for_cycle(
        5,
        &mut iq,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &iq_owner_table,
        &iq_tags,
        &crossbar,
        &uops,
    );

    assert!(iq[0].src_ready_nonspec[0]);
    assert!(!iq[1].src_ready_nonspec[0]);
    assert!(!iq[2].src_ready_nonspec[0]);
    assert_eq!(iq_entry_wait_cause_from_state(&iq[0], 5, 0, &uops), None);
    assert_eq!(
        iq_entry_wait_cause_from_state(&iq[1], 5, 0, &uops),
        Some("wait_qtag")
    );
    assert_eq!(
        iq_entry_wait_cause_from_state(&iq[2], 5, 0, &uops),
        Some("wait_qtag")
    );
}

#[test]
fn queue_wakeup_keeps_nonqueue_dependents_alive() {
    let producer = isa::decode_word(3191065).expect("decode ldi ->{t,u,Rd}");
    let queue_consumer = isa::decode_word(8314).expect("decode c.sdi t#1 consumer");
    let reg_consumer =
        isa::decode_word(enc_addi(3, 2, 1) as u64).expect("decode addi reg consumer");
    let uops = vec![
        CycleUop {
            decoded: producer,
            commit: CommitRecord {
                wb_valid: 1,
                wb_rd: 2,
                ..CommitRecord::unsupported(0, 0x1000, 3191065, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: Some(QueueWakeKind::T),
            dst_logical_tag: Some(LogicalQueueTag {
                kind: QueueWakeKind::T,
                tag: 0,
            }),
            dst_qtag: Some(QTag {
                phys_iq: PhysIq::AguIq0,
                entry_id: 1,
            }),
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(4),
            done_cycle: None,
        },
        CycleUop {
            decoded: queue_consumer,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: REG_T1,
                ..CommitRecord::unsupported(0, 0x1004, 8314, 2, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [Some(QueueWakeKind::T), None],
            src_logical_tags: [
                Some(LogicalQueueTag {
                    kind: QueueWakeKind::T,
                    tag: 0,
                }),
                None,
            ],
            src_qtags: [
                Some(QTag {
                    phys_iq: PhysIq::AguIq0,
                    entry_id: 1,
                }),
                None,
            ],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::StdIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: reg_consumer,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: 2,
                ..CommitRecord::unsupported(0, 0x1008, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut iq = vec![
        make_iq_entry(
            4,
            1,
            PhysIq::StdIq0,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &uops,
        ),
        make_iq_entry(
            4,
            2,
            PhysIq::AluIq0,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &uops,
        ),
    ];

    let iq_tags = test_iq_tags(&iq);
    let iq_owner_table = test_iq_owner_table(&iq, &iq_tags);
    let crossbar = test_qtag_wait_crossbar(&iq, &uops);
    update_iq_entries_for_cycle(
        5,
        &mut iq,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &iq_owner_table,
        &iq_tags,
        &crossbar,
        &uops,
    );

    assert!(iq[0].src_ready_nonspec[0]);
    assert!(iq[1].src_ready_nonspec[0]);
    assert_eq!(iq_entry_wait_cause_from_state(&iq[0], 5, 0, &uops), None);
    assert_eq!(iq_entry_wait_cause_from_state(&iq[1], 5, 0, &uops), None);
}

#[test]
fn ld_gen_vec_propagates_through_dependency_chain() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(0),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(5),
            data_ready_visible: Some(8),
            miss_pending_until: None,
            e1_cycle: Some(4),
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default()),
            deps: [Some(0), None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord::unsupported(0, 0x1008, 0, 4, &isa::BlockMeta::default()),
            deps: [Some(1), None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    assert_eq!(dep_load_gen_vec(2, 4, &uops), LD_GEN_E1);
    assert_eq!(dep_load_gen_vec(2, 5, &uops), LD_GEN_E2);
    assert_eq!(dep_load_gen_vec(2, 6, &uops), LD_GEN_E3);
}

#[test]
fn miss_pending_suppresses_ld_e4_dependent_pick() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(0),
            miss_injected: true,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(5),
            data_ready_visible: Some(12),
            miss_pending_until: Some(12),
            e1_cycle: Some(4),
            e4_cycle: Some(7),
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord {
                src0_valid: 1,
                src0_reg: 2,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [Some(0), None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    assert!(miss_pending_active(8, &uops));
    assert_eq!(dep_load_gen_vec(0, 8, &uops), LD_GEN_E4);
    assert!(!iq_entry_ready(1, 8, 0, &uops));

    let mut replayed = uops.clone();
    replayed[0].miss_pending_until = None;
    replayed[0].e1_cycle = Some(12);
    replayed[0].e4_cycle = None;
    replayed[0].pick_wakeup_visible = Some(13);
    replayed[0].data_ready_visible = Some(16);
    assert!(iq_entry_ready(1, 13, 0, &replayed));
}

#[test]
fn s2_stalls_when_iq_is_full() {
    let mut pipeline = StageQueues::default();
    pipeline.frontend[10].push_back(0);
    let mut iq = (0..IQ_CAPACITY)
        .map(|seq| IqEntry {
            seq,
            phys_iq: PhysIq::AluIq0,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        })
        .chain((IQ_CAPACITY..(IQ_CAPACITY * 2)).map(|seq| IqEntry {
            seq,
            phys_iq: PhysIq::SharedIq1,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        }))
        .collect::<Vec<_>>();
    let mut rob = VecDeque::new();
    let mut uops = vec![CycleUop {
        decoded: isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi"),
        commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: false,
        load_ordinal: None,
        load_store_id: None,
        miss_injected: false,
        redirect_target: None,
        phys_iq: None,
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: None,
        done_cycle: None,
    }];

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(
        pipeline.frontend[10].iter().copied().collect::<Vec<_>>(),
        vec![0]
    );
    assert_eq!(iq.len(), IQ_CAPACITY * 2);
}

#[test]
fn s2_spills_third_same_cycle_alu_enqueue_to_shared_iq() {
    let mut pipeline = StageQueues::default();
    pipeline.frontend[10].extend([0, 1, 2]);
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let mut uops = (0..3)
        .map(|_| CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        })
        .collect::<Vec<_>>();

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert!(pipeline.frontend[10].is_empty());
    assert_eq!(
        iq.iter()
            .map(|entry| (entry.seq, entry.phys_iq))
            .collect::<Vec<_>>(),
        vec![
            (0, PhysIq::AluIq0),
            (1, PhysIq::AluIq0),
            (2, PhysIq::SharedIq1),
        ]
    );
}

#[test]
fn s2_shared_only_enqueue_keeps_oldest_two_when_ports_exhausted() {
    let mut pipeline = StageQueues::default();
    pipeline.frontend[10].extend([0, 1, 2]);
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let mut uops = (0..3)
        .map(|_| {
            let mut sys_decoded = decoded.clone();
            sys_decoded.uop_group = "SYS".to_string();
            CycleUop {
                decoded: sys_decoded,
                commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
                deps: [None, None],
                src_queue_kinds: [None, None],
                src_logical_tags: [None, None],
                src_qtags: [None, None],
                dst_queue_kind: None,
                dst_logical_tag: None,
                dst_qtag: None,
                bypass_d2: false,
                is_load: false,
                is_store: false,
                load_ordinal: None,
                load_store_id: None,
                miss_injected: false,
                redirect_target: None,
                phys_iq: None,
                pick_wakeup_visible: None,
                data_ready_visible: None,
                miss_pending_until: None,
                e1_cycle: None,
                e4_cycle: None,
                w1_cycle: None,
                done_cycle: None,
            }
        })
        .collect::<Vec<_>>();

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(
        iq.iter()
            .map(|entry| (entry.seq, entry.phys_iq))
            .collect::<Vec<_>>(),
        vec![(0, PhysIq::SharedIq1), (1, PhysIq::SharedIq1)]
    );
    assert_eq!(
        pipeline.frontend[10].iter().copied().collect::<Vec<_>>(),
        vec![2]
    );
}

#[test]
fn s2_allocates_distinct_qtags_within_one_physical_iq() {
    let mut pipeline = StageQueues::default();
    pipeline.frontend[10].extend([0, 1]);
    let mut iq = Vec::new();
    let mut rob = VecDeque::new();
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let mut uops = (0..2)
        .map(|_| CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        })
        .collect::<Vec<_>>();

    dispatch_to_iq_and_bypass(0, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(
        pipeline.iq_tags.get(&0),
        Some(&QTag {
            phys_iq: PhysIq::AluIq0,
            entry_id: 0,
        })
    );
    assert_eq!(
        pipeline.iq_tags.get(&1),
        Some(&QTag {
            phys_iq: PhysIq::AluIq0,
            entry_id: 1,
        })
    );
    assert_eq!(pipeline.iq_owner_table[PhysIq::AluIq0.index()][0], Some(0));
    assert_eq!(pipeline.iq_owner_table[PhysIq::AluIq0.index()][1], Some(1));
}

#[test]
fn i2_deallocation_releases_qtag_for_reuse() {
    let mut pipeline = StageQueues::default();
    pipeline.i1 = VecDeque::from([0usize]);
    pipeline.iq_tags.insert(
        0,
        QTag {
            phys_iq: PhysIq::AluIq0,
            entry_id: 0,
        },
    );
    let mut iq = vec![IqEntry {
        seq: 0,
        phys_iq: PhysIq::AluIq0,
        inflight: true,
        src_valid: [false; 2],
        src_ready_nonspec: [false; 2],
        src_ready_spec: [false; 2],
        src_wait_qtag: [false; 2],
    }];

    advance_i1_to_i2(&mut pipeline, &mut iq);

    assert!(pipeline.iq_tags.is_empty());
    assert_eq!(pipeline.iq_owner_table[PhysIq::AluIq0.index()][0], None);

    pipeline.frontend[10].push_back(1);
    let mut rob = VecDeque::new();
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let mut uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    dispatch_to_iq_and_bypass(1, &mut pipeline, &mut iq, &mut rob, &mut uops);

    assert_eq!(
        pipeline.iq_tags.get(&1),
        Some(&QTag {
            phys_iq: PhysIq::AluIq0,
            entry_id: 0,
        })
    );
    assert_eq!(pipeline.iq_owner_table[PhysIq::AluIq0.index()][0], Some(0));
}

#[test]
fn redirect_flush_prunes_younger_qtags() {
    let mut pipeline = StageQueues::default();
    pipeline.iq_tags.insert(
        1,
        QTag {
            phys_iq: PhysIq::AluIq0,
            entry_id: 0,
        },
    );
    pipeline.iq_tags.insert(
        2,
        QTag {
            phys_iq: PhysIq::SharedIq1,
            entry_id: 0,
        },
    );
    let mut iq = vec![
        IqEntry {
            seq: 1,
            phys_iq: PhysIq::AluIq0,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
        IqEntry {
            seq: 2,
            phys_iq: PhysIq::SharedIq1,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
    ];
    let mut rob = VecDeque::from([1usize, 2]);
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let boundary = isa::decode_word(2048).expect("decode c.bstart.std");
    let uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x0ffc, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: None,
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: boundary,
            commit: CommitRecord::unsupported(4, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x2000),
            phys_iq: Some(PhysIq::BruIq),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(7),
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord::unsupported(4, 0x1004, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::SharedIq1),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    prune_speculative_state_on_redirect(7, &mut pipeline, &mut iq, &mut rob, &uops);

    assert_eq!(
        pipeline.iq_tags,
        BTreeMap::from([(
            1usize,
            QTag {
                phys_iq: PhysIq::AluIq0,
                entry_id: 0,
            },
        )])
    );
    assert_eq!(
        iq.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(pipeline.iq_owner_table[PhysIq::AluIq0.index()][0], Some(0));
    assert_eq!(pipeline.iq_owner_table[PhysIq::SharedIq1.index()][0], None);
}

#[test]
fn advance_execute_injects_configured_load_miss() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let mut pipeline = StageQueues::default();
    pipeline.e4.push_back(0);
    pipeline.lhq.push_back(0);
    let mut uops = vec![CycleUop {
        decoded,
        commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: true,
        is_store: false,
        load_ordinal: Some(0),
        load_store_id: Some(0),
        miss_injected: false,
        redirect_target: None,
        phys_iq: Some(PhysIq::AguIq0),
        pick_wakeup_visible: Some(5),
        data_ready_visible: Some(8),
        miss_pending_until: None,
        e1_cycle: Some(4),
        e4_cycle: Some(7),
        w1_cycle: None,
        done_cycle: None,
    }];

    advance_execute(
        9,
        &mut pipeline,
        &mut uops,
        &CycleRunOptions {
            max_cycles: 32,
            load_miss_every: Some(1),
            load_miss_penalty: 4,
        },
    );

    assert!(pipeline.w1.is_empty());
    assert_eq!(pipeline.liq.len(), 1);
    assert_eq!(pipeline.liq[0].seq, 0);
    assert_eq!(pipeline.liq[0].refill_ready_cycle, 13);
    assert_eq!(pipeline.mdb.len(), 1);
    assert_eq!(pipeline.mdb[0].seq, 0);
    assert!(pipeline.lhq.is_empty());
    assert!(uops[0].miss_injected);
    assert_eq!(uops[0].miss_pending_until, Some(13));
    assert_eq!(uops[0].pick_wakeup_visible, None);
    assert_eq!(uops[0].data_ready_visible, None);
    assert_eq!(uops[0].e1_cycle, None);
    assert_eq!(uops[0].e4_cycle, None);
}

#[test]
fn advance_liq_requeues_oldest_ready_load_into_e1() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let mut pipeline = StageQueues::default();
    pipeline.liq = VecDeque::from([
        LiqEntry {
            seq: 1,
            refill_ready_cycle: 11,
        },
        LiqEntry {
            seq: 0,
            refill_ready_cycle: 11,
        },
    ]);
    pipeline.mdb = VecDeque::from([
        MdbEntry {
            seq: 1,
            refill_ready_cycle: 11,
        },
        MdbEntry {
            seq: 0,
            refill_ready_cycle: 11,
        },
    ]);
    let mut uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(0),
            miss_injected: true,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: Some(11),
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(1),
            load_store_id: Some(1),
            miss_injected: true,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: Some(11),
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let rob = VecDeque::from([0usize, 1]);

    advance_liq(11, &mut pipeline, &mut uops, &rob);

    assert_eq!(pipeline.e1.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(
        pipeline
            .liq
            .iter()
            .map(|entry| entry.seq)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(
        pipeline
            .mdb
            .iter()
            .map(|entry| entry.seq)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(pipeline.lhq.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(uops[0].miss_pending_until, None);
    assert_eq!(uops[1].miss_pending_until, Some(11));
}

#[test]
fn emit_stage_events_reports_liq_lhq_and_mdb() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: true,
        is_store: false,
        load_ordinal: Some(0),
        load_store_id: Some(0),
        miss_injected: true,
        redirect_target: None,
        phys_iq: Some(PhysIq::AguIq0),
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: Some(12),
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: None,
        done_cycle: None,
    }];
    let mut pipeline = StageQueues::default();
    pipeline.liq.push_back(LiqEntry {
        seq: 0,
        refill_ready_cycle: 12,
    });
    pipeline.lhq.push_back(0);
    pipeline.mdb.push_back(MdbEntry {
        seq: 0,
        refill_ready_cycle: 12,
    });
    let mut out = Vec::new();

    emit_stage_events(
        10,
        &runtime,
        &pipeline,
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );

    assert!(out.iter().any(|event| event.stage_id == "LIQ"));
    assert!(out.iter().any(|event| event.stage_id == "LHQ"));
    assert!(out.iter().any(|event| event.stage_id == "MDB"));
}

#[test]
fn build_uops_assigns_monotonic_load_store_ids() {
    let block = isa::BlockMeta::default();
    let commits = vec![
        CommitRecord {
            mem_valid: 1,
            mem_is_store: 1,
            mem_addr: 0x2000,
            mem_size: 8,
            ..CommitRecord::unsupported(0, 0x1000, enc_addi(2, 0, 1) as u64, 4, &block)
        },
        CommitRecord::unsupported(0, 0x1004, enc_addi(3, 0, 1) as u64, 4, &block),
        CommitRecord {
            mem_valid: 1,
            mem_is_store: 0,
            mem_addr: 0x2008,
            mem_size: 8,
            ..CommitRecord::unsupported(0, 0x1008, enc_addi(4, 0, 1) as u64, 4, &block)
        },
    ];
    let decoded = vec![
        isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi"),
        isa::decode_word(enc_addi(3, 0, 1) as u64).expect("decode addi"),
        isa::decode_word(enc_addi(4, 0, 1) as u64).expect("decode addi"),
    ];

    let uops = build_uops(&commits, &decoded);

    assert_eq!(uops[0].load_store_id, Some(0));
    assert_eq!(uops[1].load_store_id, None);
    assert_eq!(uops[2].load_store_id, Some(1));
}

#[test]
fn redirect_rebases_lsid_to_oldest_surviving_unissued_memory_uop() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let boundary = isa::decode_word(2048).expect("decode c.bstart.std");
    let mut pipeline = StageQueues::default();
    pipeline.lsid_issue_ptr = 9;
    pipeline.lsid_complete_ptr = 9;
    pipeline.lsid_cache_ptr = 9;
    pipeline.frontend[0].push_back(2);
    pipeline.i2.push_back(3);
    pipeline.e1.push_back(0);

    let iq = vec![IqEntry {
        seq: 1,
        phys_iq: PhysIq::AguIq0,
        inflight: false,
        src_valid: [false; 2],
        src_ready_nonspec: [false; 2],
        src_ready_spec: [false; 2],
        src_wait_qtag: [false; 2],
    }];
    let rob = VecDeque::from([0usize, 1, 2, 3]);
    let uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord {
                mem_valid: 1,
                mem_is_store: 0,
                mem_addr: 0x1000,
                mem_size: 8,
                ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(0),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: Some(4),
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord {
                mem_valid: 1,
                mem_is_store: 0,
                mem_addr: 0x1008,
                mem_size: 8,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(1),
            load_store_id: Some(1),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: boundary,
            commit: CommitRecord::unsupported(0, 0x1008, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x2000),
            phys_iq: Some(PhysIq::BruIq),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(5),
            done_cycle: Some(5),
        },
        CycleUop {
            decoded,
            commit: CommitRecord {
                mem_valid: 1,
                mem_is_store: 1,
                mem_addr: 0x1010,
                mem_size: 8,
                ..CommitRecord::unsupported(0, 0x100c, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: true,
            load_ordinal: None,
            load_store_id: Some(2),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::StdIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    rebase_lsid_on_redirect(5, &mut pipeline, &iq, &rob, &uops);

    assert_eq!(pipeline.lsid_issue_ptr, 1);
    assert_eq!(pipeline.lsid_complete_ptr, 1);
    assert_eq!(pipeline.lsid_cache_ptr, 0);
}

#[test]
fn redirect_rebase_ignores_non_redirect_cycles() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let mut pipeline = StageQueues::default();
    pipeline.lsid_issue_ptr = 4;
    pipeline.lsid_complete_ptr = 4;
    pipeline.lsid_cache_ptr = 4;
    pipeline.i2.push_back(0);

    let iq = Vec::new();
    let rob = VecDeque::from([0usize]);
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord {
            mem_valid: 1,
            mem_is_store: 0,
            mem_addr: 0x4000,
            mem_size: 8,
            ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
        },
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: true,
        is_store: false,
        load_ordinal: Some(0),
        load_store_id: Some(0),
        miss_injected: false,
        redirect_target: None,
        phys_iq: Some(PhysIq::AguIq0),
        pick_wakeup_visible: Some(0),
        data_ready_visible: Some(0),
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: None,
        done_cycle: None,
    }];

    rebase_lsid_on_redirect(7, &mut pipeline, &iq, &rob, &uops);

    assert_eq!(pipeline.lsid_issue_ptr, 4);
    assert_eq!(pipeline.lsid_complete_ptr, 4);
    assert_eq!(pipeline.lsid_cache_ptr, 4);
}

#[test]
fn redirect_prunes_younger_memory_owner_state() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let boundary = isa::decode_word(2048).expect("decode c.bstart.std");
    let mut pipeline = StageQueues::default();
    pipeline.stq.extend([0usize, 3]);
    pipeline.lhq.extend([1usize, 4]);
    pipeline.liq.push_back(LiqEntry {
        seq: 0,
        refill_ready_cycle: 9,
    });
    pipeline.liq.push_back(LiqEntry {
        seq: 3,
        refill_ready_cycle: 9,
    });
    pipeline.mdb.push_back(MdbEntry {
        seq: 1,
        refill_ready_cycle: 9,
    });
    pipeline.mdb.push_back(MdbEntry {
        seq: 4,
        refill_ready_cycle: 9,
    });
    pipeline.scb.push_back(ScbEntry {
        seq: 2,
        enqueue_cycle: 4,
    });
    pipeline.scb.push_back(ScbEntry {
        seq: 5,
        enqueue_cycle: 4,
    });
    pipeline.l1d.push_back(L1dEntry {
        seq: 1,
        kind: L1dTxnKind::LoadHit,
        ready_cycle: 6,
    });
    pipeline.l1d.push_back(L1dEntry {
        seq: 4,
        kind: L1dTxnKind::StoreDrain,
        ready_cycle: 6,
    });

    let uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord {
                mem_valid: 1,
                mem_is_store: 1,
                mem_addr: 0x1000,
                mem_size: 8,
                ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: true,
            load_ordinal: None,
            load_store_id: Some(0),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::StdIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord {
                mem_valid: 1,
                mem_is_store: 0,
                mem_addr: 0x1008,
                mem_size: 8,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(1),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: boundary,
            commit: CommitRecord::unsupported(0, 0x1008, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x2000),
            phys_iq: Some(PhysIq::BruIq),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(5),
            done_cycle: Some(5),
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord {
                mem_valid: 1,
                mem_is_store: 1,
                mem_addr: 0x1010,
                mem_size: 8,
                ..CommitRecord::unsupported(0, 0x100c, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: true,
            load_ordinal: None,
            load_store_id: Some(2),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::StdIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord {
                mem_valid: 1,
                mem_is_store: 0,
                mem_addr: 0x1018,
                mem_size: 8,
                ..CommitRecord::unsupported(0, 0x1010, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(1),
            load_store_id: Some(3),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord {
                mem_valid: 1,
                mem_is_store: 1,
                mem_addr: 0x1020,
                mem_size: 8,
                ..CommitRecord::unsupported(0, 0x1014, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: true,
            load_ordinal: None,
            load_store_id: Some(4),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::StdIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    prune_memory_owner_state_on_redirect(5, &mut pipeline, &uops);

    assert_eq!(pipeline.stq.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(pipeline.lhq.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(
        pipeline
            .liq
            .iter()
            .map(|entry| entry.seq)
            .collect::<Vec<_>>(),
        vec![0]
    );
    assert_eq!(
        pipeline
            .mdb
            .iter()
            .map(|entry| entry.seq)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(
        pipeline
            .scb
            .iter()
            .map(|entry| entry.seq)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(
        pipeline
            .l1d
            .iter()
            .map(|entry| entry.seq)
            .collect::<Vec<_>>(),
        vec![1]
    );
}

#[test]
fn redirect_prunes_younger_frontend_iq_backend_and_rob_state() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let boundary = isa::decode_word(2048).expect("decode c.bstart.std");
    let mut pipeline = StageQueues::default();
    pipeline.frontend[0].extend([1usize, 4]);
    pipeline.frontend[7].extend([2usize, 5]);
    pipeline.p1.extend([1usize, 4]);
    pipeline.i1.extend([2usize, 5]);
    pipeline.i2.extend([1usize, 6]);
    pipeline.e1.extend([0usize, 4]);
    pipeline.e2.extend([1usize, 5]);
    pipeline.e3.extend([2usize, 6]);
    pipeline.e4.extend([1usize, 7]);
    pipeline.w1.extend([2usize, 8]);
    pipeline.w2.extend([0usize, 9]);
    let mut iq = vec![
        IqEntry {
            seq: 1,
            phys_iq: PhysIq::AluIq0,
            inflight: false,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
        IqEntry {
            seq: 6,
            phys_iq: PhysIq::AguIq0,
            inflight: true,
            src_valid: [false; 2],
            src_ready_nonspec: [false; 2],
            src_ready_spec: [false; 2],
            src_wait_qtag: [false; 2],
        },
    ];
    let mut rob = VecDeque::from([0usize, 1, 2, 6]);
    let uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: boundary,
            commit: CommitRecord::unsupported(0, 0x1008, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: Some(0x2000),
            phys_iq: Some(PhysIq::BruIq),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: Some(5),
            done_cycle: Some(5),
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x100c, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1010, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1014, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AluIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1018, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x101c, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1020, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord::unsupported(0, 0x1024, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: false,
            load_ordinal: None,
            load_store_id: None,
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(4),
            data_ready_visible: Some(4),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];

    prune_speculative_state_on_redirect(5, &mut pipeline, &mut iq, &mut rob, &uops);

    assert_eq!(
        pipeline.frontend[0].iter().copied().collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(
        pipeline.frontend[7].iter().copied().collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(pipeline.p1.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(pipeline.i1.iter().copied().collect::<Vec<_>>(), vec![2]);
    assert_eq!(pipeline.i2.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(pipeline.e1.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(pipeline.e2.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(pipeline.e3.iter().copied().collect::<Vec<_>>(), vec![2]);
    assert_eq!(pipeline.e4.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(pipeline.w1.iter().copied().collect::<Vec<_>>(), vec![2]);
    assert_eq!(pipeline.w2.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(
        iq.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(rob.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
}

#[test]
fn i2_waits_for_matching_lsid_before_memory_issue() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord {
            mem_valid: 1,
            mem_is_store: 0,
            mem_addr: 0x5000,
            mem_size: 8,
            ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
        },
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: true,
        is_store: false,
        load_ordinal: Some(0),
        load_store_id: Some(1),
        miss_injected: false,
        redirect_target: None,
        phys_iq: Some(PhysIq::AguIq0),
        pick_wakeup_visible: Some(5),
        data_ready_visible: Some(5),
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: None,
        done_cycle: None,
    }];
    let mut i2 = VecDeque::from([0usize]);
    let mut e1 = VecDeque::new();
    let mut lhq = VecDeque::new();
    let mut stq = VecDeque::new();
    let mut lsid_issue_ptr = 0usize;
    let mut lsid_complete_ptr = 0usize;

    assert!(i2_waits_on_lsid(0, 5, lsid_issue_ptr, &uops));
    advance_i2(
        5,
        &mut i2,
        &mut e1,
        &mut lhq,
        &mut stq,
        &mut lsid_issue_ptr,
        &mut lsid_complete_ptr,
        &uops,
    );

    assert_eq!(i2.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert!(e1.is_empty());
    assert_eq!(lsid_issue_ptr, 0);
    assert_eq!(lsid_complete_ptr, 0);

    lsid_issue_ptr = 1;
    lsid_complete_ptr = 1;
    advance_i2(
        5,
        &mut i2,
        &mut e1,
        &mut lhq,
        &mut stq,
        &mut lsid_issue_ptr,
        &mut lsid_complete_ptr,
        &uops,
    );

    assert!(i2.is_empty());
    assert_eq!(e1.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(lhq.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(lsid_issue_ptr, 2);
    assert_eq!(lsid_complete_ptr, 2);
}

#[test]
fn store_enters_stq_at_i2_confirmation() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord {
            mem_valid: 1,
            mem_is_store: 1,
            mem_addr: 0x2000,
            mem_size: 8,
            ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
        },
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: true,
        load_ordinal: None,
        load_store_id: Some(0),
        miss_injected: false,
        redirect_target: None,
        phys_iq: Some(PhysIq::StdIq0),
        pick_wakeup_visible: Some(5),
        data_ready_visible: Some(5),
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: None,
        done_cycle: None,
    }];
    let mut i2 = VecDeque::from([0usize]);
    let mut e1 = VecDeque::new();
    let mut lhq = VecDeque::new();
    let mut stq = VecDeque::new();

    let mut lsid_issue_ptr = 0usize;
    let mut lsid_complete_ptr = 0usize;
    advance_i2(
        5,
        &mut i2,
        &mut e1,
        &mut lhq,
        &mut stq,
        &mut lsid_issue_ptr,
        &mut lsid_complete_ptr,
        &uops,
    );

    assert!(i2.is_empty());
    assert_eq!(e1.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert!(lhq.is_empty());
    assert_eq!(stq.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(lsid_issue_ptr, 1);
    assert_eq!(lsid_complete_ptr, 1);
}

#[test]
fn retired_store_moves_from_stq_to_scb() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let mut rob = VecDeque::from([0usize]);
    let mut committed = Vec::new();
    let mut retired_seqs = Vec::new();
    let mut stage_events = Vec::new();
    let mut pipeline = StageQueues::default();
    pipeline.stq.push_back(0);
    let mut uops = vec![CycleUop {
        decoded: isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi"),
        commit: CommitRecord {
            mem_valid: 1,
            mem_is_store: 1,
            mem_addr: 0x2000,
            mem_size: 8,
            ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
        },
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: true,
        load_ordinal: None,
        load_store_id: Some(0),
        miss_injected: false,
        redirect_target: None,
        phys_iq: Some(PhysIq::StdIq0),
        pick_wakeup_visible: None,
        data_ready_visible: None,
        miss_pending_until: None,
        e1_cycle: None,
        e4_cycle: None,
        w1_cycle: Some(7),
        done_cycle: Some(8),
    }];

    retire_ready(
        9,
        &runtime,
        &mut rob,
        &mut committed,
        &mut retired_seqs,
        &mut pipeline,
        &mut uops,
        &mut stage_events,
    );

    assert!(pipeline.stq.is_empty());
    assert_eq!(pipeline.scb.len(), 1);
    assert_eq!(pipeline.scb[0].seq, 0);
    assert_eq!(pipeline.scb[0].enqueue_cycle, 9);
}

#[test]
fn emit_stage_events_marks_store_forwarded_loads() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord {
                mem_valid: 1,
                mem_is_store: 1,
                mem_addr: 0x3000,
                mem_size: 8,
                ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: false,
            is_store: true,
            load_ordinal: None,
            load_store_id: Some(0),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::StdIq0),
            pick_wakeup_visible: None,
            data_ready_visible: None,
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord {
                mem_valid: 1,
                mem_is_store: 0,
                mem_addr: 0x3000,
                mem_size: 8,
                ..CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default())
            },
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(1),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: None,
            data_ready_visible: Some(10),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: Some(9),
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut pipeline = StageQueues::default();
    pipeline.stq.push_back(0);
    pipeline.e4.push_back(1);
    let mut out = Vec::new();

    emit_stage_events(
        9,
        &runtime,
        &pipeline,
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );

    assert!(
        out.iter()
            .any(|event| event.stage_id == "STQ" && event.row_id == "uop0")
    );
    assert!(out.iter().any(|event| {
        event.stage_id == "E4" && event.row_id == "uop1" && event.cause == "ld_store_forward"
    }));
}

#[test]
fn load_hit_transitions_through_l1d_before_w1() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let mut uops = vec![CycleUop {
        decoded,
        commit: CommitRecord {
            mem_valid: 1,
            mem_is_store: 0,
            mem_addr: 0x4000,
            mem_size: 8,
            ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
        },
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: true,
        is_store: false,
        load_ordinal: Some(0),
        load_store_id: Some(0),
        miss_injected: false,
        redirect_target: None,
        phys_iq: Some(PhysIq::AguIq0),
        pick_wakeup_visible: Some(6),
        data_ready_visible: Some(9),
        miss_pending_until: None,
        e1_cycle: Some(6),
        e4_cycle: Some(8),
        w1_cycle: None,
        done_cycle: None,
    }];
    let mut pipeline = StageQueues::default();
    pipeline.e4.push_back(0);
    pipeline.lhq.push_back(0);

    advance_execute(8, &mut pipeline, &mut uops, &CycleRunOptions::default());
    assert!(pipeline.e4.is_empty());
    assert!(pipeline.w1.is_empty());
    assert_eq!(pipeline.lhq.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(pipeline.l1d.len(), 1);
    assert_eq!(pipeline.l1d[0].seq, 0);
    assert_eq!(pipeline.l1d[0].kind, L1dTxnKind::LoadHit);
    assert_eq!(pipeline.l1d[0].ready_cycle, 9);
    assert_eq!(pipeline.lsid_cache_ptr, 0);

    advance_l1d(8, &mut pipeline);
    assert!(pipeline.w1.is_empty());
    assert_eq!(pipeline.lhq.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(pipeline.lsid_cache_ptr, 0);

    advance_l1d(9, &mut pipeline);
    assert_eq!(pipeline.w1.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert!(pipeline.l1d.is_empty());
    assert!(pipeline.lhq.is_empty());
    assert_eq!(pipeline.lsid_cache_ptr, 1);
}

#[test]
fn scb_ready_entry_moves_through_l1d_drain() {
    let mut pipeline = StageQueues::default();
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord {
            mem_valid: 1,
            mem_is_store: 1,
            mem_addr: 0x4000,
            mem_size: 8,
            ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
        },
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: true,
        load_ordinal: None,
        load_store_id: Some(0),
        miss_injected: false,
        redirect_target: None,
        phys_iq: Some(PhysIq::StdIq0),
        pick_wakeup_visible: Some(8),
        data_ready_visible: Some(8),
        miss_pending_until: None,
        e1_cycle: Some(8),
        e4_cycle: None,
        w1_cycle: None,
        done_cycle: None,
    }];
    pipeline.scb.push_back(ScbEntry {
        seq: 3,
        enqueue_cycle: 9,
    });
    pipeline.scb.clear();
    pipeline.scb.push_back(ScbEntry {
        seq: 0,
        enqueue_cycle: 9,
    });

    advance_scb(10, &mut pipeline, &uops);
    assert!(pipeline.scb.is_empty());
    assert_eq!(pipeline.l1d.len(), 1);
    assert_eq!(pipeline.l1d[0].seq, 0);
    assert_eq!(pipeline.l1d[0].kind, L1dTxnKind::StoreDrain);
    assert_eq!(pipeline.l1d[0].ready_cycle, 11);

    advance_l1d(10, &mut pipeline);
    assert_eq!(pipeline.l1d.len(), 1);
    assert_eq!(pipeline.lsid_cache_ptr, 0);

    advance_l1d(11, &mut pipeline);
    assert!(pipeline.l1d.is_empty());
    assert_eq!(pipeline.lsid_cache_ptr, 1);
}

#[test]
fn load_hit_waits_for_cache_owner_turn() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let mut uops = vec![CycleUop {
        decoded,
        commit: CommitRecord {
            mem_valid: 1,
            mem_is_store: 0,
            mem_addr: 0x4100,
            mem_size: 8,
            ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
        },
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: true,
        is_store: false,
        load_ordinal: Some(1),
        load_store_id: Some(1),
        miss_injected: false,
        redirect_target: None,
        phys_iq: Some(PhysIq::AguIq0),
        pick_wakeup_visible: Some(6),
        data_ready_visible: Some(9),
        miss_pending_until: None,
        e1_cycle: Some(6),
        e4_cycle: Some(8),
        w1_cycle: None,
        done_cycle: None,
    }];
    let mut pipeline = StageQueues::default();
    pipeline.e4.push_back(0);
    pipeline.lhq.push_back(0);
    pipeline.lsid_cache_ptr = 0;

    advance_execute(8, &mut pipeline, &mut uops, &CycleRunOptions::default());
    assert_eq!(pipeline.e4.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert!(pipeline.l1d.is_empty());

    pipeline.lsid_cache_ptr = 1;
    advance_execute(8, &mut pipeline, &mut uops, &CycleRunOptions::default());
    assert!(pipeline.e4.is_empty());
    assert_eq!(pipeline.l1d.len(), 1);
    assert_eq!(pipeline.l1d[0].seq, 0);
}

#[test]
fn scb_drain_waits_for_cache_owner_turn() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord {
            mem_valid: 1,
            mem_is_store: 1,
            mem_addr: 0x4200,
            mem_size: 8,
            ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
        },
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: false,
        is_store: true,
        load_ordinal: None,
        load_store_id: Some(1),
        miss_injected: false,
        redirect_target: None,
        phys_iq: Some(PhysIq::StdIq0),
        pick_wakeup_visible: Some(8),
        data_ready_visible: Some(8),
        miss_pending_until: None,
        e1_cycle: Some(8),
        e4_cycle: None,
        w1_cycle: None,
        done_cycle: None,
    }];
    let mut pipeline = StageQueues::default();
    pipeline.scb.push_back(ScbEntry {
        seq: 0,
        enqueue_cycle: 9,
    });
    pipeline.lsid_cache_ptr = 0;

    advance_scb(10, &mut pipeline, &uops);
    assert_eq!(pipeline.scb.len(), 1);
    assert!(pipeline.l1d.is_empty());

    pipeline.lsid_cache_ptr = 1;
    advance_scb(10, &mut pipeline, &uops);
    assert!(pipeline.scb.is_empty());
    assert_eq!(pipeline.l1d.len(), 1);
    assert_eq!(pipeline.l1d[0].seq, 0);
}

#[test]
fn emit_stage_events_includes_l1d_transactions() {
    let runtime = sample_runtime(&[enc_addi(2, 0, 1)], &[]);
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![CycleUop {
        decoded,
        commit: CommitRecord {
            mem_valid: 1,
            mem_is_store: 0,
            mem_addr: 0x4000,
            mem_size: 8,
            ..CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default())
        },
        deps: [None, None],
        src_queue_kinds: [None, None],
        src_logical_tags: [None, None],
        src_qtags: [None, None],
        dst_queue_kind: None,
        dst_logical_tag: None,
        dst_qtag: None,
        bypass_d2: false,
        is_load: true,
        is_store: false,
        load_ordinal: Some(0),
        load_store_id: Some(0),
        miss_injected: false,
        redirect_target: None,
        phys_iq: Some(PhysIq::AguIq0),
        pick_wakeup_visible: Some(6),
        data_ready_visible: Some(9),
        miss_pending_until: None,
        e1_cycle: Some(6),
        e4_cycle: Some(8),
        w1_cycle: None,
        done_cycle: None,
    }];
    let mut pipeline = StageQueues::default();
    pipeline.l1d.push_back(L1dEntry {
        seq: 0,
        kind: L1dTxnKind::LoadHit,
        ready_cycle: 9,
    });
    let mut out = Vec::new();

    emit_stage_events(
        9,
        &runtime,
        &pipeline,
        &[],
        &VecDeque::new(),
        &uops,
        &mut out,
    );

    assert!(out.iter().any(|event| {
        event.stage_id == "L1D" && event.row_id == "uop0" && event.cause == "load_hit_resp"
    }));
}

#[test]
fn p1_winners_stay_live_when_i1_is_full() {
    let mut i1 = VecDeque::from([10usize, 11, 12, 13]);
    let mut admitted_i1 = VecDeque::from([20usize, 21]);
    let mut p1 = VecDeque::new();

    advance_p1_to_i1(&mut i1, &mut admitted_i1, &mut p1);

    assert_eq!(i1.iter().copied().collect::<Vec<_>>(), vec![10, 11, 12, 13]);
    assert!(admitted_i1.is_empty());
    assert_eq!(p1.iter().copied().collect::<Vec<_>>(), vec![20, 21]);
}

#[test]
fn advance_i2_respects_single_load_slot() {
    let decoded = isa::decode_word(enc_addi(2, 0, 1) as u64).expect("decode addi");
    let uops = vec![
        CycleUop {
            decoded: decoded.clone(),
            commit: CommitRecord::unsupported(0, 0x1000, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(0),
            load_store_id: Some(0),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(5),
            data_ready_visible: Some(8),
            miss_pending_until: None,
            e1_cycle: Some(4),
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
        CycleUop {
            decoded,
            commit: CommitRecord::unsupported(0, 0x1004, 0, 4, &isa::BlockMeta::default()),
            deps: [None, None],
            src_queue_kinds: [None, None],
            src_logical_tags: [None, None],
            src_qtags: [None, None],
            dst_queue_kind: None,
            dst_logical_tag: None,
            dst_qtag: None,
            bypass_d2: false,
            is_load: true,
            is_store: false,
            load_ordinal: Some(1),
            load_store_id: Some(1),
            miss_injected: false,
            redirect_target: None,
            phys_iq: Some(PhysIq::AguIq0),
            pick_wakeup_visible: Some(5),
            data_ready_visible: Some(8),
            miss_pending_until: None,
            e1_cycle: None,
            e4_cycle: None,
            w1_cycle: None,
            done_cycle: None,
        },
    ];
    let mut i2 = VecDeque::from([1usize]);
    let mut e1 = VecDeque::from([0usize]);
    let mut lhq = VecDeque::from([0usize]);
    let mut stq = VecDeque::new();

    let mut lsid_issue_ptr = 0usize;
    let mut lsid_complete_ptr = 0usize;
    advance_i2(
        8,
        &mut i2,
        &mut e1,
        &mut lhq,
        &mut stq,
        &mut lsid_issue_ptr,
        &mut lsid_complete_ptr,
        &uops,
    );

    assert_eq!(e1.iter().copied().collect::<Vec<_>>(), vec![0]);
    assert_eq!(i2.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(lhq.iter().copied().collect::<Vec<_>>(), vec![0]);
}

#[test]
fn d1_stalls_when_rob_group_would_overflow() {
    let mut pipeline = StageQueues::default();
    pipeline.frontend[6].push_back(200);
    pipeline.frontend[6].push_back(201);
    let mut rob = (0..(ROB_CAPACITY - 1))
        .map(|idx| idx + 1000)
        .collect::<VecDeque<_>>();

    advance_frontend(&mut pipeline, &mut rob);

    assert_eq!(
        pipeline.frontend[6].iter().copied().collect::<Vec<_>>(),
        vec![200, 201]
    );
    assert_eq!(pipeline.frontend[7].len(), 0);
    assert_eq!(rob.len(), ROB_CAPACITY - 1);
}

fn sample_runtime(words: &[u32], extra_regions: &[MemoryRegion]) -> GuestRuntime {
    let text_base = 0x1000u64;
    let mut text = Vec::with_capacity(words.len() * 4);
    for word in words {
        text.extend_from_slice(&word.to_le_bytes());
    }

    let mut regions = vec![MemoryRegion {
        base: text_base,
        size: 0x1000,
        flags: 0b101,
        data: {
            let mut bytes = vec![0; 0x1000];
            bytes[..text.len()].copy_from_slice(&text);
            bytes
        },
    }];
    regions.extend_from_slice(extra_regions);
    regions.push(MemoryRegion {
        base: 0x0000_7FFF_E000,
        size: 0x2000,
        flags: 0b110,
        data: vec![0; 0x2000],
    });

    GuestRuntime {
        image: LoadedElf {
            path: PathBuf::from("sample.elf"),
            entry: text_base,
            little_endian: true,
            bits: 64,
            machine: 0,
            segments: vec![SegmentImage {
                vaddr: text_base,
                mem_size: text.len() as u64,
                file_size: text.len() as u64,
                flags: 0b101,
                data: text,
            }],
        },
        config: RuntimeConfig::default(),
        state: isa::ArchitecturalState::new(text_base),
        block: isa::BlockMeta::default(),
        memory: GuestMemory { regions },
        boot: BootInfo {
            entry_pc: text_base,
            stack_top: 0x0000_7FFF_F000,
            stack_pointer: 0x0000_7FFF_F000,
            argc: 0,
        },
        fd_table: HashMap::from([(0, 0), (1, 1), (2, 2)]),
    }
}

fn enc_addi(rd: u32, rs1: u32, imm: u32) -> u32 {
    ((imm & 0x0fff) << 20) | (rs1 << 15) | (rd << 7) | 0x15
}

fn enc_acrc(rst_type: u32) -> u32 {
    ((rst_type & 0xf) << 20) | 0x302b
}
