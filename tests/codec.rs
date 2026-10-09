//! Encoding and decoding: every decode error, hand-assembled, plus scale.
//!
//! Inputs are assembled section by section here, independently of the
//! crate's encoder, so these tests also pin the byte layout documented in
//! `specs/LSB.md` §6.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use bytecode_lang::{
    Const, DecodeErrorKind, FORMAT_VERSION, Inst, InstError, IntOp, IntTy, Limit, Limits, MAGIC,
    Module, ModuleBuilder, Reg, ValType, decode, decode_with, encode,
};

fn u32le(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// The ten section payloads of an empty module.
fn empty() -> Vec<Vec<u8>> {
    let mut s = vec![u32le(0).to_vec(); 10];
    s[8] = vec![0, 0]; // meta: no name, no start
    s
}

/// A file from section payloads, ids 1..=10 in order.
fn file(sections: &[Vec<u8>]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend(u32le(FORMAT_VERSION));
    out.extend(u32le(0));
    for (id, p) in (1u32..).zip(sections) {
        out.extend(u32le(id));
        out.extend(u32le(p.len() as u32));
        out.extend(p);
    }
    out
}

fn kind(bytes: &[u8]) -> DecodeErrorKind {
    *decode(bytes).unwrap_err().kind()
}

/// A function payload: name 0, sig 0, no parameter list, the given register
/// tags, no other tables, and the given code.
fn function(regs: &[u8], code: &[[u8; 8]]) -> Vec<u8> {
    function_with(&[0], regs, &[], code)
}

/// A function payload with an encoded parameter list and shape table.
fn function_with(params: &[u8], regs: &[u8], shapes: &[u8], code: &[[u8; 8]]) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend(u32le(0));
    f.extend(u32le(0));
    f.extend(params);
    f.extend(u32le(regs.len() as u32));
    f.extend(regs);
    for _ in 0..4 {
        f.extend(u32le(0)); // captures, names, type refs, tables
    }
    if shapes.is_empty() {
        f.extend(u32le(0));
    } else {
        f.extend(shapes);
    }
    f.extend(u32le(0)); // handlers
    f.extend(u32le(code.len() as u32));
    for w in code {
        f.extend(w);
    }
    f
}

#[test]
fn the_empty_module_has_a_fixed_encoding() {
    let bytes = file(&empty());
    assert_eq!(bytes, encode(&ModuleBuilder::new().finish().unwrap()));
    assert_eq!(decode(&bytes).unwrap(), Module::default());
    assert_eq!(bytes.len(), 130);
}

#[test]
fn a_function_decodes_from_hand_assembled_bytes() {
    let mut s = empty();
    // One i64 register; `ret r0`.
    let ret = Inst::Ret { src: Reg(0) }.to_bytes();
    s[5] = [u32le(1).to_vec(), function(&[4], &[ret])].concat();
    s[9] = [u32le(1), u32le(0), u32le(0)].concat(); // debug: 1 function, no rows
    let module = decode(&file(&s)).unwrap();
    assert_eq!(module.functions()[0].regs(), &[ValType::I64]);
    assert_eq!(module.functions()[0].code(), &[Inst::Ret { src: Reg(0) }]);
    assert_eq!(encode(&module), file(&s));
}

#[test]
fn header_errors() {
    let good = file(&empty());
    let mut bad = good.clone();
    bad[0] = b'X';
    assert_eq!(kind(&bad), DecodeErrorKind::BadMagic);

    for version in [0u32, 1, 3, u32::MAX] {
        let mut bad = good.clone();
        bad[4..8].copy_from_slice(&u32le(version));
        let err = decode(&bad).unwrap_err();
        assert_eq!(err.kind(), &DecodeErrorKind::UnsupportedVersion(version));
        assert_eq!(err.offset(), 4);
    }

    let mut bad = good.clone();
    bad[8] = 1;
    let err = decode(&bad).unwrap_err();
    assert_eq!(err.kind(), &DecodeErrorKind::ReservedFlags(1));
    assert_eq!(err.offset(), 8);
}

