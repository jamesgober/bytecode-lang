//! The instruction table: metadata consistency, exhaustive encoding checks
//! over every opcode, and the documentation listing every instruction.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeSet;

use bytecode_lang::{FieldKind, Inst, InstError, IntOp, IntTy, Opcode, Reg, Slot, Target};

#[test]
fn decoded_instructions_are_eight_bytes() {
    assert_eq!(std::mem::size_of::<Inst>(), 8);
    assert_eq!(std::mem::size_of::<Opcode>(), 1);
}

#[test]
fn the_table_has_200_opcodes_with_unique_bytes_and_names() {
    assert_eq!(Opcode::ALL.len(), 200);
    let bytes: BTreeSet<u8> = Opcode::ALL.iter().map(|&op| op as u8).collect();
    let names: BTreeSet<&str> = Opcode::ALL.iter().map(|op| op.mnemonic()).collect();
    assert_eq!(bytes.len(), Opcode::ALL.len());
    assert_eq!(names.len(), Opcode::ALL.len());
    for byte in 0..=u8::MAX {
        match Opcode::from_u8(byte) {
            Some(op) => assert_eq!(op as u8, byte),
            None => assert!(!bytes.contains(&byte)),
        }
    }
}

#[test]
fn every_field_fits_its_slot_and_no_slots_overlap() {
    for &op in Opcode::ALL {
        let mut used: Vec<Slot> = Vec::new();
        let mut names = BTreeSet::new();
        for field in op.fields() {
            assert_eq!(
                field.kind.bits(),
                field.slot.width(),
                "{op:?}.{} ({:?}) does not fit slot {:?}",
                field.name,
                field.kind,
                field.slot
            );
            assert!(
                names.insert(field.name),
                "{op:?} repeats field {}",
                field.name
            );
            for &other in &used {
                let overlap = other == field.slot
                    || matches!(
                        (other, field.slot),
                        (Slot::W, Slot::C | Slot::D) | (Slot::C | Slot::D, Slot::W)
                    );
                assert!(!overlap, "{op:?}: {:?} overlaps {other:?}", field.slot);
            }
            used.push(field.slot);
        }
    }
}

/// The largest valid raw value for a field, so every bit a field may set
/// is exercised.
fn max_raw(kind: FieldKind) -> u32 {
    match kind {
        FieldKind::Reg
        | FieldKind::Name
        | FieldKind::TypeRef
        | FieldKind::Field
        | FieldKind::Upval
        | FieldKind::Shape => 0xffff,
        FieldKind::Target
        | FieldKind::Const
        | FieldKind::Func
        | FieldKind::Import
        | FieldKind::Global
        | FieldKind::Table
        | FieldKind::Imm32 => u32::MAX,
        FieldKind::Count => 0xff,
        FieldKind::Bool => 1,
        FieldKind::IntTy => 7,
        FieldKind::FloatTy => 1,
        FieldKind::Kind => 14,
        FieldKind::Prim => 13,
        // overflow = promote (3), div_zero = trap, shift = saturate (2),
        // float_to_int = saturate.
        FieldKind::Policy => 0b11_0111,
        // u64 (7), promote, divtrap, shift = saturate.
        FieldKind::IntOp => 0b1011_1111,
        FieldKind::IntConv => 0xff,
        FieldKind::IntPair => 0b0011_1111,
        FieldKind::FloatConv => 0b1111,
        // NoMatch, the largest code.
        FieldKind::ErrKind => 200,
    }
}

/// The smallest valid raw value for a field: zero, except for an error
/// kind, whose smallest code is 1 (`ArithOverflow`).
fn min_raw(kind: FieldKind) -> u32 {
    u32::from(kind == FieldKind::ErrKind)
}

#[test]
fn every_opcode_round_trips_at_zero_and_at_maximum() {
    for &op in Opcode::ALL {
        let fields = op.fields();
        for raws in [
            fields.iter().map(|f| min_raw(f.kind)).collect::<Vec<u32>>(),
            fields.iter().map(|f| max_raw(f.kind)).collect(),
        ] {
            let bytes = common::assemble(op, &raws);
            let inst = Inst::from_bytes(bytes).unwrap_or_else(|e| panic!("{op:?}: {e}"));
            assert_eq!(inst.opcode(), op);
            assert_eq!(inst.to_bytes(), bytes, "{op:?}");
        }
    }
}

