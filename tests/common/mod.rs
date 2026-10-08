//! Shared generators: arbitrary instructions and arbitrary modules.
//!
//! Instructions are generated from the public opcode metadata
//! (`Opcode::fields`): for every field, a raw value valid for its kind is
//! drawn and placed in its slot, and the word is decoded. That covers every
//! opcode and every modifier value without a hand-written list that could
//! fall out of date, and it checks the metadata itself (a field whose kind
//! or slot were wrong would fail to decode).
//!
//! Modules are built through `ModuleBuilder` from a random description, with
//! indices deliberately allowed to be out of range (the decoder and the
//! disassembler must handle unverified modules).

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use bytecode_lang::{
    Callee, Const, ConstId, ExportItem, Field, FieldKind, FuncId, FuncType, GlobalId, Hook,
    ImportId, Inst, IntConv, IntOp, IntPair, IntTy, Method, Module, ModuleBuilder, Opcode, Policy,
    Reg, StrId, StructDef, TypeDef, TypeId, ValType,
};
use proptest::prelude::*;

/// A policy built from its four parts (`promote` included).
pub fn policy() -> impl Strategy<Value = Policy> {
    (0u8..4, 0u8..2, 0u8..2, 0u8..2)
        .prop_map(|(o, d, s, f)| Policy::from_bits(o | (d << 2) | (s << 3) | (f << 4)).unwrap())
}

/// A small or arbitrary 32-bit index: mostly in range of small tables, so
/// the disassembler resolves names, sometimes anything.
fn index32() -> BoxedStrategy<u32> {
    prop_oneof![3 => 0u32..6, 1 => any::<u32>()].boxed()
}

fn index16() -> BoxedStrategy<u32> {
    prop_oneof![3 => 0u32..6, 1 => any::<u16>().prop_map(u32::from)].boxed()
}

/// A raw slot value valid for `kind`. Targets fall in `0..code_len`.
pub fn raw_for(kind: FieldKind, code_len: u32) -> BoxedStrategy<u32> {
    match kind {
        FieldKind::Reg
        | FieldKind::Name
        | FieldKind::TypeRef
        | FieldKind::Field
        | FieldKind::Upval => index16(),
        FieldKind::Target => (0..code_len.max(1)).boxed(),
        FieldKind::Const
        | FieldKind::Func
        | FieldKind::Import
        | FieldKind::Global
        | FieldKind::Table => index32(),
        FieldKind::Imm32 => any::<i32>().prop_map(|v| v as u32).boxed(),
        FieldKind::Count => any::<u8>().prop_map(u32::from).boxed(),
        FieldKind::Bool => (0u32..2).boxed(),
        FieldKind::IntTy => (0u32..8).boxed(),
        FieldKind::FloatTy => (0u32..2).boxed(),
        FieldKind::Kind => (0u32..14).boxed(),
        FieldKind::Prim => (0u32..14).boxed(),
        FieldKind::Policy => policy().prop_map(|p| u32::from(p.bits())).boxed(),
        FieldKind::IntOp => (0u8..8, policy())
            .prop_map(|(t, p)| {
                u32::from(
                    IntOp::new(IntTy::from_code(t).unwrap())
                        .with_policy(p)
                        .bits(),
                )
            })
            .boxed(),
        FieldKind::IntConv => (0u8..8, 0u8..8, 0u8..4)
            .prop_map(|(a, b, o)| {
                let ty = |c| IntTy::from_code(c).unwrap();
                let ov = bytecode_lang::Overflow::from_code(o).unwrap();
                u32::from(IntConv::new(ty(a), ty(b), ov).bits())
            })
            .boxed(),
        FieldKind::IntPair => (0u8..8, 0u8..8)
            .prop_map(|(a, b)| {
                let ty = |c| IntTy::from_code(c).unwrap();
                u32::from(IntPair::new(ty(a), ty(b)).bits())
            })
            .boxed(),
    }
}

/// Assembles the word for `opcode` with `raws` in its fields' slots.
pub fn assemble(opcode: Opcode, raws: &[u32]) -> [u8; 8] {
    let mut word = u64::from(opcode as u8);
    for (field, &raw) in opcode.fields().iter().zip(raws) {
        word |= u64::from(raw) << field.slot.shift();
    }
    word.to_le_bytes()
}

