//! The module model: everything one LSB unit contains.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::ids::{ConstId, FuncId, GlobalId, ImportId, Reg, StrId, Target, TypeId};
use crate::inst::Inst;
use crate::types::{TypeDef, ValType};

/// An entry of the module's constant pool.
///
/// Constants are exact and independent of any runtime value layout: a
/// 64-bit integer is a 64-bit integer, floats are kept as IEEE bit patterns
/// (so NaN payloads and `-0.0` survive encoding bit for bit), and strings
/// are referenced from the string table. A VM converts a constant into its
/// own representation when it loads the module (see
/// [`Inst::LoadConst`](crate::Inst::LoadConst) and
/// [`Inst::DLoadConst`](crate::Inst::DLoadConst)).
///
/// Aggregate constants ([`Array`](Const::Array), [`Map`](Const::Map)) refer
/// only to **earlier** constants, so the pool is a DAG by construction and
/// its depth is a cheap, enforced decoding budget.
///
/// # Examples
///
/// ```
/// use bytecode_lang::Const;
///
/// let half = Const::f64(0.5);
/// assert_eq!(half, Const::F64(0.5f64.to_bits()));
/// assert_eq!(half.to_string(), "f64 0.5");
/// assert_eq!(Const::Int(-3).to_string(), "int -3");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Const {
    /// A boolean.
    Bool(bool),
    /// A signed integer, loadable into any integer type it fits.
    Int(i64),
    /// An unsigned integer, for `u64` values above `i64::MAX`.
    UInt(u64),
    /// A binary32 float, as its IEEE bit pattern.
    F32(u32),
    /// A binary64 float, as its IEEE bit pattern.
    F64(u64),
    /// A Unicode scalar value.
    Char(char),
    /// A UTF-8 string from the string table, loaded as a `str`.
    Str(StrId),
    /// An arbitrary byte string, loaded as a `str` (PHP strings are bytes).
    Bytes(Vec<u8>),
    /// An immutable array of earlier constants.
    Array(Vec<ConstId>),
    /// An immutable insertion-ordered map of earlier constants.
    Map(Vec<(ConstId, ConstId)>),
}

impl Const {
    /// A binary32 constant from its value.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Const;
    ///
    /// assert_eq!(Const::f32(1.0), Const::F32(0x3f80_0000));
    /// ```
    #[must_use]
    pub fn f32(value: f32) -> Self {
        Const::F32(value.to_bits())
    }

    /// A binary64 constant from its value.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Const;
    ///
    /// assert_eq!(Const::f64(-0.0), Const::F64(0x8000_0000_0000_0000));
    /// ```
    #[must_use]
    pub fn f64(value: f64) -> Self {
        Const::F64(value.to_bits())
    }

    /// The encoding tag.
    pub(crate) const fn tag(&self) -> u8 {
        match self {
            Const::Bool(_) => 0,
            Const::Int(_) => 1,
            Const::UInt(_) => 2,
            Const::F32(_) => 3,
            Const::F64(_) => 4,
            Const::Char(_) => 5,
            Const::Str(_) => 6,
            Const::Bytes(_) => 7,
            Const::Array(_) => 8,
            Const::Map(_) => 9,
        }
    }

    /// The constants this one refers to, in order.
    pub(crate) fn children(&self) -> impl Iterator<Item = ConstId> + '_ {
        let (list, pairs): (&[ConstId], &[(ConstId, ConstId)]) = match self {
            Const::Array(items) => (items, &[]),
            Const::Map(entries) => (&[], entries),
            _ => (&[], &[]),
        };
        list.iter()
            .copied()
            .chain(pairs.iter().flat_map(|&(k, v)| [k, v]))
    }
}

/// Floats print with Rust's shortest round-trip form; NaNs print their bit
/// pattern, since the value alone would lose the payload.
impl fmt::Display for Const {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Const::Bool(v) => write!(f, "bool {v}"),
            Const::Int(v) => write!(f, "int {v}"),
            Const::UInt(v) => write!(f, "uint {v}"),
            Const::F32(bits) => {
                let v = f32::from_bits(*bits);
                if v.is_nan() {
                    write!(f, "f32 nan:{bits:#010x}")
                } else {
                    write!(f, "f32 {v:?}")
                }
            }
            Const::F64(bits) => {
                let v = f64::from_bits(*bits);
                if v.is_nan() {
                    write!(f, "f64 nan:{bits:#018x}")
                } else {
                    write!(f, "f64 {v:?}")
                }
            }
            Const::Char(c) => write!(f, "char {c:?}"),
            Const::Str(id) => write!(f, "str {id}"),
            Const::Bytes(bytes) => {
                f.write_str("bytes b\"")?;
                for &b in bytes {
                    write!(f, "{}", core::ascii::escape_default(b))?;
                }
                f.write_str("\"")
            }
            Const::Array(items) => {
                f.write_str("array [")?;
                for (i, id) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{id}")?;
                }
                f.write_str("]")
            }
            Const::Map(entries) => {
                f.write_str("map {")?;
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                f.write_str("}")
            }
        }
    }
}