#[test]
fn every_unused_byte_must_be_zero() {
    for &op in Opcode::ALL {
        let mut used = [false; 8];
        used[0] = true;
        for f in op.fields() {
            let first = (f.slot.shift() / 8) as usize;
            let len = (f.slot.width() / 8) as usize;
            for b in &mut used[first..first + len] {
                *b = true;
            }
        }
        let base = common::assemble(op, &vec![0; op.fields().len()]);
        for (i, &u) in used.iter().enumerate() {
            if !u {
                let mut bytes = base;
                bytes[i] = 0x01;
                assert_eq!(
                    Inst::from_bytes(bytes),
                    Err(InstError::NonCanonical(op)),
                    "{op:?} byte {i}"
                );
            }
        }
    }
}

#[test]
fn invalid_modifier_values_are_refused() {
    let mut checked = 0;
    for &op in Opcode::ALL {
        for (i, f) in op.fields().iter().enumerate() {
            // Counts and `IntConv` have no invalid byte: every value names
            // a count, or two types and an overflow policy.
            if f.slot != Slot::A || matches!(f.kind, FieldKind::Count | FieldKind::IntConv) {
                continue;
            }
            // 0xff is invalid for every byte-sized kind except counts.
            let mut raws = vec![0; op.fields().len()];
            raws[i] = 0xff;
            assert_eq!(
                Inst::from_bytes(common::assemble(op, &raws)),
                Err(InstError::InvalidOperand {
                    opcode: op,
                    field: f.name
                }),
            );
            checked += 1;
        }
    }
    // 83 opcodes carry a modifier byte that has invalid values: an integer
    // or float type, a `Policy`, an `IntOp` (shift code 3), a `FloatConv`, an
    // `IntPair`, a kind, a prim, an error kind, or a flag.
    assert_eq!(checked, 83);
}

#[test]
fn unknown_opcodes_are_refused() {
    for byte in 0..=u8::MAX {
        if Opcode::from_u8(byte).is_none() {
            assert_eq!(
                Inst::from_bytes([byte, 0, 0, 0, 0, 0, 0, 0]),
                Err(InstError::UnknownOpcode(byte))
            );
        }
    }
}

#[test]
fn errors_describe_the_problem() {
    assert_eq!(
        InstError::UnknownOpcode(0xff).to_string(),
        "unknown opcode 0xff"
    );
    assert_eq!(
        InstError::InvalidOperand {
            opcode: Opcode::IAdd,
            field: "op"
        }
        .to_string(),
        "invalid `op` operand of `iadd`"
    );
    assert_eq!(
        InstError::NonCanonical(Opcode::Nop).to_string(),
        "`nop` has non-zero bytes in unused slots"
    );
}

#[test]
fn display_uses_mnemonic_modifiers_and_operands() {
    let cases = [
        (Inst::Nop {}, "nop"),
        (Inst::Jmp { target: Target(7) }, "jmp @7"),
        (
            Inst::IShl {
                dst: Reg(0),
                lhs: Reg(1),
                rhs: Reg(2),
                op: IntOp::new(IntTy::U32).with_policy(
                    bytecode_lang::Policy::new().with_shift(bytecode_lang::Shift::Mask),
                ),
            },
            "ishl.u32.mask r0, r1, r2",
        ),
        (
            Inst::LoadBool {
                dst: Reg(4),
                val: true,
            },
            "load_bool r4, true",
        ),
        (
            Inst::IsKind {
                dst: Reg(0),
                src: Reg(1),
                kind: bytecode_lang::Kind::Str,
            },
            "is_kind.str r0, r1",
        ),
        (
            Inst::GetProp {
                dst: Reg(0),
                obj: Reg(1),
                name: bytecode_lang::NameRef(3),
            },
            "get_prop r0, r1, n3",
        ),
        (
            Inst::StrConcatN {
                dst: Reg(0),
                first: Reg(1),
                count: 3,
            },
            "str_concat_n r0, r1, 3",
        ),
    ];
    for (inst, text) in cases {
        assert_eq!(inst.to_string(), text);
    }
}

#[test]
fn branch_targets_are_reported_only_for_branches() {
    for &op in Opcode::ALL {
        let raws: Vec<u32> = op.fields().iter().map(|_| 1).collect(); // 1 is valid for every kind
        let inst = Inst::from_bytes(common::assemble(op, &raws)).unwrap();
        let has_target = op.fields().iter().any(|f| f.kind == FieldKind::Target);
        assert_eq!(inst.branch_target().is_some(), has_target, "{op:?}");
    }
}

#[test]
fn the_api_reference_lists_every_instruction() {
    let api = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/API.md")).unwrap();
    for &op in Opcode::ALL {
        let row = format!("| `{}` |", op.mnemonic());
        assert!(
            api.contains(&row),
            "docs/API.md has no row for `{}`",
            op.mnemonic()
        );
        let hex = format!("0x{:02X}", op as u8);
        assert!(api.contains(&hex), "docs/API.md does not list opcode {hex}");
    }
}