/// An arbitrary instruction of any opcode whose branch targets fall in
/// `0..code_len`.
pub fn inst(code_len: u32) -> impl Strategy<Value = Inst> {
    prop::sample::select(Opcode::ALL).prop_flat_map(move |opcode| {
        let fields: Vec<BoxedStrategy<u32>> = opcode
            .fields()
            .iter()
            .map(|f| raw_for(f.kind, code_len))
            .collect();
        fields.prop_map(move |raws| {
            Inst::from_bytes(assemble(opcode, &raws))
                .unwrap_or_else(|e| panic!("generated {opcode:?} did not decode: {e}"))
        })
    })
}

pub fn val_type() -> impl Strategy<Value = ValType> {
    prop_oneof![
        8 => prop::sample::select(vec![
            ValType::Bool, ValType::I8, ValType::I16, ValType::I32, ValType::I64, ValType::U8,
            ValType::U16, ValType::U32, ValType::U64, ValType::F32, ValType::F64, ValType::Char,
            ValType::Str, ValType::Dyn,
        ]),
        2 => index32().prop_map(|t| ValType::Ref(TypeId(t))),
    ]
}

pub fn type_def() -> impl Strategy<Value = TypeDef> {
    let vt = || val_type();
    prop_oneof![
        (
            prop::collection::vec(vt(), 0..4),
            prop::collection::vec(vt(), 0..2)
        )
            .prop_map(|(params, results)| TypeDef::Func(FuncType { params, results })),
        (
            index32(),
            prop::option::of(index32()),
            prop::collection::vec((index32(), vt()), 0..4),
            prop::collection::vec((index32(), index32()), 0..3),
        )
            .prop_map(
                |(name, parent, fields, methods)| TypeDef::Struct(StructDef {
                    name: StrId(name),
                    parent: parent.map(TypeId),
                    fields: fields
                        .into_iter()
                        .map(|(n, ty)| Field { name: StrId(n), ty })
                        .collect(),
                    methods: methods
                        .into_iter()
                        .map(|(n, f)| Method {
                            name: StrId(n),
                            func: FuncId(f)
                        })
                        .collect(),
                })
            ),
        vt().prop_map(TypeDef::Array),
        (vt(), vt()).prop_map(|(key, value)| TypeDef::Map { key, value }),
        vt().prop_map(TypeDef::Cell),
        Just(TypeDef::Coroutine),
        (vt(), vt()).prop_map(|(key, value)| TypeDef::Iter { key, value }),
    ]
}

/// A scalar constant, or an aggregate whose element picks are resolved
/// against the constants created before it.
#[derive(Clone, Debug)]
pub enum ConstSpec {
    Scalar(Const),
    Array(Vec<usize>),
    Map(Vec<(usize, usize)>),
}

pub fn const_spec() -> impl Strategy<Value = ConstSpec> {
    prop_oneof![
        any::<bool>().prop_map(|v| ConstSpec::Scalar(Const::Bool(v))),
        any::<i64>().prop_map(|v| ConstSpec::Scalar(Const::Int(v))),
        any::<u64>().prop_map(|v| ConstSpec::Scalar(Const::UInt(v))),
        any::<u32>().prop_map(|v| ConstSpec::Scalar(Const::F32(v))),
        any::<u64>().prop_map(|v| ConstSpec::Scalar(Const::F64(v))),
        any::<char>().prop_map(|v| ConstSpec::Scalar(Const::Char(v))),
        index32().prop_map(|v| ConstSpec::Scalar(Const::Str(StrId(v)))),
        prop::collection::vec(any::<u8>(), 0..12).prop_map(|v| ConstSpec::Scalar(Const::Bytes(v))),
        prop::collection::vec(any::<usize>(), 0..5).prop_map(ConstSpec::Array),
        prop::collection::vec((any::<usize>(), any::<usize>()), 0..4).prop_map(ConstSpec::Map),
    ]
}

#[derive(Clone, Debug)]
pub struct FuncSpec {
    pub name: String,
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
    pub regs: Vec<ValType>,
    pub captures: Vec<ValType>,
    pub names: Vec<u32>,
    pub type_refs: Vec<u32>,
    pub code: Vec<Inst>,
    /// (targets, default) as pcs in `0..len`.
    pub tables: Vec<(Vec<u32>, u32)>,
    /// (start, end, target, catch) with start <= end <= len, target < len.
    pub handlers: Vec<(u32, u32, u32, u16)>,
    /// (pc, file, line, column): a location set before emitting at pc.
    pub lines: Vec<(u32, u32, u32, u32)>,
    /// (reg, name, start, end) with start <= end <= len.
    pub locals: Vec<(u16, u32, u32, u32)>,
}