/// A host function the module calls by index
/// ([`Inst::CallImport`](crate::Inst::CallImport)), bound by the loader by
/// `module` and `name`.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Import, StrId, TypeId};
///
/// let print = Import { module: StrId(0), name: StrId(1), sig: TypeId(2) };
/// assert_eq!(print.sig, TypeId(2));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Import {
    /// The providing module's name (for example `ls.io`).
    pub module: StrId,
    /// The function's name within that module.
    pub name: StrId,
    /// The function's signature (a [`TypeDef::Func`] entry).
    pub sig: TypeId,
}

/// A module-level variable.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{ConstId, Global, StrId, ValType};
///
/// let counter = Global { name: StrId(0), ty: ValType::I64, mutable: true, init: Some(ConstId(0)) };
/// assert!(counter.mutable);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Global {
    /// The global's name.
    pub name: StrId,
    /// The global's type.
    pub ty: ValType,
    /// Whether [`Inst::SetGlobal`](crate::Inst::SetGlobal) may write it.
    pub mutable: bool,
    /// The initial value; `None` means the type's default (zero, `false`,
    /// U+0000, or `nil`).
    pub init: Option<ConstId>,
}

/// What an [`Export`] makes visible to other units.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{ExportItem, FuncId};
///
/// assert_eq!(ExportItem::Func(FuncId(0)).to_string(), "f0");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ExportItem {
    /// A function.
    Func(FuncId),
    /// A global.
    Global(GlobalId),
    /// A type.
    Type(TypeId),
}

impl ExportItem {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            ExportItem::Func(_) => 0,
            ExportItem::Global(_) => 1,
            ExportItem::Type(_) => 2,
        }
    }

    pub(crate) const fn index(self) -> u32 {
        match self {
            ExportItem::Func(id) => id.0,
            ExportItem::Global(id) => id.0,
            ExportItem::Type(id) => id.0,
        }
    }
}

impl fmt::Display for ExportItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportItem::Func(id) => write!(f, "{id}"),
            ExportItem::Global(id) => write!(f, "{id}"),
            ExportItem::Type(id) => write!(f, "{id}"),
        }
    }
}

/// A named item the module exports: the shared module interface the
/// loader links native and bytecode units through.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Export, ExportItem, FuncId, StrId};
///
/// let main = Export { name: StrId(0), item: ExportItem::Func(FuncId(0)) };
/// assert_eq!(main.item, ExportItem::Func(FuncId(0)));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Export {
    /// The exported name.
    pub name: StrId,
    /// The exported item.
    pub item: ExportItem,
}

code_enum! {
    /// A dynamic-operation hook: the function a dynamic instruction calls when
    /// its built-in fast path does not apply (for example `dadd` on two
    /// strings, or `get_prop` on an object with no such field).
    ///
    /// Hooks are how a dynamic language supplies its own semantics (PHP's
    /// loose `==`, Python's list equality) without a separate instruction
    /// set per language. A missing hook makes the operation raise
    /// `TypeError` (E0100) or the error the instruction documents.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Hook;
    ///
    /// assert_eq!(Hook::Add.code(), 0);
    /// assert_eq!(Hook::GetProp.to_string(), "get_prop");
    /// ```
    Hook {
        /// `dadd` fallback: `(dyn, dyn) -> dyn`.
        Add = 0 => "add",
        /// `dsub` fallback: `(dyn, dyn) -> dyn`.
        Sub = 1 => "sub",
        /// `dmul` fallback: `(dyn, dyn) -> dyn`.
        Mul = 2 => "mul",
        /// `ddiv` fallback: `(dyn, dyn) -> dyn`.
        Div = 3 => "div",
        /// `drem` fallback: `(dyn, dyn) -> dyn`.
        Rem = 4 => "rem",
        /// `dfloor_div` fallback: `(dyn, dyn) -> dyn`.
        FloorDiv = 5 => "floor_div",
        /// `dfloor_mod` fallback: `(dyn, dyn) -> dyn`.
        FloorMod = 6 => "floor_mod",
        /// `dand` fallback: `(dyn, dyn) -> dyn`.
        BitAnd = 7 => "bit_and",
        /// `dor` fallback: `(dyn, dyn) -> dyn`.
        BitOr = 8 => "bit_or",
        /// `dxor` fallback: `(dyn, dyn) -> dyn`.
        BitXor = 9 => "bit_xor",
        /// `dshl` fallback: `(dyn, dyn) -> dyn`.
        Shl = 10 => "shl",
        /// `dshr` fallback: `(dyn, dyn) -> dyn`.
        Shr = 11 => "shr",
        /// `dneg` fallback: `(dyn) -> dyn`.
        Neg = 12 => "neg",
        /// `dnot` fallback: `(dyn) -> dyn`.
        BitNot = 13 => "bit_not",
        /// `deq`/`dne` fallback: `(dyn, dyn) -> dyn` returning a boolean.
        Eq = 14 => "eq",
        /// `dlt`/`dgt` fallback: `(dyn, dyn) -> dyn` returning a boolean.
        Lt = 15 => "lt",
        /// `dle`/`dge` fallback: `(dyn, dyn) -> dyn` returning a boolean.
        Le = 16 => "le",
        /// `dtruthy` for kinds other than nil, bool, int, float: `(dyn) -> dyn`.
        Truthy = 17 => "truthy",
        /// `dconcat` fallback: `(dyn, dyn) -> dyn`.
        Concat = 18 => "concat",
        /// `dget_index` fallback: `(dyn, dyn) -> dyn`.
        GetIndex = 19 => "get_index",
        /// `dset_index` fallback: `(dyn, dyn, dyn) -> ()`.
        SetIndex = 20 => "set_index",
        /// `get_prop` fallback: `(dyn, dyn) -> dyn` (object, name string).
        GetProp = 21 => "get_prop",
        /// `set_prop` fallback: `(dyn, dyn, dyn) -> ()`.
        SetProp = 22 => "set_prop",
        /// `has_prop` fallback: `(dyn, dyn) -> dyn` returning a boolean.
        HasProp = 23 => "has_prop",
        /// `diter_new` fallback: `(dyn) -> dyn` returning something iterable.
        Iter = 24 => "iter",
        /// `dlen` fallback: `(dyn) -> dyn` returning an integer.
        Len = 25 => "len",
        /// `dcall` on a non-callable: `(dyn, dyn) -> dyn` (callee, argument array).
        Call = 26 => "call",
        /// `spawn`: hands a new coroutine to the host scheduler
        /// (host-lang): `(dyn) -> dyn` (coroutine, task handle).
        Spawn = 27 => "spawn",
    }
}

