//! What format version 2 added and renamed (bytecode-lang 0.3): every new
//! instruction encoded, decoded, disassembled, and round-tripped through a
//! module; the renamed not/bit-not pair; the repacked policy bytes; the new
//! kinds, hooks, and error kinds.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use bytecode_lang::{
    ArgKind, ErrorKind, FloatConv, FloatToInt, FloatTy, Hook, Inst, IntOp, IntTy, Kind,
    ModuleBuilder, NameRef, Opcode, Overflow, Policy, Reg, ShapeId, Shift, ValType, decode,
    disassemble, encode,
};

fn r(n: u16) -> Reg {
    Reg(n)
}

/// Every instruction format version 2 added, with its listing text.
fn new_instructions() -> Vec<(Inst, &'static str)> {
    let php = Policy::new()
        .with_overflow(Overflow::Promote)
        .with_shift(Shift::Saturate);
    vec![
        (
            Inst::IPow {
                dst: r(0),
                lhs: r(1),
                rhs: r(2),
                op: IntOp::new(IntTy::I32).with_policy(Policy::new().with_overflow(Overflow::Wrap)),
            },
            "ipow.i32.wrap r0, r1, r2",
        ),
        (
            Inst::FPow {
                dst: r(0),
                lhs: r(1),
                rhs: r(2),
                ty: FloatTy::F32,
            },
            "fpow.f32 r0, r1, r2",
        ),
        (
            Inst::DPow {
                dst: r(0),
                lhs: r(1),
                rhs: r(2),
                pol: php,
            },
            "dpow.promote.shsat r0, r1, r2",
        ),
        (
            Inst::DAbs {
                dst: r(0),
                src: r(1),
                pol: Policy::new().with_overflow(Overflow::Promote),
            },
            "dabs.promote r0, r1",
        ),
        (
            Inst::DSepIndex {
                dst: r(3),
                obj: r(1),
                key: r(2),
            },
            "dsep_index r3, r1, r2",
        ),
        (
            Inst::DSepProp {
                dst: r(3),
                obj: r(1),
                name: NameRef(0),
            },
            "dsep_prop r3, r1, n0",
        ),
        (
            Inst::DCallShape {
                dst: r(4),
                callee: r(1),
                shape: ShapeId(0),
            },
            "dcall_shape r4, r1, cs0",
        ),
        (
            Inst::DParamRef {
                dst: r(3),
                callee: r(1),
                pos: r(2),
            },
            "dparam_ref r3, r1, r2",
        ),
        (
            Inst::DParamRefNamed {
                dst: r(3),
                callee: r(1),
                name: NameRef(0),
            },
            "dparam_ref_named r3, r1, n0",
        ),
        (
            Inst::ErrPayload {
                dst: r(0),
                src: r(1),
            },
            "err_payload r0, r1",
        ),
        (
            Inst::Raise {
                src: r(1),
                kind: ErrorKind::NoMatch,
            },
            "raise.E0200 r1  ; NoMatch",
        ),
        (
            Inst::NewRef {
                dst: r(0),
                src: r(1),
            },
            "new_ref r0, r1",
        ),
        (
            Inst::DRefIndex {
                dst: r(0),
                obj: r(1),
                key: r(2),
            },
            "dref_index r0, r1, r2",
        ),
        (
            Inst::DRefProp {
                dst: r(0),
                obj: r(1),
                name: NameRef(0),
            },
            "dref_prop r0, r1, n0",
        ),
        (
            Inst::DBindIndex {
                obj: r(1),
                key: r(2),
                src: r(0),
            },
            "dbind_index r1, r2, r0",
        ),
        (
            Inst::DBindProp {
                obj: r(1),
                name: NameRef(0),
                src: r(0),
            },
            "dbind_prop r1, n0, r0",
        ),
        (
            Inst::DUnrefIndex {
                obj: r(1),
                key: r(2),
            },
            "dunref_index r1, r2",
        ),
        (
            Inst::DUnrefProp {
                obj: r(1),
                name: NameRef(0),
            },
            "dunref_prop r1, n0",
        ),
    ]
}

#[test]
fn every_new_instruction_round_trips_as_a_word() {
    let cases = new_instructions();
    assert_eq!(cases.len(), 18);
    for (inst, text) in cases {
        assert_eq!(Inst::from_bytes(inst.to_bytes()), Ok(inst), "{text}");
        // `Display` has no module, so it prints no name comments.
        let bare = text.split("  ;").next().unwrap();
        assert!(inst.to_string().starts_with(bare), "{inst}");
    }
}

#[test]
fn every_new_instruction_round_trips_through_a_module_and_its_listing() {
    let mut m = ModuleBuilder::new();
    let key = m.string("key");
    let d = ValType::Dyn;
    let mut f = m.function("f", &[], &[]);
    let _regs = f.regs(&[d, d, d, ValType::Bool, d, d]);
    assert_eq!(f.name_ref(key), NameRef(0));
    assert_eq!(
        f.call_shape(&[ArgKind::Positional, ArgKind::Named(key)]),
        ShapeId(0)
    );
    let cases = new_instructions();
    for (inst, _) in &cases {
        f.emit(*inst);
    }
    f.ret_void();
    let id = m.add_function(f).unwrap();
    let module = m.finish().unwrap();

    let bytes = encode(&module);
    let back = decode(&bytes).unwrap();
    assert_eq!(back, module);
    assert_eq!(encode(&back), bytes);
    let code = back.function(id).unwrap().code();
    let listing = disassemble(&back);
    for (pc, (inst, text)) in cases.iter().enumerate() {
        assert_eq!(code[pc], *inst);
        // In a module, names and shapes resolve in comments.
        let line = format!("  {pc:04} {text}");
        assert!(listing.contains(&line), "missing `{line}` in\n{listing}");
    }
    assert!(listing.contains("dsep_prop r3, r1, n0  ; \"key\""));
    assert!(listing.contains("dcall_shape r4, r1, cs0  ; (_, \"key\":)"));
    assert!(listing.contains("  shape cs0 (_, s0:)"));
}

