//! Property tests for every invariant in `dev/DIRECTIVES.md` §4 that v0.2.0
//! can check, plus the builder's guarantees.
//!
//! - Encoding round-trips exactly: `decode(encode(m)) == m` for arbitrary
//!   modules (every opcode, every modifier, every table), and
//!   `encode(decode(b)) == b` for every input the decoder accepts.
//! - Encoding and disassembly are deterministic.
//! - The decoder never panics on arbitrary bytes, on valid headers with
//!   arbitrary bodies, or on mutated valid encodings, and honours its limits.
//! - Builder label resolution matches a reference resolver, and every
//!   built branch lands on an instruction.
//! - `parallel_move` matches the parallel-assignment reference on random
//!   move sets, cycles included, with exactly one extra move per cycle.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::{BTreeMap, BTreeSet};

use bytecode_lang::{
    BuildError, DecodeErrorKind, FORMAT_VERSION, Inst, Limit, Limits, MAGIC, ModuleBuilder, Reg,
    Target, ValType, decode, decode_with, disassemble, encode,
};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn instruction_words_round_trip(inst in common::inst(64)) {
        let bytes = inst.to_bytes();
        prop_assert_eq!(Inst::from_bytes(bytes), Ok(inst));
        prop_assert_eq!(inst.to_bytes(), bytes);
    }

    #[test]
    fn modules_round_trip_exactly(spec in common::module_spec()) {
        let module = common::build(&spec);
        let bytes = encode(&module);
        let back = decode(&bytes).unwrap();
        prop_assert_eq!(&back, &module);
        // Canonical: re-encoding the decoded module reproduces the bytes.
        prop_assert_eq!(encode(&back), bytes);
    }

    #[test]
    fn encoding_and_disassembly_are_deterministic(spec in common::module_spec()) {
        let a = common::build(&spec);
        let b = common::build(&spec);
        prop_assert_eq!(encode(&a), encode(&b));
        let text = disassemble(&a);
        prop_assert_eq!(&text, &disassemble(&b));
        prop_assert_eq!(&text, &disassemble(&decode(&encode(&a)).unwrap()));
        // Every instruction appears in the listing by mnemonic.
        for f in a.functions() {
            for inst in f.code() {
                prop_assert!(text.contains(inst.mnemonic()));
            }
        }
    }

    #[test]
    fn built_branches_land_on_instructions(spec in common::module_spec()) {
        let module = common::build(&spec);
        for f in module.functions() {
            let len = f.code().len() as u32;
            for inst in f.code() {
                if let Some(Target(t)) = inst.branch_target() {
                    prop_assert!(t < len);
                }
            }
            for table in f.tables() {
                prop_assert!(table.default.0 < len);
                prop_assert!(table.targets.iter().all(|t| t.0 < len));
            }
            for h in f.handlers() {
                prop_assert!(h.start <= h.end && h.end <= len && h.target.0 < len);
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

    /// The fuzz-style property: raw bytes never panic the decoder, and
    /// anything accepted is canonical.
    #[test]
    fn decoder_survives_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        if let Ok(module) = decode(&bytes) {
            prop_assert_eq!(encode(&module), bytes);
        }
    }

    /// Arbitrary bodies behind a valid header reach the section parsers.
    #[test]
    fn decoder_survives_valid_headers(tail in prop::collection::vec(any::<u8>(), 0..512)) {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&tail);
        if let Ok(module) = decode(&bytes) {
            prop_assert_eq!(encode(&module), bytes);
        }
    }

    /// Plausible section framing (ids in order, short lengths) with random
    /// payloads reaches deep into every section parser.
    #[test]
    fn decoder_survives_framed_sections(
        payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..24), 10),
    ) {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        for (id, p) in (1u32..).zip(&payloads) {
            bytes.extend_from_slice(&id.to_le_bytes());
            bytes.extend_from_slice(&(p.len() as u32).to_le_bytes());
            bytes.extend_from_slice(p);
        }
        if let Ok(module) = decode(&bytes) {
            prop_assert_eq!(encode(&module), bytes);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1024, ..ProptestConfig::default() })]

    #[test]
    fn decoder_survives_mutated_modules(
        spec in common::module_spec(),
        flips in prop::collection::vec((any::<usize>(), any::<u8>()), 1..6),
        cut in any::<usize>(),
        insert in prop::option::of((any::<usize>(), any::<u8>())),
    ) {
        let valid = encode(&common::build(&spec));
        // Byte substitutions.
        let mut flipped = valid.clone();
        for &(at, byte) in &flips {
            let i = at % flipped.len();
            flipped[i] = byte;
        }
        if let Ok(module) = decode(&flipped) {
            prop_assert_eq!(encode(&module), flipped);
        }
        // Truncation: every strict prefix is refused.
        let prefix = &valid[..cut % valid.len()];
        prop_assert!(decode(prefix).is_err());
        // Insertion.
        if let Some((at, byte)) = insert {
            let mut grown = valid.clone();
            grown.insert(at % (grown.len() + 1), byte);
            if let Ok(module) = decode(&grown) {
                prop_assert_eq!(encode(&module), grown);
            }
        }
    }

    #[test]
    fn decoder_honours_limits(
        spec in common::module_spec(),
        max_strings in 0usize..6,
        max_types in 0usize..5,
        max_consts in 0usize..8,
        max_functions in 0usize..4,
        max_insts in 0usize..24,
        max_items in 0usize..6,
        max_const_depth in 0u32..4,
    ) {
        let bytes = encode(&common::build(&spec));
        let mut limits = Limits::default();
        limits.max_strings = max_strings;
        limits.max_types = max_types;
        limits.max_consts = max_consts;
        limits.max_functions = max_functions;
        limits.max_insts = max_insts;
        limits.max_total_insts = max_insts * 2;
        limits.max_items = max_items;
        limits.max_const_depth = max_const_depth;
        match decode_with(&bytes, &limits) {
            Ok(m) => {
                prop_assert!(m.string_count() <= max_strings);
                prop_assert!(m.types().len() <= max_types);
                prop_assert!(m.consts().len() <= max_consts);
                prop_assert!(m.functions().len() <= max_functions);
                prop_assert!(m.imports().len() <= max_items);
                prop_assert!(m.globals().len() <= max_items);
                prop_assert!(m.exports().len() <= max_items);
                let total: usize = m.functions().iter().map(|f| f.code().len()).sum();
                prop_assert!(total <= max_insts * 2);
                for f in m.functions() {
                    prop_assert!(f.code().len() <= max_insts);
                    prop_assert!(f.regs().len() <= max_items);
                    prop_assert!(f.handlers().len() <= max_items);
                }
                if !m.consts().is_empty() {
                    prop_assert!(max_const_depth >= 1);
                }
            }
            Err(e) => {
                let ok = matches!(e.kind(), DecodeErrorKind::LimitExceeded(_));
                prop_assert!(ok, "expected a limit error, got {}", e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Builder label resolution against a reference resolver
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Step {
    Nop,
    NewLabel,
    Bind(usize),
    Jmp(usize),
    JmpIf(usize),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => Just(Step::Nop),
        2 => Just(Step::NewLabel),
        2 => any::<usize>().prop_map(Step::Bind),
        2 => any::<usize>().prop_map(Step::Jmp),
        1 => any::<usize>().prop_map(Step::JmpIf),
    ]
}

/// What the builder must produce for a script: the reference resolver.
#[derive(Debug, PartialEq)]
enum Expected {
    Ok(Vec<Inst>),
    Rebound,
    Unbound,
    OutOfRange,
}

fn reference(steps: &[Step]) -> Expected {
    let mut labels: Vec<Option<u32>> = Vec::new();
    let mut code: Vec<(Inst, Option<usize>)> = Vec::new();
    let mut rebound = false;
    for s in steps {
        match *s {
            Step::Nop => code.push((Inst::Nop {}, None)),
            Step::NewLabel => labels.push(None),
            Step::Bind(i) if !labels.is_empty() => {
                let i = i % labels.len();
                if labels[i].is_some() {
                    rebound = true;
                } else {
                    labels[i] = Some(code.len() as u32);
                }
            }
            Step::Jmp(i) if !labels.is_empty() => {
                code.push((Inst::Jmp { target: Target(0) }, Some(i % labels.len())));
            }
            Step::JmpIf(i) if !labels.is_empty() => {
                code.push((
                    Inst::JmpIf {
                        cond: Reg(0),
                        target: Target(0),
                    },
                    Some(i % labels.len()),
                ));
            }
            _ => {}
        }
    }
    if rebound {
        return Expected::Rebound;
    }
    let len = code.len() as u32;
    let mut out = Vec::new();
    for (inst, label) in code {
        let Some(l) = label else {
            out.push(inst);
            continue;
        };
        let Some(pc) = labels[l] else {
            return Expected::Unbound;
        };
        if pc >= len {
            return Expected::OutOfRange;
        }
        out.push(match inst {
            Inst::Jmp { .. } => Inst::Jmp { target: Target(pc) },
            _ => Inst::JmpIf {
                cond: Reg(0),
                target: Target(pc),
            },
        });
    }
    Expected::Ok(out)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    #[test]
    fn labels_resolve_like_the_reference(steps in prop::collection::vec(step(), 0..40)) {
        let mut m = ModuleBuilder::new();
        let mut f = m.function("f", &[ValType::Bool], &[]);
        let mut labels = Vec::new();
        for s in &steps {
            match *s {
                Step::Nop => { f.emit(Inst::Nop {}); }
                Step::NewLabel => labels.push(f.label()),
                Step::Bind(i) if !labels.is_empty() => f.bind(labels[i % labels.len()]),
                Step::Jmp(i) if !labels.is_empty() => { f.jmp(labels[i % labels.len()]); }
                Step::JmpIf(i) if !labels.is_empty() => { f.jmp_if(Reg(0), labels[i % labels.len()]); }
                _ => {}
            }
        }
        let got = m.add_function(f);
        match reference(&steps) {
            Expected::Ok(code) => {
                let id = got.unwrap();
                let module = m.finish().unwrap();
                prop_assert_eq!(module.function(id).unwrap().code(), code.as_slice());
            }
            // (`prop_assert!` formats its condition, so the patterns are
            // matched outside it.)
            Expected::Rebound => {
                let ok = matches!(got, Err(BuildError::LabelRebound { .. }));
                prop_assert!(ok, "expected LabelRebound, got {:?}", got);
            }
            Expected::Unbound => {
                let ok = matches!(got, Err(BuildError::UnboundLabel { .. }));
                prop_assert!(ok, "expected UnboundLabel, got {:?}", got);
            }
            Expected::OutOfRange => {
                let ok = matches!(got, Err(BuildError::TargetOutOfRange { .. }));
                prop_assert!(ok, "expected TargetOutOfRange, got {:?}", got);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parallel moves against parallel assignment
// ---------------------------------------------------------------------------

/// Destinations unique (first wins), registers among the first `regs`.
fn move_set(regs: u16) -> impl Strategy<Value = Vec<(Reg, Reg)>> {
    prop::collection::vec((0..regs, 0..regs), 0..12).prop_map(|pairs| {
        let mut seen = BTreeSet::new();
        pairs
            .into_iter()
            .filter(|&(d, _)| seen.insert(d))
            .map(|(d, s)| (Reg(d), Reg(s)))
            .collect()
    })
}

/// The number of cycles of length two or more among the moves.
fn cycles(moves: &[(Reg, Reg)]) -> usize {
    let src_of: BTreeMap<Reg, Reg> = moves.iter().copied().filter(|(d, s)| d != s).collect();
    let mut seen = BTreeSet::new();
    let mut count = 0;
    for &start in src_of.keys() {
        if seen.contains(&start) {
            continue;
        }
        // Walk dst -> src while the source is itself a destination.
        let mut path = Vec::new();
        let mut on_path = BTreeSet::new();
        let mut r = start;
        while let Some(&s) = src_of.get(&r) {
            if seen.contains(&r) {
                break;
            }
            if !on_path.insert(r) {
                count += 1; // came back around to the current path
                break;
            }
            path.push(r);
            r = s;
        }
        seen.extend(path);
    }
    count
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

    #[test]
    fn parallel_moves_match_parallel_assignment(moves in move_set(10)) {
        // Eight declared registers, alternating i64 and dyn. A first swap
        // of each type declares the cached temporaries r8 (i64) and r9
        // (dyn); the random move set then ranges over r0..r9, so it often
        // names those temporaries, which the builder must not reuse.
        const REGS: u16 = 10;
        let mut m = ModuleBuilder::new();
        let mut f = m.function("f", &[], &[]);
        for i in 0..8u16 {
            let _ = f.reg(if i % 2 == 0 { ValType::I64 } else { ValType::Dyn });
        }
        f.parallel_move(&[(Reg(0), Reg(2)), (Reg(2), Reg(0)), (Reg(1), Reg(3)), (Reg(3), Reg(1))]);
        let start = f.pc() as usize;
        // Same-type moves only (r8 is even, i64; r9 odd, dyn): mixing types
        // is the verifier's concern.
        let moves: Vec<_> = moves.into_iter().filter(|(d, s)| d.0 % 2 == s.0 % 2).collect();
        f.parallel_move(&moves);
        f.ret_void();
        let id = m.add_function(f).unwrap();
        let module = m.finish().unwrap();
        let func = module.function(id).unwrap();
        prop_assert!(func.regs().len() >= REGS as usize);

        // Run the second set's moves on a register file.
        let mut regs: Vec<u64> = (0..func.regs().len() as u64).map(|i| 1000 + i).collect();
        let before = regs.clone();
        let mut emitted = 0;
        for inst in &func.code()[start..] {
            if let Inst::Mov { dst, src } = *inst {
                // Moves never mix types, temporaries included.
                prop_assert_eq!(func.regs()[dst.index()], func.regs()[src.index()]);
                regs[dst.index()] = regs[src.index()];
                emitted += 1;
            }
        }
        // Parallel assignment: each destination gets its source's old value,
        // and no register the set names is used as scratch.
        let mut expected = before.clone();
        for &(d, s) in &moves {
            expected[d.index()] = before[s.index()];
        }
        // The user's registers r0..r7 must match exactly; the temporaries
        // r8 and r9 must too whenever the set names them (when it does not,
        // the builder may use them as scratch, which is their purpose).
        let named: BTreeSet<Reg> = moves.iter().flat_map(|&(d, s)| [d, s]).collect();
        for r in 0..REGS {
            if r < 8 || named.contains(&Reg(r)) {
                prop_assert_eq!(regs[r as usize], expected[r as usize], "register r{}", r);
            }
        }
        // Optimal: one move per non-self move plus one per cycle.
        let real = moves.iter().filter(|(d, s)| d != s).count();
        prop_assert_eq!(emitted, real + cycles(&moves));
    }

    #[test]
    fn promote_is_refused_exactly_on_declared_static_destinations(
        declared in common::val_type(),
        undeclared in any::<bool>(),
        dynamic_op in any::<bool>(),
    ) {
        use bytecode_lang::{IntOp, IntTy, Overflow, Policy};
        let promote = Policy::new().with_overflow(Overflow::Promote);
        let mut m = ModuleBuilder::new();
        let mut f = m.function("f", &[], &[]);
        let dst = if undeclared { Reg(50) } else { f.reg(declared) };
        let inst = if dynamic_op {
            Inst::DSub { dst, lhs: dst, rhs: dst, pol: promote }
        } else {
            Inst::ISub { dst, lhs: dst, rhs: dst, op: IntOp::new(IntTy::I32).with_policy(promote) }
        };
        f.emit(inst);
        f.ret_void();
        let refused = matches!(m.add_function(f), Err(BuildError::PromoteNotDynamic { pc: 0, .. }));
        prop_assert_eq!(refused, !undeclared && declared != ValType::Dyn);
    }
}

#[test]
fn parallel_move_rejects_two_writes_to_one_register() {
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::I64, ValType::I64], &[]);
    f.parallel_move(&[
        (Reg(0), Reg(1)),
        (Reg(0), Reg(0)),
        (Reg(1), Reg(0)),
        (Reg(1), Reg(1)),
    ]);
    // (r0, r0) and (r1, r1) are self-moves and dropped, so this is a swap.
    f.ret_void();
    assert!(m.add_function(f).is_ok());

    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::I64, ValType::I64], &[]);
    f.parallel_move(&[(Reg(0), Reg(1)), (Reg(0), Reg(1))]);
    f.ret_void();
    assert!(matches!(
        m.add_function(f),
        Err(BuildError::ConflictingMoves { dst: Reg(0), .. })
    ));
}

#[test]
fn parallel_move_scales_on_a_65k_cycle() {
    // A rotation r0 <- r1 <- ... <- r64999 <- r0: one cycle as long as a
    // frame allows (65,536 registers).
    const N: u16 = 65_000;
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    let _ = f.regs(&vec![ValType::I64; N as usize]);
    let moves: Vec<_> = (0..N).map(|i| (Reg(i), Reg((i + 1) % N))).collect();
    let start = std::time::Instant::now();
    f.parallel_move(&moves);
    f.ret_void();
    let id = m.add_function(f).unwrap();
    let elapsed = start.elapsed();
    let module = m.finish().unwrap();
    // N moves plus one to save the cycle, plus the return.
    assert_eq!(module.function(id).unwrap().code().len(), N as usize + 2);
    // A quadratic algorithm would take seconds here; this is a loose bound.
    assert!(elapsed.as_secs() < 5, "parallel_move took {elapsed:?}");
}

#[test]
fn limits_name_the_budget_they_hit() {
    let spec_module = {
        let mut m = ModuleBuilder::new();
        let mut f = m.function("f", &[], &[]);
        for _ in 0..10 {
            f.emit(Inst::Nop {});
        }
        f.ret_void();
        m.add_function(f).unwrap();
        m.finish().unwrap()
    };
    let bytes = encode(&spec_module);
    let mut limits = Limits::default();
    limits.max_insts = 10;
    assert_eq!(
        decode_with(&bytes, &limits).unwrap_err().kind(),
        &DecodeErrorKind::LimitExceeded(Limit::Insts)
    );
    limits.max_insts = 11;
    assert!(decode_with(&bytes, &limits).is_ok());
    limits.max_total_insts = 10;
    assert_eq!(
        decode_with(&bytes, &limits).unwrap_err().kind(),
        &DecodeErrorKind::LimitExceeded(Limit::TotalInsts)
    );
}
