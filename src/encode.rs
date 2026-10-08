//! The binary encoder.
//!
//! The format is fixed-width little-endian throughout (see `specs/LSB.md`
//! §6): every value has exactly one encoding, which is what makes encoding
//! deterministic and `encode(decode(bytes)) == bytes` for every accepted
//! input. Each section is written by one function generic over [`Out`];
//! running it with a byte counter gives the section's exact length, and
//! running it with a buffer writes it, so the length prefix can never
//! disagree with the payload.

use alloc::vec::Vec;

use crate::module::{Const, Function, Module};
use crate::types::{TypeDef, ValType};
use crate::{FORMAT_VERSION, MAGIC};

/// The number of sections, in their fixed order (ids `1..=SECTIONS`).
pub(crate) const SECTIONS: usize = 10;

/// Section ids.
pub(crate) mod section {
    pub(crate) const STRINGS: u32 = 1;
    pub(crate) const TYPES: u32 = 2;
    pub(crate) const CONSTS: u32 = 3;
    pub(crate) const IMPORTS: u32 = 4;
    pub(crate) const GLOBALS: u32 = 5;
    pub(crate) const FUNCTIONS: u32 = 6;
    pub(crate) const EXPORTS: u32 = 7;
    pub(crate) const HOOKS: u32 = 8;
    pub(crate) const META: u32 = 9;
    // Section 10, the last, is DEBUG; it is the `_` arm of every match.
}

/// The size of the file header: magic, format version, flags.
pub(crate) const HEADER_LEN: usize = 12;

/// Where encoded bytes go: a buffer, or a counter.
pub(crate) trait Out {
    fn put(&mut self, bytes: &[u8]);

    fn put_code(&mut self, code: &[crate::Inst]);

    fn u8(&mut self, v: u8) {
        self.put(&[v]);
    }