#[test]
fn the_not_pair_is_named_as_in_hir() {
    // HIR's `not` is the logical not, `bit_not` the bitwise complement.
    assert_eq!(Opcode::from_u8(0x94).map(Opcode::mnemonic), Some("dnot"));
    assert_eq!(
        Opcode::from_u8(0x7D).map(Opcode::mnemonic),
        Some("dbit_not")
    );
    assert_eq!(
        Opcode::from_u8(0x1F).map(Opcode::mnemonic),
        Some("ibit_not")
    );
    assert_eq!(Opcode::from_u8(0x50).map(Opcode::mnemonic), Some("bnot"));
    for gone in ["dlnot", "inot"] {
        assert!(Opcode::ALL.iter().all(|op| op.mnemonic() != gone));
    }
    let logical = Inst::DNot {
        dst: r(0),
        src: r(1),
    };
    assert_eq!(logical.to_string(), "dnot r0, r1");
    let bitwise = Inst::DBitNot {
        dst: r(0),
        src: r(1),
        pol: Policy::new(),
    };
    assert_eq!(bitwise.to_string(), "dbit_not r0, r1");
    assert_eq!(Hook::BitNot.to_string(), "bit_not");
}

#[test]
fn shift_saturate_rides_in_every_policy_byte() {
    let sat = Policy::new().with_shift(Shift::Saturate);
    let typed = Inst::IShr {
        dst: r(0),
        lhs: r(1),
        rhs: r(2),
        op: IntOp::new(IntTy::I64).with_policy(sat),
    };
    assert_eq!(typed.to_string(), "ishr.i64.shsat r0, r1, r2");
    assert_eq!(Inst::from_bytes(typed.to_bytes()), Ok(typed));
    let dynamic = Inst::DShl {
        dst: r(0),
        lhs: r(1),
        rhs: r(2),
        pol: sat,
    };
    assert_eq!(dynamic.to_string(), "dshl.shsat r0, r1, r2");
    assert_eq!(Inst::from_bytes(dynamic.to_bytes()), Ok(dynamic));
    // The reserved shift code 3 is refused in both bytes.
    let mut bytes = typed.to_bytes();
    bytes[1] |= 0b1100_0000;
    assert!(Inst::from_bytes(bytes).is_err());
    let mut bytes = dynamic.to_bytes();
    bytes[1] |= 0b0001_1000;
    assert!(Inst::from_bytes(bytes).is_err());
}

#[test]
fn float_to_int_conversions_carry_a_float_conv() {
    let i = Inst::F64ToInt {
        dst: r(0),
        src: r(1),
        conv: FloatConv::new(IntTy::U8).with_float_to_int(FloatToInt::Saturate),
    };
    assert_eq!(i.to_string(), "f64_to_int.u8.sat r0, r1");
    assert_eq!(Inst::from_bytes(i.to_bytes()), Ok(i));
    let mut bytes = i.to_bytes();
    bytes[1] |= 0x10;
    assert!(Inst::from_bytes(bytes).is_err());
}

#[test]
fn raise_accepts_exactly_the_catchable_kinds() {
    let mut accepted = Vec::new();
    for code in 0..=u8::MAX {
        let word = [0xAF, code, 1, 0, 0, 0, 0, 0];
        if let Ok(Inst::Raise { kind, .. }) = Inst::from_bytes(word) {
            accepted.push(kind);
        }
    }
    let catchable: Vec<ErrorKind> = ErrorKind::ALL
        .iter()
        .copied()
        .filter(|k| k.is_catchable())
        .collect();
    assert_eq!(accepted, catchable);
    assert!(accepted.contains(&ErrorKind::NoMatch));
    assert!(!accepted.contains(&ErrorKind::OutOfFuel));
}

#[test]
fn new_kinds_hooks_and_error_codes_are_stable() {
    assert_eq!(Kind::Reference.code(), 14);
    assert_eq!(Kind::from_code(14), Some(Kind::Reference));
    assert_eq!(Kind::Reference.to_string(), "reference");
    let is_ref = Inst::IsKind {
        dst: r(0),
        src: r(1),
        kind: Kind::Reference,
    };
    assert_eq!(is_ref.to_string(), "is_kind.reference r0, r1");
    assert_eq!(Hook::Pow.code(), 28);
    assert_eq!(Hook::Abs.code(), 29);
    assert_eq!(Hook::CallShape.code(), 30);
    assert_eq!(Hook::ALL.len(), 31);
    assert_eq!(ErrorKind::NegativeExponent.code(), 6);
    assert_eq!(ErrorKind::ArgumentError.code(), 114);
    assert_eq!(ErrorKind::NoMatch.code(), 200);
    assert_eq!(ErrorKind::from_code(200), Some(ErrorKind::NoMatch));
    assert!(ErrorKind::NoMatch.is_catchable());
}

#[test]
fn version_one_files_are_refused() {
    let mut bytes = encode(&ModuleBuilder::new().finish().unwrap());
    bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        decode(&bytes).unwrap_err().kind(),
        &bytecode_lang::DecodeErrorKind::UnsupportedVersion(1)
    );
}