/// A function the module can call: one of its own, or an import.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Callee, ImportId};
///
/// assert_eq!(Callee::Import(ImportId(3)).to_string(), "imp3");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Callee {
    /// A function of this module.
    Func(FuncId),
    /// A host function.
    Import(ImportId),
}

impl fmt::Display for Callee {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Callee::Func(id) => write!(f, "{id}"),
            Callee::Import(id) => write!(f, "{id}"),
        }
    }
}

/// One bound [`Hook`]. A module binds each hook at most once; bindings are
/// kept sorted by hook code.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Callee, FuncId, Hook, HookBinding};
///
/// let b = HookBinding { hook: Hook::Concat, callee: Callee::Func(FuncId(1)) };
/// assert_eq!(b.hook, Hook::Concat);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct HookBinding {
    /// The hook.
    pub hook: Hook,
    /// The function that implements it.
    pub callee: Callee,
}

/// A jump table of [`Inst::Switch`](crate::Inst::Switch): `targets[v]` for a
/// selector `v` in range, `default` otherwise.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{JumpTable, Target};
///
/// let t = JumpTable { targets: vec![Target(4), Target(9)], default: Target(12) };
/// assert_eq!(t.targets.len(), 2);
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct JumpTable {
    /// Targets for selectors `0..len`.
    pub targets: Vec<Target>,
    /// The target for every other selector.
    pub default: Target,
}

/// An exception-handling region: an error raised by an instruction at a pc
/// in `start..end` transfers control to `target` with the error value in
/// `catch`.
///
/// A function's handlers are searched in order and the first that covers
/// the faulting pc wins, so inner regions are listed before the regions
/// that enclose them.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Handler, Reg, Target};
///
/// let h = Handler { start: 0, end: 5, target: Target(7), catch: Reg(3) };
/// assert!(h.start < h.end);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Handler {
    /// First covered pc.
    pub start: u32,
    /// One past the last covered pc.
    pub end: u32,
    /// Where control continues when an error is caught.
    pub target: Target,
    /// The `dyn` register that receives the error value.
    pub catch: Reg,
}

/// A row of a function's line table: instructions from `pc` up to the next
/// row's `pc` came from this source position.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{LineRow, StrId};
///
/// let row = LineRow { pc: 0, file: StrId(2), line: 10, column: 5 };
/// assert_eq!(row.line, 10);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LineRow {
    /// The first instruction the row covers.
    pub pc: u32,
    /// The source file's name.
    pub file: StrId,
    /// The 1-based line.
    pub line: u32,
    /// The 1-based column.
    pub column: u32,
}

/// A named source variable living in a register over a pc range, for
/// debuggers.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{LocalVar, Reg, StrId};
///
/// let x = LocalVar { reg: Reg(0), name: StrId(1), start: 0, end: 9 };
/// assert_eq!(x.reg, Reg(0));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LocalVar {
    /// The register holding the variable.
    pub reg: Reg,
    /// The variable's source name.
    pub name: StrId,
    /// First pc where the variable is live.
    pub start: u32,
    /// One past the last pc where it is live.
    pub end: u32,
}

