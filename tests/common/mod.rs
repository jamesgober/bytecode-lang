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
    ArgKind, Callee, Const, ConstId, ErrorKind, ExportItem, Field, FieldKind, FloatConv,
    FloatToInt, FuncId, FuncType, GlobalId, Hook, ImportId, Inst, IntConv, IntOp, IntPair, IntTy,
    Method, Module, ModuleBuilder, Opcode, Param, ParamKind, ParamList, Policy, Reg, StrId,
    StructDef, TypeDef, TypeId, ValType,
};
use proptest::prelude::*;

/// A policy built from its four parts (`promote` and `shift = saturate`
/// included).
pub fn policy() -> impl Strategy<Value = Policy> {
    (0u8..4, 0u8..2, 0u8..3, 0u8..2)
        .prop_map(|(o, d, s, f)| Policy::from_bits(o | (d << 2) | (s << 3) | (f << 5)).unwrap())
}

/// The codes of the error kinds `raise` accepts (the catchable ones).
pub fn raisable_codes() -> Vec<u32> {
    ErrorKind::ALL
        .iter()
        .filter(|k| k.is_catchable())
        .map(|k| k.code())
        .collect()
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
        | FieldKind::Upval
        | FieldKind::Shape => index16(),
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
        FieldKind::Kind => (0u32..15).boxed(),
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
        FieldKind::FloatConv => (0u8..8, any::<bool>())
            .prop_map(|(t, sat)| {
                let f = if sat {
                    FloatToInt::Saturate
                } else {
                    FloatToInt::Error
                };
                let c = FloatConv::new(IntTy::from_code(t).unwrap()).with_float_to_int(f);
                u32::from(c.bits())
            })
            .boxed(),
        FieldKind::ErrKind => prop::sample::select(raisable_codes()).boxed(),
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

/// A parameter-list description: (kind code, by reference, default) per
/// parameter, and `ignore_extra`. [`param_list`] turns it into a valid list.
pub type ParamSpec = (Vec<(u8, bool, bool)>, bool);

pub fn param_spec() -> impl Strategy<Value = ParamSpec> {
    (
        prop::collection::vec((0u8..6, any::<bool>(), any::<bool>()), 0..6),
        any::<bool>(),
    )
}

/// A valid parameter list from a description: kinds sorted into their
/// required order, one rest of each rank kept, every parameter named
/// (`name_base + i`, so names are unique), no default on a rest.
pub fn param_list(spec: &ParamSpec, name_base: u32) -> ParamList {
    let rank = |k: ParamKind| match k {
        ParamKind::PositionalOnly => 0,
        ParamKind::Normal => 1,
        ParamKind::Rest | ParamKind::RestMap => 2,
        ParamKind::NamedOnly => 3,
        ParamKind::RestNamed => 4,
    };
    let mut entries: Vec<(ParamKind, bool, bool)> = spec
        .0
        .iter()
        .map(|&(k, r, d)| (ParamKind::from_code(k).unwrap(), r, d))
        .collect();
    entries.sort_by_key(|e| rank(e.0));
    let mut params = Vec::new();
    let mut last = None;
    for (kind, by_ref, default) in entries {
        let r = rank(kind);
        if (r == 2 || r == 4) && last == Some(r) {
            continue;
        }
        last = Some(r);
        let mut p = Param::new(kind, Some(StrId(name_base + params.len() as u32)));
        p.by_ref = by_ref;
        p.default = default && !kind.is_rest();
        params.push(p);
    }
    ParamList {
        params,
        ignore_extra: spec.1,
    }
}

/// The `dyn` signature a parameter list fits (plus the `i64` presence mask).
pub fn signature_for(list: &ParamList) -> Vec<ValType> {
    let mut sig = vec![ValType::Dyn; list.params.len()];
    if list.has_defaults() {
        sig.push(ValType::I64);
    }
    sig
}

/// A call-shape description: (tag, name) per argument.
pub fn shape_spec() -> impl Strategy<Value = Vec<(u8, u32)>> {
    prop::collection::vec((0u8..4, index32()), 0..6)
}

/// A valid call shape from a description: positional arguments and spreads
/// first, then named ones (names made unique by position).
pub fn call_shape(spec: &[(u8, u32)]) -> Vec<ArgKind> {
    let mut positional: Vec<ArgKind> = Vec::new();
    let mut named: Vec<ArgKind> = Vec::new();
    for (i, &(tag, name)) in spec.iter().enumerate() {
        match tag {
            0 => positional.push(ArgKind::Positional),
            1 => named.push(ArgKind::Named(StrId(
                name.wrapping_mul(8).wrapping_add(i as u32),
            ))),
            2 => positional.push(ArgKind::Spread),
            _ => named.push(ArgKind::SpreadNamed),
        }
    }
    positional.extend(named);
    positional
}

#[derive(Clone, Debug)]
pub struct FuncSpec {
    pub name: String,
    /// A dynamic-call signature; when present it replaces `params` with the
    /// `dyn` signature it fits.
    pub param_list: Option<ParamSpec>,
    pub shapes: Vec<Vec<(u8, u32)>>,
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
            (
                "[a-z]{1,8}",
                prop::option::of(param_spec()),
                prop::collection::vec(shape_spec(), 0..3),
            ),
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
                    (name, param_list, shapes),
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
                        param_list,
                        shapes,
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
    pub imports: Vec<(String, String, u32, Option<ParamSpec>)>,
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
        prop::collection::vec(
            (
                "[a-z]{1,4}",
                "[a-z]{1,4}",
                index32(),
                prop::option::of(param_spec()),
            ),
            0..3,
        ),
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
        prop::collection::vec((0u8..Hook::ALL.len() as u8, any::<bool>(), index32()), 0..4),
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
    for (i, (module, name, sig, params)) in spec.imports.iter().enumerate() {
        match params {
            None => {
                let _ = m.import(module, name, TypeId(*sig));
            }
            Some(p) => {
                let list = param_list(p, 100 * i as u32);
                let sig = m.func_type(&signature_for(&list), &[ValType::Dyn]);
                let _ = m.import_with_params(module, name, sig, list);
            }
        }
    }
    for (name, ty, mutable, init) in &spec.globals {
        let init =
            init.and_then(|i| (!const_ids.is_empty()).then(|| const_ids[i % const_ids.len()]));
        let _ = m.global(name, *ty, *mutable, init);
    }
    let lists: Vec<Option<ParamList>> = spec
        .functions
        .iter()
        .map(|f| f.param_list.as_ref().map(|p| param_list(p, 7)))
        .collect();
    let params: Vec<Vec<ValType>> = spec
        .functions
        .iter()
        .zip(&lists)
        .map(|(f, list)| match list {
            Some(list) => signature_for(list),
            None => f.params.clone(),
        })
        .collect();
    let builders: Vec<_> = spec
        .functions
        .iter()
        .zip(&params)
        .map(|(f, params)| m.function(&f.name, params, &f.results))
        .collect();
    for ((mut b, f), (list, params)) in builders
        .into_iter()
        .zip(&spec.functions)
        .zip(lists.into_iter().zip(&params))
    {
        if let Some(list) = list {
            b.set_params(list);
        }
        for shape in &f.shapes {
            let _ = b.call_shape(&call_shape(shape));
        }
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
        let declared: Vec<ValType> = params.iter().chain(&f.regs).copied().collect();
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