#[test]
fn every_strict_prefix_is_refused() {
    let mut m = ModuleBuilder::new();
    let k = m.constant(Const::Int(5));
    let mut f = m.function("f", &[ValType::I64], &[ValType::I64]);
    let x = f.reg(ValType::I64);
    f.emit(Inst::LoadConst { dst: x, k });
    f.emit(Inst::IAdd {
        dst: x,
        lhs: x,
        rhs: f.param(0),
        op: IntOp::new(IntTy::I64),
    });
    f.ret(x);
    m.add_function(f).unwrap();
    let bytes = encode(&m.finish().unwrap());
    for len in 0..bytes.len() {
        assert!(
            decode(&bytes[..len]).is_err(),
            "prefix of {len} bytes accepted"
        );
    }
}

#[test]
fn section_framing_errors() {
    // Section 2 where section 1 belongs.
    let mut bad = file(&empty());
    bad[12..16].copy_from_slice(&u32le(2));
    assert_eq!(
        kind(&bad),
        DecodeErrorKind::SectionOutOfOrder {
            expected: 1,
            found: 2
        }
    );

    // A meta section one byte longer than its contents.
    let mut s = empty();
    s[8] = vec![0, 0, 0];
    assert_eq!(kind(&file(&s)), DecodeErrorKind::SectionLength(9));

    // A section length past the end of the input.
    let mut bad = file(&empty());
    bad[16..20].copy_from_slice(&u32le(1000));
    assert_eq!(kind(&bad), DecodeErrorKind::UnexpectedEnd);

    let mut bad = file(&empty());
    bad.push(0);
    assert_eq!(kind(&bad), DecodeErrorKind::TrailingBytes);
}

#[test]
fn string_errors() {
    let mut s = empty();
    s[0] = [u32le(1).to_vec(), u32le(2).to_vec(), vec![0xc3, 0x28]].concat();
    assert_eq!(kind(&file(&s)), DecodeErrorKind::InvalidUtf8);

    // A string longer than its section.
    let mut s = empty();
    s[0] = [u32le(1).to_vec(), u32le(5).to_vec(), b"ab".to_vec()].concat();
    assert_eq!(kind(&file(&s)), DecodeErrorKind::UnexpectedEnd);
}

#[test]
fn tag_errors() {
    let tag = |what, tag| DecodeErrorKind::InvalidTag { what, tag };

    // (Each list entry must have room for its smallest encoding, so the
    // payloads below are padded to pass that check and reach the tag.)
    let mut s = empty();
    s[1] = [u32le(1).to_vec(), vec![9, 0]].concat();
    assert_eq!(kind(&file(&s)), tag("type", 9));

    let mut s = empty();
    s[1] = [u32le(1).to_vec(), vec![2, 99]].concat(); // array of tag 99
    assert_eq!(kind(&file(&s)), tag("value type", 99));

    let mut s = empty();
    s[2] = [u32le(1).to_vec(), vec![10, 0]].concat();
    assert_eq!(kind(&file(&s)), tag("constant", 10));

    let mut s = empty();
    s[2] = [u32le(1).to_vec(), vec![0, 2]].concat(); // bool 2
    assert_eq!(kind(&file(&s)), tag("boolean", 2));

    let mut s = empty();
    s[4] = [u32le(1).to_vec(), u32le(0).to_vec(), vec![4, 7, 0]].concat(); // mutable = 7
    assert_eq!(kind(&file(&s)), tag("boolean", 7));

    let mut s = empty();
    s[6] = [
        u32le(1).to_vec(),
        u32le(0).to_vec(),
        vec![3],
        u32le(0).to_vec(),
    ]
    .concat();
    assert_eq!(kind(&file(&s)), tag("export", 3));

    let mut s = empty();
    s[7] = [u32le(1).to_vec(), vec![99, 0], u32le(0).to_vec()].concat();
    assert_eq!(kind(&file(&s)), tag("hook", 99));

    let mut s = empty();
    s[7] = [u32le(1).to_vec(), vec![0, 2], u32le(0).to_vec()].concat();
    assert_eq!(kind(&file(&s)), tag("callee", 2));

    let mut s = empty();
    s[8] = vec![2, 0];
    assert_eq!(kind(&file(&s)), tag("option", 2));
}