/// One function of a module: its frame layout, its tables, its code, and
/// its debug information.
///
/// Built with [`FunctionBuilder`](crate::FunctionBuilder) or produced by
/// [`decode`](crate::decode); read-only afterwards.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Inst, ModuleBuilder, ValType};
///
/// let mut m = ModuleBuilder::new();
/// let mut f = m.function("id", &[ValType::I64], &[ValType::I64]);
/// let x = f.param(0);
/// f.ret(x);
/// let id = m.add_function(f).unwrap();
/// let module = m.finish().unwrap();
///
/// let func = module.function(id).unwrap();
/// assert_eq!(func.regs(), &[ValType::I64]);
/// assert_eq!(func.code(), &[Inst::Ret { src: x }]);
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Function {
    pub(crate) name: StrId,
    pub(crate) sig: TypeId,
    pub(crate) regs: Vec<ValType>,
    pub(crate) captures: Vec<ValType>,
    pub(crate) names: Vec<StrId>,
    pub(crate) type_refs: Vec<TypeId>,
    pub(crate) tables: Vec<JumpTable>,
    pub(crate) handlers: Vec<Handler>,
    pub(crate) code: Vec<Inst>,
    pub(crate) lines: Vec<LineRow>,
    pub(crate) locals: Vec<LocalVar>,
}

impl Function {
    /// The function's name.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("main", &[], &[]);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// let func = module.function(id).unwrap();
    /// assert_eq!(module.string(func.name()), Some("main"));
    /// ```
    #[must_use]
    pub fn name(&self) -> StrId {
        self.name
    }

    /// The function's signature (a [`TypeDef::Func`] entry).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{FuncType, ModuleBuilder, TypeDef, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("main", &[], &[]);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// let sig = module.function(id).unwrap().sig();
    /// assert_eq!(module.type_def(sig), Some(&TypeDef::Func(FuncType::default())));
    /// ```
    #[must_use]
    pub fn sig(&self) -> TypeId {
        self.sig
    }

    /// The declared type of every register; the parameters come first.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[ValType::Bool], &[]);
    /// let _tmp = f.reg(ValType::Dyn);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().regs(), &[ValType::Bool, ValType::Dyn]);
    /// ```
    #[must_use]
    pub fn regs(&self) -> &[ValType] {
        &self.regs
    }

    /// The types of the values a closure of this function captures, read
    /// with [`Inst::GetUpval`](crate::Inst::GetUpval).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, UpvalIdx, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("inner", &[], &[]);
    /// assert_eq!(f.capture(ValType::Dyn), UpvalIdx(0));
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().captures(), &[ValType::Dyn]);
    /// ```
    #[must_use]
    pub fn captures(&self) -> &[ValType] {
        &self.captures
    }

    /// The function's name list, indexed by [`NameRef`](crate::NameRef).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// let mut m = ModuleBuilder::new();
    /// let x = m.string("x");
    /// let mut f = m.function("f", &[], &[]);
    /// let n = f.name_ref(x);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().names()[n.index()], x);
    /// ```
    #[must_use]
    pub fn names(&self) -> &[StrId] {
        &self.names
    }

    /// The function's type list, indexed by [`TypeRef`](crate::TypeRef).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, TypeDef, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let arr = m.add_type(TypeDef::Array(ValType::I64));
    /// let mut f = m.function("f", &[], &[]);
    /// let t = f.type_ref(arr);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().type_refs()[t.index()], arr);
    /// ```
    #[must_use]
    pub fn type_refs(&self) -> &[TypeId] {
        &self.type_refs
    }

    /// The function's jump tables, indexed by [`TableId`](crate::TableId).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntTy, ModuleBuilder, Target, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[ValType::I64], &[]);
    /// let (a, b) = (f.label(), f.label());
    /// f.switch(IntTy::I64, f.param(0), &[a], b);
    /// f.bind(a);
    /// f.bind(b);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// let table = &module.function(id).unwrap().tables()[0];
    /// assert_eq!((table.targets.as_slice(), table.default), (&[Target(1)][..], Target(1)));
    /// ```
    #[must_use]
    pub fn tables(&self) -> &[JumpTable] {
        &self.tables
    }

    /// The function's exception-handling regions, innermost first.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[], &[]);
    /// let err = f.reg(ValType::Dyn);
    /// let (start, end, catch) = (f.label(), f.label(), f.label());
    /// f.bind(start);
    /// f.emit(Inst::Nop {});
    /// f.bind(end);
    /// f.ret_void();
    /// f.bind(catch);
    /// f.ret_void();
    /// f.try_region(start, end, catch, err);
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// let h = module.function(id).unwrap().handlers()[0];
    /// assert_eq!((h.start, h.end, h.target.0), (0, 1, 2));
    /// ```
    #[must_use]
    pub fn handlers(&self) -> &[Handler] {
        &self.handlers
    }

    /// The function's instructions; `code()[pc]` is the instruction at `pc`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[], &[]);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().code(), &[Inst::RetVoid {}]);
    /// ```
    #[must_use]
    pub fn code(&self) -> &[Inst] {
        &self.code
    }

    /// The line table, ordered by pc.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// let mut m = ModuleBuilder::new();
    /// let file = m.string("main.mox");
    /// let mut f = m.function("f", &[], &[]);
    /// f.set_location(file, 3, 1);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().lines()[0].line, 3);
    /// ```
    #[must_use]
    pub fn lines(&self) -> &[LineRow] {
        &self.lines
    }

    /// The named source variables.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let name = m.string("x");
    /// let mut f = m.function("f", &[ValType::I64], &[]);
    /// let (start, end) = (f.label(), f.label());
    /// f.bind(start);
    /// f.ret_void();
    /// f.bind(end);
    /// f.local(f.param(0), name, start, end);
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().locals()[0].end, 1);
    /// ```
    #[must_use]
    pub fn locals(&self) -> &[LocalVar] {
        &self.locals
    }
}