    fn u16(&mut self, v: u16) {
        self.put(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.put(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.put(&v.to_le_bytes());
    }

    /// A count or length. Every list in a [`Module`] has at most `u32::MAX`
    /// entries (the builder and decoder both guarantee it), so this never
    /// saturates in practice; saturating keeps it total regardless.
    fn len(&mut self, n: usize) {
        self.u32(u32::try_from(n).unwrap_or(u32::MAX));
    }
}

/// Counts bytes without writing them.
#[derive(Default)]
pub(crate) struct Count(pub(crate) u64);

impl Out for Count {
    fn put(&mut self, bytes: &[u8]) {
        self.0 = self.0.saturating_add(bytes.len() as u64);
    }

    fn put_code(&mut self, code: &[crate::Inst]) {
        self.0 = self.0.saturating_add((code.len() as u64).saturating_mul(8));
    }
}

impl Out for Vec<u8> {
    fn put(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }

    fn put_code(&mut self, code: &[crate::Inst]) {
        self.reserve(code.len().saturating_mul(8));
        for inst in code {
            self.extend_from_slice(&inst.to_bytes());
        }
    }
}

fn val_type(o: &mut impl Out, ty: ValType) {
    o.u8(ty.tag());
    if let ValType::Ref(id) = ty {
        o.u32(id.0);
    }
}

fn val_types(o: &mut impl Out, types: &[ValType]) {
    o.len(types.len());
    for &ty in types {
        val_type(o, ty);
    }
}

fn opt_u32(o: &mut impl Out, v: Option<u32>) {
    match v {
        None => o.u8(0),
        Some(v) => {
            o.u8(1);
            o.u32(v);
        }
    }
}

fn strings(o: &mut impl Out, m: &Module) {
    o.len(m.strings.len());
    for s in m.strings.iter() {
        o.len(s.len());
        o.put(s.as_bytes());
    }
}

fn types(o: &mut impl Out, m: &Module) {
    o.len(m.types.len());
    for def in &m.types {
        o.u8(def.tag());
        match def {
            TypeDef::Func(sig) => {
                val_types(o, &sig.params);
                val_types(o, &sig.results);
            }
            TypeDef::Struct(s) => {
                o.u32(s.name.0);
                opt_u32(o, s.parent.map(|p| p.0));
                o.len(s.fields.len());
                for field in &s.fields {
                    o.u32(field.name.0);
                    val_type(o, field.ty);
                }
                o.len(s.methods.len());
                for method in &s.methods {
                    o.u32(method.name.0);
                    o.u32(method.func.0);
                }
            }
            TypeDef::Array(elem) | TypeDef::Cell(elem) => val_type(o, *elem),
            TypeDef::Map { key, value } | TypeDef::Iter { key, value } => {
                val_type(o, *key);
                val_type(o, *value);
            }
            TypeDef::Coroutine => {}
        }
    }
}

fn consts(o: &mut impl Out, m: &Module) {
    o.len(m.consts.len());
    for c in &m.consts {
        o.u8(c.tag());
        match c {
            Const::Bool(v) => o.u8(u8::from(*v)),
            Const::Int(v) => o.put(&v.to_le_bytes()),
            Const::UInt(v) | Const::F64(v) => o.u64(*v),
            Const::F32(bits) => o.u32(*bits),
            Const::Char(c) => o.u32(u32::from(*c)),
            Const::Str(id) => o.u32(id.0),
            Const::Bytes(bytes) => {
                o.len(bytes.len());
                o.put(bytes);
            }
            Const::Array(items) => {
                o.len(items.len());
                for id in items {
                    o.u32(id.0);
                }
            }
            Const::Map(entries) => {
                o.len(entries.len());
                for (k, v) in entries {
                    o.u32(k.0);
                    o.u32(v.0);
                }
            }
        }
    }
}

fn imports(o: &mut impl Out, m: &Module) {
    o.len(m.imports.len());
    for i in &m.imports {
        o.u32(i.module.0);
        o.u32(i.name.0);
        o.u32(i.sig.0);
    }
}

fn globals(o: &mut impl Out, m: &Module) {
    o.len(m.globals.len());
    for g in &m.globals {
        o.u32(g.name.0);
        val_type(o, g.ty);
        o.u8(u8::from(g.mutable));
        opt_u32(o, g.init.map(|k| k.0));
    }
}

fn function(o: &mut impl Out, f: &Function) {
    o.u32(f.name.0);
    o.u32(f.sig.0);
    val_types(o, &f.regs);
    val_types(o, &f.captures);
    o.len(f.names.len());
    for s in &f.names {
        o.u32(s.0);
    }
    o.len(f.type_refs.len());
    for t in &f.type_refs {
        o.u32(t.0);
    }
    o.len(f.tables.len());
    for table in &f.tables {
        o.u32(table.default.0);
        o.len(table.targets.len());
        for t in &table.targets {
            o.u32(t.0);
        }
    }
    o.len(f.handlers.len());
    for h in &f.handlers {
        o.u32(h.start);
        o.u32(h.end);
        o.u32(h.target.0);
        o.u16(h.catch.0);
    }
    o.len(f.code.len());
    o.put_code(&f.code);
}

fn functions(o: &mut impl Out, m: &Module) {
    o.len(m.functions.len());
    for f in &m.functions {
        function(o, f);
    }
}

fn exports(o: &mut impl Out, m: &Module) {
    o.len(m.exports.len());
    for e in &m.exports {
        o.u32(e.name.0);
        o.u8(e.item.tag());
        o.u32(e.item.index());
    }
}

fn hooks(o: &mut impl Out, m: &Module) {
    o.len(m.hooks.len());
    for b in &m.hooks {
        o.u8(b.hook.code());
        match b.callee {
            crate::Callee::Func(id) => {
                o.u8(0);
                o.u32(id.0);
            }
            crate::Callee::Import(id) => {
                o.u8(1);
                o.u32(id.0);
            }
        }
    }
}

fn meta(o: &mut impl Out, m: &Module) {
    opt_u32(o, m.name.map(|s| s.0));
    opt_u32(o, m.start.map(|f| f.0));
}

fn debug(o: &mut impl Out, m: &Module) {
    o.len(m.functions.len());
    for f in &m.functions {
        o.len(f.lines.len());
        for row in &f.lines {
            o.u32(row.pc);
            o.u32(row.file.0);
            o.u32(row.line);
            o.u32(row.column);
        }
        o.len(f.locals.len());
        for local in &f.locals {
            o.u16(local.reg.0);
            o.u32(local.name.0);
            o.u32(local.start);
            o.u32(local.end);
        }
    }
}

/// Writes the payload of section `id` (1-based).
fn write_section(o: &mut impl Out, id: u32, m: &Module) {
    match id {
        section::STRINGS => strings(o, m),
        section::TYPES => types(o, m),
        section::CONSTS => consts(o, m),
        section::IMPORTS => imports(o, m),
        section::GLOBALS => globals(o, m),
        section::FUNCTIONS => functions(o, m),
        section::EXPORTS => exports(o, m),
        section::HOOKS => hooks(o, m),
        section::META => meta(o, m),
        _ => debug(o, m),
    }
}

/// The exact payload length of every section, in section order.
pub(crate) fn section_sizes(m: &Module) -> [u64; SECTIONS] {
    let mut sizes = [0u64; SECTIONS];
    for (id, size) in (1u32..).zip(sizes.iter_mut()) {
        let mut count = Count::default();
        write_section(&mut count, id, m);
        *size = count.0;
    }
    sizes
}

/// Encodes `module`; see [`crate::encode`].
pub(crate) fn encode(m: &Module) -> Vec<u8> {
    let sizes = section_sizes(m);
    let total = sizes.iter().fold(HEADER_LEN as u64, |acc, s| {
        acc.saturating_add(8).saturating_add(*s)
    });
    let mut out = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
    out.put(&MAGIC);
    out.u32(FORMAT_VERSION);
    out.u32(0);
    for (id, &size) in (1u32..).zip(sizes.iter()) {
        out.u32(id);
        // A module only exists if every section fits a u32 length (the
        // builder refuses larger ones; the decoder read them from u32s).
        out.u32(u32::try_from(size).unwrap_or(u32::MAX));
        write_section(&mut out, id, m);
    }
    out
}