#[test]
fn constant_errors() {
    let mut s = empty();
    s[2] = [u32le(1).to_vec(), vec![5], u32le(0xd800).to_vec()].concat();
    assert_eq!(kind(&file(&s)), DecodeErrorKind::InvalidChar(0xd800));

    // k0 = [k0]: a constant may not contain itself.
    let mut s = empty();
    s[2] = [
        u32le(1).to_vec(),
        vec![8],
        u32le(1).to_vec(),
        u32le(0).to_vec(),
    ]
    .concat();
    assert_eq!(
        kind(&file(&s)),
        DecodeErrorKind::ConstForwardRef { index: 0, child: 0 }
    );

    // k0 = 1, k1 = {k0: k2}.
    let mut s = empty();
    s[2] = [
        u32le(2).to_vec(),
        vec![1],
        1i64.to_le_bytes().to_vec(),
        vec![9],
        u32le(1).to_vec(),
        u32le(0).to_vec(),
        u32le(2).to_vec(),
    ]
    .concat();
    assert_eq!(
        kind(&file(&s)),
        DecodeErrorKind::ConstForwardRef { index: 1, child: 2 }
    );
}

/// A chain k0 = true, k1 = [k0], k2 = [k1], ... of the given depth.
fn const_chain(depth: u32) -> Vec<u8> {
    let mut s = empty();
    let mut p = u32le(depth).to_vec();
    p.extend([0, 1]);
    for i in 1..depth {
        p.push(8);
        p.extend(u32le(1));
        p.extend(u32le(i - 1));
    }
    s[2] = p;
    file(&s)
}

#[test]
fn constant_depth_is_budgeted() {
    let limits = Limits::default();
    assert_eq!(limits.max_const_depth, 64);
    assert!(decode(&const_chain(64)).is_ok());
    assert_eq!(
        kind(&const_chain(65)),
        DecodeErrorKind::LimitExceeded(Limit::ConstDepth)
    );
    // A 100,000-deep chain is refused in linear time, without recursion.
    assert_eq!(
        kind(&const_chain(100_000)),
        DecodeErrorKind::LimitExceeded(Limit::ConstDepth)
    );
    let mut deep = Limits::default();
    deep.max_const_depth = 200_000;
    let module = decode_with(&const_chain(100_000), &deep).unwrap();
    assert_eq!(module.consts().len(), 100_000);
}

#[test]
fn hook_and_debug_errors() {
    let mut s = empty();
    let hook = |code: u8| [vec![code, 0], u32le(0).to_vec()].concat();
    s[7] = [u32le(2).to_vec(), hook(5), hook(5)].concat();
    assert_eq!(kind(&file(&s)), DecodeErrorKind::HookOrder);
    s[7] = [u32le(2).to_vec(), hook(5), hook(4)].concat();
    assert_eq!(kind(&file(&s)), DecodeErrorKind::HookOrder);
    s[7] = [u32le(2).to_vec(), hook(4), hook(5)].concat();
    assert!(decode(&file(&s)).is_ok());

    let mut s = empty();
    s[9] = [u32le(1), u32le(0), u32le(0)].concat();
    assert_eq!(
        kind(&file(&s)),
        DecodeErrorKind::DebugCount {
            expected: 0,
            found: 1
        }
    );
}

#[test]
fn instruction_errors_carry_their_offset() {
    let mut s = empty();
    s[5] = [
        u32le(1).to_vec(),
        function(&[], &[[0xff, 0, 0, 0, 0, 0, 0, 0]]),
    ]
    .concat();
    s[9] = [u32le(1), u32le(0), u32le(0)].concat();
    let bytes = file(&s);
    let err = decode(&bytes).unwrap_err();
    assert_eq!(
        err.kind(),
        &DecodeErrorKind::Inst(InstError::UnknownOpcode(0xff))
    );
    assert_eq!(bytes[err.offset()], 0xff);
    assert_eq!(
        err.to_string(),
        format!("at byte {}: unknown opcode 0xff", err.offset())
    );
}