/// The string table: UTF-8 strings stored back to back in one buffer.
///
/// One allocation for all strings (instead of one per string) is what keeps
/// decoding a module with a million names cheap.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct Strings {
    data: String,
    ends: Vec<u32>,
}

impl Strings {
    pub(crate) fn with_capacity(count: usize, bytes: usize) -> Self {
        Strings {
            data: String::with_capacity(bytes),
            ends: Vec::with_capacity(count),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.ends.len()
    }

    /// Appends `s`; `false` if the buffer would pass `u32::MAX` bytes.
    pub(crate) fn push(&mut self, s: &str) -> bool {
        let Some(end) = self
            .data
            .len()
            .checked_add(s.len())
            .and_then(|end| u32::try_from(end).ok())
        else {
            return false;
        };
        self.data.push_str(s);
        self.ends.push(end);
        true
    }

    pub(crate) fn get(&self, index: usize) -> Option<&str> {
        let end = *self.ends.get(index)? as usize;
        let start = match index.checked_sub(1) {
            Some(prev) => *self.ends.get(prev)? as usize,
            None => 0,
        };
        self.data.get(start..end)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &str> + '_ {
        (0..self.len()).filter_map(|i| self.get(i))
    }
}

/// A complete LSB module: the unit that is encoded, decoded, verified,
/// loaded, and disassembled.
///
/// A module holds a string table, a type table, a constant pool, imports,
/// globals, functions, exports, dynamic-operation hooks, an optional name and
/// start function, and per-function debug information. It is built with
/// [`ModuleBuilder`](crate::ModuleBuilder) or produced by
/// [`decode`](crate::decode), and is read-only afterwards, which is what
/// lets [`encode`](crate::encode) be infallible. `Display` prints the
/// [`disassemble`](crate::disassemble) listing.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Inst, IntOp, IntTy, ModuleBuilder, ValType};
///
/// let mut m = ModuleBuilder::new();
/// let mut f = m.function("add", &[ValType::I64, ValType::I64], &[ValType::I64]);
/// let (a, b) = (f.param(0), f.param(1));
/// let sum = f.reg(ValType::I64);
/// f.emit(Inst::IAdd { op: IntOp::new(IntTy::I64), dst: sum, lhs: a, rhs: b });
/// f.ret(sum);
/// m.add_function(f).unwrap();
/// let module = m.finish().unwrap();
///
/// assert_eq!(module.functions().len(), 1);
/// let bytes = bytecode_lang::encode(&module);
/// assert_eq!(bytecode_lang::decode(&bytes).unwrap(), module);
/// ```
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Module {
    pub(crate) strings: Strings,
    pub(crate) types: Vec<TypeDef>,
    pub(crate) consts: Vec<Const>,
    pub(crate) imports: Vec<Import>,
    pub(crate) globals: Vec<Global>,
    pub(crate) functions: Vec<Function>,
    pub(crate) exports: Vec<Export>,
    pub(crate) hooks: Vec<HookBinding>,
    pub(crate) name: Option<StrId>,
    pub(crate) start: Option<FuncId>,
}

impl Module {
    /// The string with this id, or `None` if it is out of range.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, StrId};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let hi = m.string("hi");
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.string(hi), Some("hi"));
    /// assert_eq!(module.string(StrId(99)), None);
    /// ```
    #[must_use]
    pub fn string(&self, id: StrId) -> Option<&str> {
        self.strings.get(id.index())
    }

    /// The number of strings in the string table.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// let mut m = ModuleBuilder::new();
    /// m.string("a");
    /// m.string("a"); // deduplicated
    /// assert_eq!(m.finish().unwrap().string_count(), 1);
    /// ```
    #[must_use]
    pub fn string_count(&self) -> usize {
        self.strings.len()
    }