fn range(len: u32) -> impl Strategy<Value = (u32, u32)> {
    (0..=len, 0..=len).prop_map(|(a, b)| (a.min(b), a.max(b)))
}

pub fn func_spec() -> impl Strategy<Value = FuncSpec> {
    (1u32..24).prop_flat_map(|len| {
        (
            "[a-z]{1,8}",
            prop::collection::vec(val_type(), 0..4),
            prop::collection::vec(val_type(), 0..2),
            prop::collection::vec(val_type(), 0..6),
            prop::collection::vec(val_type(), 0..3),
            prop::collection::vec(index32(), 0..3),
            prop::collection::vec(index32(), 0..3),
            prop::collection::vec(inst(len), len as usize),
            prop::collection::vec((prop::collection::vec(0..len, 0..4), 0..len), 0..3),
            prop::collection::vec((range(len), 0..len, any::<u16>()), 0..3),
            prop::collection::vec((0..len, index32(), any::<u32>(), any::<u32>()), 0..4),
            prop::collection::vec((any::<u16>(), index32(), range(len)), 0..3),
        )
            .prop_map(
                |(
                    name,
                    params,
                    results,
                    regs,
                    captures,
                    names,
                    type_refs,
                    code,
                    tables,
                    handlers,
                    lines,
                    locals,
                )| {
                    FuncSpec {
                        name,
                        params,
                        results,
                        regs,
                        captures,
                        names,
                        type_refs,
                        code,
                        tables,
                        handlers: handlers
                            .into_iter()
                            .map(|((s, e), t, c)| (s, e, t, c))
                            .collect(),
                        lines,
                        locals: locals
                            .into_iter()
                            .map(|(r, n, (s, e))| (r, n, s, e))
                            .collect(),
                    }
                },
            )
    })
}

#[derive(Clone, Debug)]
pub struct ModuleSpec {
    pub strings: Vec<String>,
    pub types: Vec<TypeDef>,
    pub consts: Vec<ConstSpec>,
    pub imports: Vec<(String, String, u32)>,
    pub globals: Vec<(String, ValType, bool, Option<usize>)>,
    pub functions: Vec<FuncSpec>,
    pub exports: Vec<(String, u8, u32)>,
    pub hooks: Vec<(u8, bool, u32)>,
    pub name: Option<String>,
    pub start: Option<u32>,
}

pub fn module_spec() -> impl Strategy<Value = ModuleSpec> {
    (
        prop::collection::vec("\\PC{0,6}", 0..6),
        prop::collection::vec(type_def(), 0..5),
        prop::collection::vec(const_spec(), 0..8),
        prop::collection::vec(("[a-z]{1,4}", "[a-z]{1,4}", index32()), 0..3),
        prop::collection::vec(
            (
                "[a-z]{1,4}",
                val_type(),
                any::<bool>(),
                prop::option::of(any::<usize>()),
            ),
            0..3,
        ),
        prop::collection::vec(func_spec(), 0..4),
        prop::collection::vec(("[a-z]{1,4}", 0u8..3, index32()), 0..3),
        prop::collection::vec((0u8..27, any::<bool>(), index32()), 0..4),
        prop::option::of("[a-z.]{1,8}"),
        prop::option::of(index32()),
    )
        .prop_map(
            |(strings, types, consts, imports, globals, functions, exports, hooks, name, start)| {
                ModuleSpec {
                    strings,
                    types,
                    consts,
                    imports,
                    globals,
                    functions,
                    exports,
                    hooks,
                    name,
                    start,
                }
            },
        )
}

/// `inst`, unless it carries `overflow = promote` and writes a register
/// declared with a static type, which the builder rightly refuses: then the
/// same instruction with its destination moved to an undeclared register
/// (the verifier's concern, not the builder's), so `promote` stays covered.
pub fn promote_safe(inst: Inst, declared: &[ValType]) -> Inst {
    if inst.overflow() != Some(bytecode_lang::Overflow::Promote) {
        return inst;
    }
    let op = inst.opcode();
    let word = u64::from_le_bytes(inst.to_bytes());
    let Some(dst) = op.fields().iter().find(|f| f.name == "dst") else {
        return inst;
    };
    let shift = dst.slot.shift();
    let reg = ((word >> shift) & 0xffff) as usize;
    if declared.get(reg).is_none_or(|&t| t == ValType::Dyn) {
        return inst;
    }
    let word = (word & !(0xffff << shift)) | (0xffff << shift);
    Inst::from_bytes(word.to_le_bytes()).unwrap()
}