#[test]
fn every_limit_is_enforced() {
    let mut m = ModuleBuilder::new();
    m.string("a");
    m.string("b");
    m.add_type(bytecode_lang::TypeDef::Cell(ValType::Dyn));
    m.add_type(bytecode_lang::TypeDef::Array(ValType::Dyn));
    m.constant(Const::Int(1));
    m.constant(Const::Int(2));
    for name in ["f", "g"] {
        let mut f = m.function(name, &[ValType::I64, ValType::I64], &[]);
        f.emit(Inst::Nop {});
        f.ret_void();
        m.add_function(f).unwrap();
    }
    let bytes = encode(&m.finish().unwrap());
    let refused = |set: fn(&mut Limits), limit: Limit| {
        let mut limits = Limits::default();
        set(&mut limits);
        assert_eq!(
            decode_with(&bytes, &limits).unwrap_err().kind(),
            &DecodeErrorKind::LimitExceeded(limit)
        );
    };
    refused(|l| l.max_bytes = 100, Limit::Bytes);
    refused(|l| l.max_strings = 1, Limit::Strings);
    refused(|l| l.max_types = 1, Limit::Types);
    refused(|l| l.max_consts = 1, Limit::Consts);
    refused(|l| l.max_functions = 1, Limit::Functions);
    refused(|l| l.max_insts = 1, Limit::Insts);
    refused(|l| l.max_total_insts = 3, Limit::TotalInsts);
    refused(|l| l.max_items = 1, Limit::Items); // two parameter registers
    refused(|l| l.max_const_depth = 0, Limit::ConstDepth);
    assert!(decode_with(&bytes, &Limits::default()).is_ok());
}

#[test]
fn per_function_tables_are_capped_by_the_format() {
    // A register count of 65,537 cannot be addressed by a 16-bit register.
    let mut s = empty();
    let mut f = Vec::new();
    f.extend(u32le(0));
    f.extend(u32le(0));
    f.push(0); // no parameter list
    f.extend(u32le(65_537));
    f.resize(45, 0); // room for the smallest function encoding
    s[5] = [u32le(1).to_vec(), f].concat();
    assert_eq!(
        kind(&file(&s)),
        DecodeErrorKind::LimitExceeded(Limit::PerFunction)
    );
}

/// A module with one function whose payload is `f` (and its debug entry).
fn one_function(f: Vec<u8>) -> Vec<u8> {
    let mut s = empty();
    s[5] = [u32le(1).to_vec(), f].concat();
    s[9] = [u32le(1), u32le(0), u32le(0)].concat();
    file(&s)
}

#[test]
fn parameter_lists_and_shapes_decode_from_hand_assembled_bytes() {
    use bytecode_lang::{ArgKind, Param, ParamKind, StrId};
    // ignore_extra; `&$a` (normal, by reference, named s3) and `...$rest`
    // (rest map, default flag clear, no name).
    let params = [&[1u8, 1][..], &u32le(2), &[1, 1, 1], &u32le(3), &[4, 0, 0]].concat();
    // One shape: (_, s5:, **).
    let shapes = [&u32le(1)[..], &u32le(3), &[0, 1], &u32le(5), &[3]].concat();
    let bytes = one_function(function_with(&params, &[13, 13], &shapes, &[]));
    let module = decode(&bytes).unwrap();
    let f = &module.functions()[0];
    let list = f.params().unwrap();
    assert!(list.ignore_extra);
    assert_eq!(
        list.params,
        [
            Param::normal(StrId(3)).by_ref(),
            Param::new(ParamKind::RestMap, None)
        ]
    );
    assert_eq!(
        f.shapes()[0].args,
        [
            ArgKind::Positional,
            ArgKind::Named(StrId(5)),
            ArgKind::SpreadNamed
        ]
    );
    assert_eq!(encode(&module), bytes);
}