    /// Every string, in id order.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// let mut m = ModuleBuilder::new();
    /// m.string("a");
    /// m.string("b");
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.strings().collect::<Vec<_>>(), ["a", "b"]);
    /// ```
    pub fn strings(&self) -> impl Iterator<Item = &str> + '_ {
        self.strings.iter()
    }

    /// The type table.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, TypeDef, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// m.add_type(TypeDef::Cell(ValType::Dyn));
    /// assert_eq!(m.finish().unwrap().types(), &[TypeDef::Cell(ValType::Dyn)]);
    /// ```
    #[must_use]
    pub fn types(&self) -> &[TypeDef] {
        &self.types
    }

    /// The type with this id, or `None` if it is out of range.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, TypeDef, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let t = m.add_type(TypeDef::Array(ValType::U8));
    /// assert_eq!(m.finish().unwrap().type_def(t), Some(&TypeDef::Array(ValType::U8)));
    /// ```
    #[must_use]
    pub fn type_def(&self, id: TypeId) -> Option<&TypeDef> {
        self.types.get(id.index())
    }

    /// The constant pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Const, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// m.constant(Const::Int(7));
    /// assert_eq!(m.finish().unwrap().consts(), &[Const::Int(7)]);
    /// ```
    #[must_use]
    pub fn consts(&self) -> &[Const] {
        &self.consts
    }

    /// The constant with this id, or `None` if it is out of range.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Const, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let k = m.constant(Const::Bool(true));
    /// assert_eq!(m.finish().unwrap().constant(k), Some(&Const::Bool(true)));
    /// ```
    #[must_use]
    pub fn constant(&self, id: ConstId) -> Option<&Const> {
        self.consts.get(id.index())
    }

    /// The import table.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let sig = m.func_type(&[ValType::Str], &[]);
    /// m.import("ls.io", "print", sig);
    /// assert_eq!(m.finish().unwrap().imports().len(), 1);
    /// ```
    #[must_use]
    pub fn imports(&self) -> &[Import] {
        &self.imports
    }

    /// The import with this id, or `None` if it is out of range.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let sig = m.func_type(&[], &[]);
    /// let id = m.import("env", "tick", sig);
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.import(id).map(|i| i.sig), Some(sig));
    /// ```
    #[must_use]
    pub fn import(&self, id: ImportId) -> Option<&Import> {
        self.imports.get(id.index())
    }

    /// The global table.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// m.global("count", ValType::I64, true, None);
    /// assert_eq!(m.finish().unwrap().globals().len(), 1);
    /// ```
    #[must_use]
    pub fn globals(&self) -> &[Global] {
        &self.globals
    }

    /// The global with this id, or `None` if it is out of range.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let g = m.global("debug", ValType::Bool, false, None);
    /// assert_eq!(m.finish().unwrap().global(g).map(|g| g.ty), Some(ValType::Bool));
    /// ```
    #[must_use]
    pub fn global(&self, id: GlobalId) -> Option<&Global> {
        self.globals.get(id.index())
    }

    /// The function table.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// assert!(ModuleBuilder::new().finish().unwrap().functions().is_empty());
    /// ```
    #[must_use]
    pub fn functions(&self) -> &[Function] {
        &self.functions
    }

    /// The function with this id, or `None` if it is out of range.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{FuncId, ModuleBuilder};
    ///
    /// let module = ModuleBuilder::new().finish().unwrap();
    /// assert!(module.function(FuncId(0)).is_none());
    /// ```
    #[must_use]
    pub fn function(&self, id: FuncId) -> Option<&Function> {
        self.functions.get(id.index())
    }

    /// The export table.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ExportItem, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("main", &[], &[]);
    /// f.ret_void();
    /// let main = m.add_function(f).unwrap();
    /// m.export("main", ExportItem::Func(main));
    /// assert_eq!(m.finish().unwrap().exports()[0].item, ExportItem::Func(main));
    /// ```
    #[must_use]
    pub fn exports(&self) -> &[Export] {
        &self.exports
    }

    /// The bound dynamic-operation hooks, sorted by hook code.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Callee, Hook, ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("concat", &[ValType::Dyn, ValType::Dyn], &[ValType::Dyn]);
    /// let a = f.param(0);
    /// f.ret(a);
    /// let id = m.add_function(f).unwrap();
    /// m.hook(Hook::Concat, Callee::Func(id));
    /// assert_eq!(m.finish().unwrap().hooks()[0].hook, Hook::Concat);
    /// ```
    #[must_use]
    pub fn hooks(&self) -> &[HookBinding] {
        &self.hooks
    }

    /// The function bound to `hook`, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Hook, ModuleBuilder};
    ///
    /// assert_eq!(ModuleBuilder::new().finish().unwrap().hook(Hook::Add), None);
    /// ```
    #[must_use]
    pub fn hook(&self, hook: Hook) -> Option<Callee> {
        self.hooks
            .binary_search_by_key(&hook, |b| b.hook)
            .ok()
            .and_then(|i| self.hooks.get(i))
            .map(|b| b.callee)
    }

    /// The module's name, if it has one.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// let mut m = ModuleBuilder::new();
    /// m.set_name("app.main");
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.name().and_then(|n| module.string(n)), Some("app.main"));
    /// ```
    #[must_use]
    pub fn name(&self) -> Option<StrId> {
        self.name
    }

    /// The initializer the loader runs after binding imports, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("init", &[], &[]);
    /// f.ret_void();
    /// let init = m.add_function(f).unwrap();
    /// m.set_start(init);
    /// assert_eq!(m.finish().unwrap().start(), Some(init));
    /// ```
    #[must_use]
    pub fn start(&self) -> Option<FuncId> {
        self.start
    }
}

