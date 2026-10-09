//! The disassembler: a deterministic text listing of a module.
//!
//! The listing depends only on the module (there is no map iteration and no
//! address or pointer anywhere), so the same module always prints the same
//! text, byte for byte. It never panics on a module whose indices are out of
//! range (a decoded module is not yet verified): such references print as
//! `<invalid>`.

use alloc::vec::Vec;
use core::fmt;

use crate::FORMAT_VERSION;
use crate::inst::{Cx, write_const_preview, write_quoted};
use crate::module::{Function, Module};
use crate::types::ValType;

/// Writes `id` followed by its string in quotes, or `<invalid>`.
fn named(f: &mut fmt::Formatter<'_>, m: &Module, id: crate::StrId) -> fmt::Result {
    write!(f, "{id} ")?;
    match m.string(id) {
        Some(s) => write_quoted(f, s),
        None => f.write_str("<invalid>"),
    }
}

fn types_list(
    f: &mut fmt::Formatter<'_>,
    label: &str,
    prefix: &str,
    types: &[ValType],
) -> fmt::Result {
    if types.is_empty() {
        return Ok(());
    }
    write!(f, "  {label}")?;
    for (i, ty) in types.iter().enumerate() {
        write!(f, " {prefix}{i}:{ty}")?;
    }
    writeln!(f)
}

/// The sorted, deduplicated in-range pcs some branch, table, or handler
/// jumps to.
fn labels(func: &Function) -> Vec<u32> {
    let len = func.code.len();
    let mut out: Vec<u32> = func
        .code
        .iter()
        .filter_map(crate::Inst::branch_target)
        .map(|t| t.0)
        .chain(func.tables.iter().flat_map(|t| {
            t.targets
                .iter()
                .map(|t| t.0)
                .chain(core::iter::once(t.default.0))
        }))
        .chain(func.handlers.iter().map(|h| h.target.0))
        .filter(|&pc| (pc as usize) < len)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn function(f: &mut fmt::Formatter<'_>, m: &Module, id: usize, func: &Function) -> fmt::Result {
    write!(f, "\nfunc f{id} ")?;
    named(f, m, func.name)?;
    writeln!(f, " : {}", func.sig)?;
    if let Some(params) = &func.params {
        writeln!(f, "  params {params}")?;
    }
    types_list(f, "regs", "r", &func.regs)?;
    types_list(f, "captures", "u", &func.captures)?;
    if !func.names.is_empty() {
        f.write_str("  names")?;
        for (i, s) in func.names.iter().enumerate() {
            write!(f, " n{i}={s}")?;
        }
        writeln!(f)?;
    }
    if !func.type_refs.is_empty() {
        f.write_str("  type_refs")?;
        for (i, t) in func.type_refs.iter().enumerate() {
            write!(f, " ty{i}={t}")?;
        }
        writeln!(f)?;
    }
    let labels = labels(func);
    let cx = Cx {
        labels: &labels,
        module: Some(m),
        func: Some(func),
    };
    let target = |f: &mut fmt::Formatter<'_>, pc: u32| -> fmt::Result {
        match labels.binary_search(&pc) {
            Ok(i) => write!(f, "L{i}"),
            Err(_) => write!(f, "@{pc}"),
        }
    };
    for (i, table) in func.tables.iter().enumerate() {
        write!(f, "  table jt{i} [")?;
        for (j, t) in table.targets.iter().enumerate() {
            if j > 0 {
                f.write_str(", ")?;
            }
            target(f, t.0)?;
        }
        f.write_str("] default ")?;
        target(f, table.default.0)?;
        writeln!(f)?;
    }
    for (i, shape) in func.shapes.iter().enumerate() {
        writeln!(f, "  shape cs{i} {shape}")?;
    }
    for h in &func.handlers {
        write!(f, "  try @{}..@{} -> ", h.start, h.end)?;
        target(f, h.target.0)?;
        writeln!(f, " catch {}", h.catch)?;
    }
    for local in &func.locals {
        write!(f, "  local {} ", local.reg)?;
        named(f, m, local.name)?;
        writeln!(f, " @{}..@{}", local.start, local.end)?;
    }
    for row in &func.lines {
        writeln!(
            f,
            "  line @{} {}:{}:{}",
            row.pc, row.file, row.line, row.column
        )?;
    }
    let mut next_label = 0usize;
    for (pc, inst) in func.code.iter().enumerate() {
        if labels.get(next_label).is_some_and(|&l| l as usize == pc) {
            writeln!(f, "L{next_label}:")?;
            next_label += 1;
        }
        write!(f, "  {pc:04} ")?;
        inst.render(f, &cx)?;
        writeln!(f)?;
    }
    Ok(())
}

/// The disassembly listing; see [`crate::disassemble`].
impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self;
        writeln!(f, "lsb {FORMAT_VERSION}")?;
        if let Some(name) = m.name {
            f.write_str("module ")?;
            named(f, m, name)?;
            writeln!(f)?;
        }
        if let Some(start) = m.start {
            writeln!(f, "start {start}")?;
        }

        writeln!(f, "strings {}", m.strings.len())?;
        for (i, s) in m.strings.iter().enumerate() {
            write!(f, "  s{i} ")?;
            write_quoted(f, s)?;
            writeln!(f)?;
        }
        writeln!(f, "types {}", m.types.len())?;
        for (i, t) in m.types.iter().enumerate() {
            writeln!(f, "  t{i} {t}")?;
        }
        writeln!(f, "consts {}", m.consts.len())?;
        for (i, c) in m.consts.iter().enumerate() {
            write!(f, "  k{i} {c}")?;
            if let crate::Const::Str(_) = c {
                f.write_str("  ; ")?;
                write_const_preview(f, m, c)?;
            }
            writeln!(f)?;
        }
        writeln!(f, "imports {}", m.imports.len())?;
        for (i, imp) in m.imports.iter().enumerate() {
            write!(f, "  imp{i} ")?;
            named(f, m, imp.module)?;
            f.write_str(" . ")?;
            named(f, m, imp.name)?;
            write!(f, " : {}", imp.sig)?;
            if let Some(params) = &imp.params {
                write!(f, " params {params}")?;
            }
            writeln!(f)?;
        }
        writeln!(f, "globals {}", m.globals.len())?;
        for (i, g) in m.globals.iter().enumerate() {
            write!(f, "  g{i} ")?;
            named(f, m, g.name)?;
            write!(f, " : {}", g.ty)?;
            if g.mutable {
                f.write_str(" mut")?;
            }
            if let Some(init) = g.init {
                write!(f, " = {init}")?;
            }
            writeln!(f)?;
        }
        writeln!(f, "exports {}", m.exports.len())?;
        for e in &m.exports {
            f.write_str("  ")?;
            named(f, m, e.name)?;
            writeln!(f, " = {}", e.item)?;
        }
        writeln!(f, "hooks {}", m.hooks.len())?;
        for b in &m.hooks {
            writeln!(f, "  {} = {}", b.hook, b.callee)?;
        }
        writeln!(f, "functions {}", m.functions.len())?;
        for (i, func) in m.functions.iter().enumerate() {
            function(f, m, i, func)?;
        }
        Ok(())
    }
}