#[test]
fn parameter_list_and_shape_errors() {
    let invalid =
        |params: &[u8], shapes: &[u8]| kind(&one_function(function_with(params, &[], shapes, &[])));
    let tag = |what, tag| DecodeErrorKind::InvalidTag { what, tag };
    assert_eq!(invalid(&[2], &[]), tag("option", 2));
    assert_eq!(invalid(&[1, 2], &[]), tag("parameter list flags", 2));
    let kind6 = [&[1u8, 0][..], &u32le(1), &[6, 0, 0]].concat();
    assert_eq!(invalid(&kind6, &[]), tag("parameter kind", 6));
    let flags4 = [&[1u8, 0][..], &u32le(1), &[1, 4, 0]].concat();
    assert_eq!(invalid(&flags4, &[]), tag("parameter flags", 4));
    let too_many = [&[1u8, 0][..], &u32le(256), &[0; 768]].concat();
    assert_eq!(
        invalid(&too_many, &[]),
        DecodeErrorKind::LimitExceeded(Limit::Arity)
    );
    let bad_arg = [&u32le(1)[..], &u32le(1), &[4]].concat();
    assert_eq!(invalid(&[0], &bad_arg), tag("call shape argument", 4));
    let long_shape = [&u32le(1)[..], &u32le(256), &[0; 256]].concat();
    assert_eq!(
        invalid(&[0], &long_shape),
        DecodeErrorKind::LimitExceeded(Limit::Arity)
    );
    // An import's parameter list is read the same way.
    let mut s = empty();
    s[3] = [&u32le(1)[..], &u32le(0), &u32le(0), &u32le(0), &[1, 9]].concat();
    assert_eq!(kind(&file(&s)), tag("parameter list flags", 9));
}

#[test]
fn huge_counts_do_not_allocate() {
    // Claims of four billion strings, types, constants, and functions in a
    // tiny file are refused by the remaining-bytes check, not by trying.
    let mut limits = Limits::default();
    limits.max_strings = usize::MAX;
    limits.max_types = usize::MAX;
    limits.max_consts = usize::MAX;
    limits.max_functions = usize::MAX;
    limits.max_items = usize::MAX;
    for section in [0usize, 1, 2, 3, 4, 5, 6] {
        let mut s = empty();
        s[section] = u32le(u32::MAX).to_vec();
        assert_eq!(
            decode_with(&file(&s), &limits).unwrap_err().kind(),
            &DecodeErrorKind::UnexpectedEnd,
            "section {}",
            section + 1
        );
    }
}

#[test]
fn errors_render_with_their_offset() {
    let err = decode(&file(&empty())[..20]).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("at byte {}: unexpected end of input", err.offset())
    );
    assert_eq!(
        DecodeErrorKind::LimitExceeded(Limit::ConstDepth).to_string(),
        "constant nesting depth limit exceeded"
    );
}

#[test]
fn large_modules_round_trip() {
    // 200,000 instructions across 100 functions, 100,000 strings, and
    // 100,000 constants.
    let mut m = ModuleBuilder::new();
    for i in 0..100_000 {
        m.string(&format!("name{i}"));
        m.constant(Const::Int(i));
    }
    for i in 0..100 {
        let mut f = m.function(&format!("f{i}"), &[ValType::I64], &[ValType::I64]);
        let x = f.param(0);
        let op = IntOp::new(IntTy::I64);
        for _ in 0..1_999 {
            f.emit(Inst::IAdd {
                dst: x,
                lhs: x,
                rhs: x,
                op,
            });
        }
        f.ret(x);
        m.add_function(f).unwrap();
    }
    let module = m.finish().unwrap();
    let bytes = encode(&module);
    let back = decode(&bytes).unwrap();
    assert_eq!(
        back.functions()
            .iter()
            .map(|f| f.code().len())
            .sum::<usize>(),
        200_000
    );
    assert_eq!(back.string_count(), 100_100);
    assert_eq!(back, module);
    assert_eq!(encode(&back), bytes);
}

#[test]
fn a_coroutine_type_is_one_byte() {
    // `coroutine` is a bare tag, so a one-entry types section is 5 bytes;
    // the remaining-bytes check must not demand more.
    let mut s = empty();
    s[1] = [u32le(1).to_vec(), vec![6]].concat();
    let module = decode(&file(&s)).unwrap();
    assert_eq!(module.types(), &[bytecode_lang::TypeDef::Coroutine]);
    assert_eq!(encode(&module), file(&s));
    s[1] = [u32le(1).to_vec(), vec![7]].concat();
    assert_eq!(
        kind(&file(&s)),
        DecodeErrorKind::InvalidTag {
            what: "type",
            tag: 7
        }
    );
}