/// A runtime error kind with its stable code.
///
/// Codes `E0001`–`E0005` are defined by `specs/OPS.md` §6 for arithmetic
/// and conversions; `E0100` and up are defined by LSB for the other
/// instructions. Every execution tier raises the same kind for the same
/// fault. [`Inst::ErrCode`](crate::Inst::ErrCode) reads the numeric code
/// back from a caught error value.
///
/// # Examples
///
/// ```
/// use bytecode_lang::ErrorKind;
///
/// assert_eq!(ErrorKind::DivByZero.code(), 2);
/// assert_eq!(ErrorKind::DivByZero.to_string(), "E0002 DivByZero");
/// assert_eq!(ErrorKind::from_code(102), Some(ErrorKind::IndexOutOfBounds));
/// assert!(!ErrorKind::OutOfFuel.is_catchable());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    /// E0001: an integer result does not fit under `overflow = error`.
    ArithOverflow,
    /// E0002: integer division by zero under `div_zero = error`.
    DivByZero,
    /// E0003: a shift amount out of range under `shift = error`.
    ShiftOutOfRange,
    /// E0004: NaN or an out-of-range float converted under `float_to_int = error`.
    InvalidConversion,
    /// E0005: `char_from_u32` on a value that is not a Unicode scalar value.
    InvalidChar,
    /// E0100: a dynamic operation on kinds it does not support, a failed
    /// `cast` or `from_dyn`, or a dynamic call with the wrong arity.
    TypeError,
    /// E0101: a heap instruction on a `nil` reference.
    NullReference,
    /// E0102: an array, string, or length operand out of range.
    IndexOutOfBounds,
    /// E0103: `map_get` (or `dget_index` on a map) of an absent key.
    KeyNotFound,
    /// E0104: `get_prop`/`set_prop` of a name the object does not have.
    UndefinedProperty,
    /// E0105: the call depth limit was reached.
    StackOverflow,
    /// E0106: an allocation failed. Always a trap.
    OutOfMemory,
    /// E0107: the run's fuel budget is spent. Always a trap.
    OutOfFuel,
    /// E0108: a UTF-8 `str_slice` boundary inside a character.
    InvalidStrIndex,
    /// E0109: `unreachable` was executed. Always a trap.
    Unreachable,
    /// E0110: a coroutine operation in a state that does not allow it:
    /// resuming one that is running, returned, or failed (a coroutine
    /// resuming itself included), or `coro_result` before it returned.
    InvalidCoroState,
    /// E0111: `yield` or `await` with no coroutine to suspend, or with a
    /// host frame between the instruction and its coroutine.
    CannotSuspend,
    /// E0112: `spawn` in a module with no `spawn` hook bound.
    NoScheduler,
    /// E0113: a coroutine being closed yielded instead of finishing
    /// (Python's "generator ignored GeneratorExit").
    CloseIgnored,
}