/// Builds the module a spec describes. Deterministic in the spec.
pub fn build(spec: &ModuleSpec) -> Module {
    let mut m = ModuleBuilder::new();
    for s in &spec.strings {
        let _ = m.string(s);
    }
    for t in &spec.types {
        let _ = m.add_type(t.clone());
    }
    let mut const_ids: Vec<ConstId> = Vec::new();
    for c in &spec.consts {
        let pick = |i: usize| const_ids[i % const_ids.len()];
        let c = match c {
            ConstSpec::Scalar(c) => c.clone(),
            ConstSpec::Array(items) if !const_ids.is_empty() => {
                Const::Array(items.iter().map(|&i| pick(i)).collect())
            }
            ConstSpec::Map(entries) if !const_ids.is_empty() => {
                Const::Map(entries.iter().map(|&(k, v)| (pick(k), pick(v))).collect())
            }
            ConstSpec::Array(_) => Const::Array(vec![]),
            ConstSpec::Map(_) => Const::Map(vec![]),
        };
        const_ids.push(m.constant(c));
    }
    for (module, name, sig) in &spec.imports {
        let _ = m.import(module, name, TypeId(*sig));
    }
    for (name, ty, mutable, init) in &spec.globals {
        let init =
            init.and_then(|i| (!const_ids.is_empty()).then(|| const_ids[i % const_ids.len()]));
        let _ = m.global(name, *ty, *mutable, init);
    }
    let builders: Vec<_> = spec
        .functions
        .iter()
        .map(|f| m.function(&f.name, &f.params, &f.results))
        .collect();
    for (mut b, f) in builders.into_iter().zip(&spec.functions) {
        let _ = b.regs(&f.regs);
        for &ty in &f.captures {
            let _ = b.capture(ty);
        }
        for &n in &f.names {
            let _ = b.name_ref(StrId(n));
        }
        for &t in &f.type_refs {
            let _ = b.type_ref(TypeId(t));
        }
        let len = f.code.len() as u32;
        // One label per pc 0..=len, bound as emission reaches it.
        let labels: Vec<_> = (0..=len).map(|_| b.label()).collect();
        for &(start, end, target, catch) in &f.handlers {
            b.try_region(
                labels[start as usize],
                labels[end as usize],
                labels[target as usize],
                Reg(catch),
            );
        }
        for &(reg, name, start, end) in &f.locals {
            b.local(
                Reg(reg),
                StrId(name),
                labels[start as usize],
                labels[end as usize],
            );
        }
        let declared: Vec<ValType> = f.params.iter().chain(&f.regs).copied().collect();
        for (pc, inst) in f.code.iter().enumerate() {
            b.bind(labels[pc]);
            for &(at, file, line, column) in &f.lines {
                if at as usize == pc {
                    b.set_location(StrId(file), line, column);
                }
            }
            let _ = b.emit(promote_safe(*inst, &declared));
        }
        b.bind(labels[len as usize]);
        // Jump tables come with a `switch` each, emitted after the raw code
        // so the raw code's pcs (and its targets) stay where they were.
        for (targets, default) in &f.tables {
            let ts: Vec<_> = targets.iter().map(|&t| labels[t as usize]).collect();
            let _ = b.switch(IntTy::I64, Reg(0), &ts, labels[*default as usize]);
        }
        m.add_function(b).unwrap();
    }
    for (name, kind, index) in &spec.exports {
        let item = match kind {
            0 => ExportItem::Func(FuncId(*index)),
            1 => ExportItem::Global(GlobalId(*index)),
            _ => ExportItem::Type(TypeId(*index)),
        };
        m.export(name, item);
    }
    for &(code, import, index) in &spec.hooks {
        let callee = if import {
            Callee::Import(ImportId(index))
        } else {
            Callee::Func(FuncId(index))
        };
        m.hook(Hook::from_code(code).unwrap(), callee);
    }
    if let Some(name) = &spec.name {
        m.set_name(name);
    }
    if let Some(start) = spec.start {
        m.set_start(FuncId(start));
    }
    m.finish().unwrap()
}