impl ErrorKind {
    /// Every kind, in code order.
    pub const ALL: &'static [ErrorKind] = &[
        ErrorKind::ArithOverflow,
        ErrorKind::DivByZero,
        ErrorKind::ShiftOutOfRange,
        ErrorKind::InvalidConversion,
        ErrorKind::InvalidChar,
        ErrorKind::TypeError,
        ErrorKind::NullReference,
        ErrorKind::IndexOutOfBounds,
        ErrorKind::KeyNotFound,
        ErrorKind::UndefinedProperty,
        ErrorKind::StackOverflow,
        ErrorKind::OutOfMemory,
        ErrorKind::OutOfFuel,
        ErrorKind::InvalidStrIndex,
        ErrorKind::Unreachable,
        ErrorKind::InvalidCoroState,
        ErrorKind::CannotSuspend,
        ErrorKind::NoScheduler,
        ErrorKind::CloseIgnored,
    ];

    /// The stable numeric code (`2` for `E0002`).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ErrorKind;
    ///
    /// assert_eq!(ErrorKind::TypeError.code(), 100);
    /// ```
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            ErrorKind::ArithOverflow => 1,
            ErrorKind::DivByZero => 2,
            ErrorKind::ShiftOutOfRange => 3,
            ErrorKind::InvalidConversion => 4,
            ErrorKind::InvalidChar => 5,
            ErrorKind::TypeError => 100,
            ErrorKind::NullReference => 101,
            ErrorKind::IndexOutOfBounds => 102,
            ErrorKind::KeyNotFound => 103,
            ErrorKind::UndefinedProperty => 104,
            ErrorKind::StackOverflow => 105,
            ErrorKind::OutOfMemory => 106,
            ErrorKind::OutOfFuel => 107,
            ErrorKind::InvalidStrIndex => 108,
            ErrorKind::Unreachable => 109,
            ErrorKind::InvalidCoroState => 110,
            ErrorKind::CannotSuspend => 111,
            ErrorKind::NoScheduler => 112,
            ErrorKind::CloseIgnored => 113,
        }
    }

    /// The kind with this numeric code, or `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ErrorKind;
    ///
    /// assert_eq!(ErrorKind::from_code(1), Some(ErrorKind::ArithOverflow));
    /// assert_eq!(ErrorKind::from_code(0), None);
    /// ```
    #[must_use]
    pub fn from_code(code: u32) -> Option<Self> {
        ErrorKind::ALL.iter().copied().find(|k| k.code() == code)
    }

    /// The kind's name, as in OPS and LSB.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ErrorKind;
    ///
    /// assert_eq!(ErrorKind::KeyNotFound.name(), "KeyNotFound");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            ErrorKind::ArithOverflow => "ArithOverflow",
            ErrorKind::DivByZero => "DivByZero",
            ErrorKind::ShiftOutOfRange => "ShiftOutOfRange",
            ErrorKind::InvalidConversion => "InvalidConversion",
            ErrorKind::InvalidChar => "InvalidChar",
            ErrorKind::TypeError => "TypeError",
            ErrorKind::NullReference => "NullReference",
            ErrorKind::IndexOutOfBounds => "IndexOutOfBounds",
            ErrorKind::KeyNotFound => "KeyNotFound",
            ErrorKind::UndefinedProperty => "UndefinedProperty",
            ErrorKind::StackOverflow => "StackOverflow",
            ErrorKind::OutOfMemory => "OutOfMemory",
            ErrorKind::OutOfFuel => "OutOfFuel",
            ErrorKind::InvalidStrIndex => "InvalidStrIndex",
            ErrorKind::Unreachable => "Unreachable",
            ErrorKind::InvalidCoroState => "InvalidCoroState",
            ErrorKind::CannotSuspend => "CannotSuspend",
            ErrorKind::NoScheduler => "NoScheduler",
            ErrorKind::CloseIgnored => "CloseIgnored",
        }
    }

    /// Whether a try region can catch this kind. Resource exhaustion and
    /// `unreachable` always abort the run; every other kind is raised as an
    /// error (or aborts, if the instruction's policy says `trap`).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ErrorKind;
    ///
    /// assert!(ErrorKind::KeyNotFound.is_catchable());
    /// assert!(!ErrorKind::Unreachable.is_catchable());
    /// ```
    #[must_use]
    pub const fn is_catchable(self) -> bool {
        !matches!(
            self,
            ErrorKind::OutOfMemory | ErrorKind::OutOfFuel | ErrorKind::Unreachable
        )
    }
}

/// `E0002 DivByZero`.
impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "E{:04} {}", self.code(), self.name())
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;

    #[test]
    fn strings_store_back_to_back() {
        let mut s = Strings::default();
        assert!(s.push("ab"));
        assert!(s.push(""));
        assert!(s.push("é"));
        assert_eq!(s.get(0), Some("ab"));
        assert_eq!(s.get(1), Some(""));
        assert_eq!(s.get(2), Some("é"));
        assert_eq!(s.get(3), None);
        assert_eq!(s.iter().count(), 3);
    }

    #[test]
    fn error_codes_are_unique_and_round_trip() {
        for &kind in ErrorKind::ALL {
            assert_eq!(ErrorKind::from_code(kind.code()), Some(kind));
        }
        assert_eq!(ErrorKind::ArithOverflow.to_string(), "E0001 ArithOverflow");
        assert_eq!(ErrorKind::Unreachable.to_string(), "E0109 Unreachable");
    }

    #[test]
    fn const_children_cover_arrays_and_maps() {
        let map = Const::Map(vec![(ConstId(0), ConstId(1)), (ConstId(2), ConstId(3))]);
        assert_eq!(
            map.children().collect::<Vec<_>>(),
            [ConstId(0), ConstId(1), ConstId(2), ConstId(3)]
        );
        assert_eq!(Const::Int(1).children().count(), 0);
    }

    #[test]
    fn nan_constants_print_their_payload() {
        assert_eq!(
            Const::F64(0x7ff8_0000_0000_0001).to_string(),
            "f64 nan:0x7ff8000000000001"
        );
        assert_eq!(Const::F32(0x7fc0_0000).to_string(), "f32 nan:0x7fc00000");
        assert_eq!(
            Const::Bytes(vec![b'a', 0xff]).to_string(),
            "bytes b\"a\\xff\""
        );
    }
}
