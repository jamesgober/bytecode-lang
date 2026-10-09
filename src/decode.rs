//! The budgeted binary decoder.
//!
//! The decoder treats its input as hostile. It never panics, never recurses,
//! and never allocates more than the input can justify: every count is
//! checked against its [`Limits`] entry *and* against the bytes that remain
//! (each element needs at least a known number of bytes) before anything is
//! reserved. It checks the format: the header, the section order and
//! lengths, every tag, UTF-8, `char` validity, canonical instructions, the
//! constant DAG and its depth. It does not check meaning (whether an index
//! is in range, whether registers are typed correctly); that is the
//! verifier's job (`specs/LSB.md` §8).

use alloc::vec::Vec;
use core::fmt;

use crate::call::{ArgKind, CallShape, MAX_ARITY, Param, ParamKind, ParamList};
use crate::encode::{SECTIONS, section};
use crate::ids::{ConstId, FuncId, GlobalId, ImportId, Reg, StrId, Target, TypeId};
use crate::inst::{Inst, InstError};
use crate::module::{
    Callee, Const, Export, ExportItem, Function, Global, Handler, Hook, HookBinding, Import,
    JumpTable, LineRow, LocalVar, Module, Strings,
};
use crate::types::{Field, FuncType, Method, StructDef, TypeDef, ValType};
use crate::{FORMAT_VERSION, MAGIC};

/// The budgets the decoder enforces on untrusted input.
///
/// The defaults admit any realistic module (hundreds of megabytes of code)
/// while bounding the work and memory a hostile file can demand. Lower them
/// for a sandbox; there is no "unlimited" mode, because the format itself
/// caps registers, name refs, type refs, and captures at 65,536 per function.
/// Independently of these numbers, no list is allocated before the input is
/// known to be long enough to hold it.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{decode_with, encode, DecodeErrorKind, Limit, Limits, ModuleBuilder};
///
/// let mut m = ModuleBuilder::new();
/// m.string("a");
/// m.string("b");
/// let bytes = encode(&m.finish().unwrap());
///
/// let mut limits = Limits::default();
/// limits.max_strings = 1;
/// let err = decode_with(&bytes, &limits).unwrap_err();
/// assert_eq!(err.kind(), &DecodeErrorKind::LimitExceeded(Limit::Strings));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum input size in bytes. Default 256 MiB.
    pub max_bytes: usize,
    /// Maximum string-table entries. Default 4,194,304.
    pub max_strings: usize,
    /// Maximum type-table entries. Default 1,048,576.
    pub max_types: usize,
    /// Maximum constant-pool entries. Default 4,194,304.
    pub max_consts: usize,
    /// Maximum nesting depth of aggregate constants (a scalar is depth 1).
    /// Default 64.
    pub max_const_depth: u32,
    /// Maximum functions. Default 1,048,576.
    pub max_functions: usize,
    /// Maximum instructions in one function. Default 16,777,216.
    pub max_insts: usize,
    /// Maximum instructions in the whole module. Default 33,554,432.
    pub max_total_insts: usize,
    /// Maximum length of every other list: imports, globals, exports,
    /// parameters, methods, constant elements, byte-string constants (in
    /// bytes), jump-table entries, handlers, line rows, locals. Default
    /// 16,777,216.
    pub max_items: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_bytes: 256 << 20,
            max_strings: 1 << 22,
            max_types: 1 << 20,
            max_consts: 1 << 22,
            max_const_depth: 64,
            max_functions: 1 << 20,
            max_insts: 1 << 24,
            max_total_insts: 1 << 25,
            max_items: 1 << 24,
        }
    }
}

/// Which budget a [`DecodeErrorKind::LimitExceeded`] refers to.
///
/// # Examples
///
/// ```
/// use bytecode_lang::Limit;
///
/// assert_eq!(Limit::ConstDepth.to_string(), "constant nesting depth");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum Limit {
    /// [`Limits::max_bytes`].
    Bytes,
    /// [`Limits::max_strings`].
    Strings,
    /// [`Limits::max_types`].
    Types,
    /// [`Limits::max_consts`].
    Consts,
    /// [`Limits::max_const_depth`].
    ConstDepth,
    /// [`Limits::max_functions`].
    Functions,
    /// [`Limits::max_insts`].
    Insts,
    /// [`Limits::max_total_insts`].
    TotalInsts,
    /// [`Limits::max_items`].
    Items,
    /// The format's cap of 65,536 entries on the tables a 16-bit operand
    /// indexes: a function's registers, captures, name refs, type refs, and
    /// call shapes, and a struct's fields.
    PerFunction,
    /// The format's cap of 255 entries on a parameter list or a call shape
    /// (call windows and argument counts are 8-bit).
    Arity,
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Limit::Bytes => "input size",
            Limit::Strings => "string count",
            Limit::Types => "type count",
            Limit::Consts => "constant count",
            Limit::ConstDepth => "constant nesting depth",
            Limit::Functions => "function count",
            Limit::Insts => "instructions per function",
            Limit::TotalInsts => "total instructions",
            Limit::Items => "list length",
            Limit::PerFunction => "16-bit table size (65,536 entries)",
            Limit::Arity => "arity (255 parameters or arguments)",
        })
    }
}

/// What is wrong with the input; see [`DecodeError`].
///
/// # Examples
///
/// ```
/// use bytecode_lang::{decode, DecodeErrorKind};
///
/// assert_eq!(decode(b"LS").unwrap_err().kind(), &DecodeErrorKind::UnexpectedEnd);
/// assert_eq!(decode(b"MZ\0\0\x01\0\0\0\0\0\0\0").unwrap_err().kind(), &DecodeErrorKind::BadMagic);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum DecodeErrorKind {
    /// The input ends before a value it promises.
    UnexpectedEnd,
    /// The first four bytes are not `LSB\0`.
    BadMagic,
    /// The format version is not one this decoder implements.
    UnsupportedVersion(u32),
    /// The header's reserved flags are not zero.
    ReservedFlags(u32),
    /// A section id is not the next one in the fixed order.
    SectionOutOfOrder {
        /// The id that should come next.
        expected: u32,
        /// The id found.
        found: u32,
    },
    /// A section's contents do not fill exactly its declared length.
    SectionLength(u32),
    /// Bytes follow the last section.
    TrailingBytes,
    /// A budget was exceeded.
    LimitExceeded(Limit),
    /// A string is not valid UTF-8.
    InvalidUtf8,
    /// A tag byte names nothing (`what` says which kind of tag).
    InvalidTag {
        /// The kind of tag: `"value type"`, `"type"`, `"constant"`, ...
        what: &'static str,
        /// The byte found.
        tag: u8,
    },
    /// A `char` constant is not a Unicode scalar value.
    InvalidChar(u32),
    /// An aggregate constant refers to itself or a later constant.
    ConstForwardRef {
        /// The aggregate.
        index: u32,
        /// The offending element.
        child: u32,
    },
    /// Hook bindings are not in strictly increasing hook order.
    HookOrder,
    /// The debug section's function count differs from the function
    /// section's.
    DebugCount {
        /// Functions in the module.
        expected: u32,
        /// Debug entries found.
        found: u32,
    },
    /// An instruction is malformed.
    Inst(InstError),
}

impl fmt::Display for DecodeErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeErrorKind::UnexpectedEnd => f.write_str("unexpected end of input"),
            DecodeErrorKind::BadMagic => f.write_str("not an LSB module (bad magic)"),
            DecodeErrorKind::UnsupportedVersion(v) => write!(
                f,
                "unsupported format version {v} (this decoder reads version {FORMAT_VERSION})"
            ),
            DecodeErrorKind::ReservedFlags(v) => write!(f, "reserved header flags set ({v:#x})"),
            DecodeErrorKind::SectionOutOfOrder { expected, found } => {
                write!(f, "expected section {expected}, found {found}")
            }
            DecodeErrorKind::SectionLength(id) => {
                write!(f, "section {id} does not match its declared length")
            }
            DecodeErrorKind::TrailingBytes => f.write_str("bytes after the last section"),
            DecodeErrorKind::LimitExceeded(limit) => write!(f, "{limit} limit exceeded"),
            DecodeErrorKind::InvalidUtf8 => f.write_str("string is not valid UTF-8"),
            DecodeErrorKind::InvalidTag { what, tag } => write!(f, "invalid {what} tag {tag}"),
            DecodeErrorKind::InvalidChar(v) => {
                write!(f, "{v:#x} is not a Unicode scalar value")
            }
            DecodeErrorKind::ConstForwardRef { index, child } => write!(
                f,
                "constant k{index} refers to k{child}, which is not an earlier constant"
            ),
            DecodeErrorKind::HookOrder => {
                f.write_str("hook bindings are not in strictly increasing order")
            }
            DecodeErrorKind::DebugCount { expected, found } => write!(
                f,
                "debug section describes {found} functions, the module has {expected}"
            ),
            DecodeErrorKind::Inst(e) => write!(f, "{e}"),
        }
    }
}

/// Why bytes are not a module, and where.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{decode, DecodeErrorKind};
///
/// let err = decode(b"LSB\0\x09\0\0\0\0\0\0\0").unwrap_err();
/// assert_eq!(err.kind(), &DecodeErrorKind::UnsupportedVersion(9));
/// assert_eq!(err.offset(), 4);
/// assert_eq!(
///     err.to_string(),
///     "at byte 4: unsupported format version 9 (this decoder reads version 2)",
/// );
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecodeError {
    offset: usize,
    kind: DecodeErrorKind,
}

impl DecodeError {
    /// The byte offset in the input where the problem was found.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::decode;
    ///
    /// assert_eq!(decode(b"").unwrap_err().offset(), 0);
    /// ```
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// What the problem is.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{decode, DecodeErrorKind};
    ///
    /// assert_eq!(decode(b"").unwrap_err().kind(), &DecodeErrorKind::UnexpectedEnd);
    /// ```
    #[must_use]
    pub fn kind(&self) -> &DecodeErrorKind {
        &self.kind
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at byte {}: {}", self.offset, self.kind)
    }
}

impl core::error::Error for DecodeError {}

type Result<T> = core::result::Result<T, DecodeError>;

/// A cursor over the input with a movable end (the current section's).
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> Reader<'a> {
    fn fail<T>(&self, at: usize, kind: DecodeErrorKind) -> Result<T> {
        Err(DecodeError { offset: at, kind })
    }

    fn remaining(&self) -> usize {
        self.end.saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let slice = self
            .pos
            .checked_add(n)
            .filter(|&stop| stop <= self.end)
            .and_then(|stop| self.bytes.get(self.pos..stop));
        match slice {
            Some(s) => {
                self.pos += n;
                Ok(s)
            }
            None => self.fail(self.pos, DecodeErrorKind::UnexpectedEnd),
        }
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let at = self.pos;
        match <[u8; N]>::try_from(self.take(N)?) {
            Ok(a) => Ok(a),
            Err(_) => self.fail(at, DecodeErrorKind::UnexpectedEnd),
        }
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    /// Reads a list length and checks it against `limit` and against the
    /// remaining bytes (each element needs at least `min_size` bytes), so
    /// the caller may reserve that many elements.
    fn count(&mut self, limit: usize, which: Limit, min_size: usize) -> Result<usize> {
        let at = self.pos;
        let n = self.u32()? as usize;
        if n > limit {
            return self.fail(at, DecodeErrorKind::LimitExceeded(which));
        }
        if n.saturating_mul(min_size) > self.remaining() {
            return self.fail(at, DecodeErrorKind::UnexpectedEnd);
        }
        Ok(n)
    }

    fn bool(&mut self, what: &'static str) -> Result<bool> {
        let at = self.pos;
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => self.fail(at, DecodeErrorKind::InvalidTag { what, tag }),
        }
    }

    fn opt_u32(&mut self) -> Result<Option<u32>> {
        if self.bool("option")? {
            Ok(Some(self.u32()?))
        } else {
            Ok(None)
        }
    }

    fn val_type(&mut self) -> Result<ValType> {
        let at = self.pos;
        let tag = self.u8()?;
        if let Some(ty) = ValType::from_simple_tag(tag) {
            return Ok(ty);
        }
        if tag == ValType::Ref(TypeId(0)).tag() {
            return Ok(ValType::Ref(TypeId(self.u32()?)));
        }
        self.fail(
            at,
            DecodeErrorKind::InvalidTag {
                what: "value type",
                tag,
            },
        )
    }

    fn val_types(&mut self, limit: usize, which: Limit) -> Result<Vec<ValType>> {
        let n = self.count(limit, which, 1)?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.val_type()?);
        }
        Ok(out)
    }
}

/// The largest per-function table the format can address.
const PER_FUNCTION: usize = 1 << 16;

struct Decoder<'a, 'l> {
    r: Reader<'a>,
    limits: &'l Limits,
    total_insts: usize,
}

impl Decoder<'_, '_> {
    fn items(&mut self, min_size: usize) -> Result<usize> {
        self.r.count(self.limits.max_items, Limit::Items, min_size)
    }

    fn per_function(&mut self, min_size: usize) -> Result<usize> {
        let (limit, which) = if self.limits.max_items < PER_FUNCTION {
            (self.limits.max_items, Limit::Items)
        } else {
            (PER_FUNCTION, Limit::PerFunction)
        };
        self.r.count(limit, which, min_size)
    }

    fn strings(&mut self) -> Result<Strings> {
        let n = self.r.count(self.limits.max_strings, Limit::Strings, 4)?;
        let mut strings = Strings::with_capacity(n, self.r.remaining());
        for _ in 0..n {
            let len = self.r.u32()? as usize;
            let at = self.r.pos;
            let bytes = self.r.take(len)?;
            let Ok(s) = core::str::from_utf8(bytes) else {
                return self.r.fail(at, DecodeErrorKind::InvalidUtf8);
            };
            if !strings.push(s) {
                return self
                    .r
                    .fail(at, DecodeErrorKind::LimitExceeded(Limit::Bytes));
            }
        }
        Ok(strings)
    }

    fn type_def(&mut self) -> Result<TypeDef> {
        let at = self.r.pos;
        Ok(match self.r.u8()? {
            0 => {
                let params = self.r.val_types(self.limits.max_items, Limit::Items)?;
                let results = self.r.val_types(self.limits.max_items, Limit::Items)?;
                TypeDef::Func(FuncType { params, results })
            }
            1 => {
                let name = StrId(self.r.u32()?);
                let parent = self.r.opt_u32()?.map(TypeId);
                let n = self.per_function(5)?;
                let mut fields = Vec::with_capacity(n);
                for _ in 0..n {
                    let name = StrId(self.r.u32()?);
                    let ty = self.r.val_type()?;
                    fields.push(Field { name, ty });
                }
                let n = self.items(8)?;
                let mut methods = Vec::with_capacity(n);
                for _ in 0..n {
                    let name = StrId(self.r.u32()?);
                    let func = FuncId(self.r.u32()?);
                    methods.push(Method { name, func });
                }
                TypeDef::Struct(StructDef {
                    name,
                    parent,
                    fields,
                    methods,
                })
            }
            2 => TypeDef::Array(self.r.val_type()?),
            3 => {
                let key = self.r.val_type()?;
                let value = self.r.val_type()?;
                TypeDef::Map { key, value }
            }
            4 => TypeDef::Cell(self.r.val_type()?),
            5 => {
                let key = self.r.val_type()?;
                let value = self.r.val_type()?;
                TypeDef::Iter { key, value }
            }
            6 => TypeDef::Coroutine,
            tag => {
                return self
                    .r
                    .fail(at, DecodeErrorKind::InvalidTag { what: "type", tag });
            }
        })
    }

    fn types(&mut self) -> Result<Vec<TypeDef>> {
        // The smallest type (`coroutine`) is its tag alone.
        let n = self.r.count(self.limits.max_types, Limit::Types, 1)?;
        let mut types = Vec::with_capacity(n);
        for _ in 0..n {
            types.push(self.type_def()?);
        }
        Ok(types)
    }

    fn consts(&mut self) -> Result<Vec<Const>> {
        let n = self.r.count(self.limits.max_consts, Limit::Consts, 2)?;
        let mut consts = Vec::with_capacity(n);
        // depth[i] = nesting depth of constant i; children are earlier, so
        // one forward pass computes it with no recursion.
        let mut depth: Vec<u32> = Vec::with_capacity(n);
        for index in 0..n {
            let at = self.r.pos;
            let c = match self.r.u8()? {
                0 => Const::Bool(self.r.bool("boolean")?),
                1 => Const::Int(i64::from_le_bytes(self.r.array()?)),
                2 => Const::UInt(self.r.u64()?),
                3 => Const::F32(self.r.u32()?),
                4 => Const::F64(self.r.u64()?),
                5 => {
                    let char_at = self.r.pos;
                    let v = self.r.u32()?;
                    match char::from_u32(v) {
                        Some(c) => Const::Char(c),
                        None => return self.r.fail(char_at, DecodeErrorKind::InvalidChar(v)),
                    }
                }
                6 => Const::Str(StrId(self.r.u32()?)),
                7 => {
                    let len = self.items(1)?;
                    Const::Bytes(self.r.take(len)?.to_vec())
                }
                8 => {
                    let len = self.items(4)?;
                    let mut items = Vec::with_capacity(len);
                    for _ in 0..len {
                        items.push(ConstId(self.r.u32()?));
                    }
                    Const::Array(items)
                }
                9 => {
                    let len = self.items(8)?;
                    let mut entries = Vec::with_capacity(len);
                    for _ in 0..len {
                        let k = ConstId(self.r.u32()?);
                        let v = ConstId(self.r.u32()?);
                        entries.push((k, v));
                    }
                    Const::Map(entries)
                }
                tag => {
                    return self.r.fail(
                        at,
                        DecodeErrorKind::InvalidTag {
                            what: "constant",
                            tag,
                        },
                    );
                }
            };
            let mut d = 1u32;
            for child in c.children() {
                match depth.get(child.index()) {
                    Some(&cd) => d = d.max(cd.saturating_add(1)),
                    None => {
                        let index = u32::try_from(index).unwrap_or(u32::MAX);
                        let kind = DecodeErrorKind::ConstForwardRef {
                            index,
                            child: child.0,
                        };
                        return self.r.fail(at, kind);
                    }
                }
            }
            if d > self.limits.max_const_depth {
                return self
                    .r
                    .fail(at, DecodeErrorKind::LimitExceeded(Limit::ConstDepth));
            }
            depth.push(d);
            consts.push(c);
        }
        Ok(consts)
    }

    /// A byte that must be within `0..=max`, else an invalid tag.
    fn small(&mut self, max: u8, what: &'static str) -> Result<u8> {
        let at = self.r.pos;
        let v = self.r.u8()?;
        if v > max {
            return self
                .r
                .fail(at, DecodeErrorKind::InvalidTag { what, tag: v });
        }
        Ok(v)
    }

    /// `opt<paramlist>` (see the encoder's `param_list`).
    fn param_list(&mut self) -> Result<Option<ParamList>> {
        if !self.r.bool("option")? {
            return Ok(None);
        }
        let ignore_extra = self.small(1, "parameter list flags")? == 1;
        // Kind, flags, and an absent name: three bytes at least.
        let n = self.r.count(MAX_ARITY, Limit::Arity, 3)?;
        let mut params = Vec::with_capacity(n);
        for _ in 0..n {
            let at = self.r.pos;
            let code = self.r.u8()?;
            let Some(kind) = ParamKind::from_code(code) else {
                return self.r.fail(
                    at,
                    DecodeErrorKind::InvalidTag {
                        what: "parameter kind",
                        tag: code,
                    },
                );
            };
            let flags = self.small(3, "parameter flags")?;
            let name = self.r.opt_u32()?.map(StrId);
            params.push(Param {
                name,
                kind,
                by_ref: flags & 1 != 0,
                default: flags & 2 != 0,
            });
        }
        Ok(Some(ParamList {
            params,
            ignore_extra,
        }))
    }

    /// A function's call shapes (see the encoder's `shapes`).
    fn shapes(&mut self) -> Result<Vec<CallShape>> {
        let n = self.per_function(4)?;
        let mut shapes = Vec::with_capacity(n);
        for _ in 0..n {
            let len = self.r.count(MAX_ARITY, Limit::Arity, 1)?;
            let mut args = Vec::with_capacity(len);
            for _ in 0..len {
                let at = self.r.pos;
                args.push(match self.r.u8()? {
                    0 => ArgKind::Positional,
                    1 => ArgKind::Named(StrId(self.r.u32()?)),
                    2 => ArgKind::Spread,
                    3 => ArgKind::SpreadNamed,
                    tag => {
                        return self.r.fail(
                            at,
                            DecodeErrorKind::InvalidTag {
                                what: "call shape argument",
                                tag,
                            },
                        );
                    }
                });
            }
            shapes.push(CallShape { args });
        }
        Ok(shapes)
    }

    fn imports(&mut self) -> Result<Vec<Import>> {
        // Three ids and an absent parameter list.
        let n = self.items(13)?;
        let mut imports = Vec::with_capacity(n);
        for _ in 0..n {
            let module = StrId(self.r.u32()?);
            let name = StrId(self.r.u32()?);
            let sig = TypeId(self.r.u32()?);
            let params = self.param_list()?;
            imports.push(Import {
                module,
                name,
                sig,
                params,
            });
        }
        Ok(imports)
    }

    fn globals(&mut self) -> Result<Vec<Global>> {
        let n = self.items(7)?;
        let mut globals = Vec::with_capacity(n);
        for _ in 0..n {
            let name = StrId(self.r.u32()?);
            let ty = self.r.val_type()?;
            let mutable = self.r.bool("boolean")?;
            let init = self.r.opt_u32()?.map(ConstId);
            globals.push(Global {
                name,
                ty,
                mutable,
                init,
            });
        }
        Ok(globals)
    }

    fn function(&mut self) -> Result<Function> {
        let name = StrId(self.r.u32()?);
        let sig = TypeId(self.r.u32()?);
        let params = self.param_list()?;
        let regs = {
            let n = self.per_function(1)?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(self.r.val_type()?);
            }
            v
        };
        let captures = {
            let n = self.per_function(1)?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(self.r.val_type()?);
            }
            v
        };
        let names = {
            let n = self.per_function(4)?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(StrId(self.r.u32()?));
            }
            v
        };
        let type_refs = {
            let n = self.per_function(4)?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(TypeId(self.r.u32()?));
            }
            v
        };
        let tables = {
            let n = self.items(8)?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                let default = Target(self.r.u32()?);
                let len = self.items(4)?;
                let mut targets = Vec::with_capacity(len);
                for _ in 0..len {
                    targets.push(Target(self.r.u32()?));
                }
                v.push(JumpTable { targets, default });
            }
            v
        };
        let shapes = self.shapes()?;
        let handlers = {
            let n = self.items(14)?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                let start = self.r.u32()?;
                let end = self.r.u32()?;
                let target = Target(self.r.u32()?);
                let catch = Reg(self.r.u16()?);
                v.push(Handler {
                    start,
                    end,
                    target,
                    catch,
                });
            }
            v
        };
        let code = {
            let at = self.r.pos;
            let n = self.r.count(self.limits.max_insts, Limit::Insts, 8)?;
            self.total_insts = self.total_insts.saturating_add(n);
            if self.total_insts > self.limits.max_total_insts {
                return self
                    .r
                    .fail(at, DecodeErrorKind::LimitExceeded(Limit::TotalInsts));
            }
            // `count` proved the bytes are there; take them in one slice and
            // decode word by word with no per-word bounds bookkeeping.
            let start = self.r.pos;
            let words = self.r.take(n.saturating_mul(8))?;
            let mut code = Vec::with_capacity(n);
            for (i, word) in words.chunks_exact(8).enumerate() {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(word);
                match Inst::from_bytes(bytes) {
                    Ok(inst) => code.push(inst),
                    Err(e) => return self.r.fail(start + i * 8, DecodeErrorKind::Inst(e)),
                }
            }
            code
        };
        Ok(Function {
            name,
            sig,
            params,
            regs,
            captures,
            names,
            type_refs,
            tables,
            shapes,
            handlers,
            code,
            lines: Vec::new(),
            locals: Vec::new(),
        })
    }

    fn functions(&mut self) -> Result<Vec<Function>> {
        // Name, signature, an absent parameter list, and eight list lengths.
        let n = self
            .r
            .count(self.limits.max_functions, Limit::Functions, 41)?;
        let mut functions = Vec::with_capacity(n);
        for _ in 0..n {
            functions.push(self.function()?);
        }
        Ok(functions)
    }

    fn exports(&mut self) -> Result<Vec<Export>> {
        let n = self.items(9)?;
        let mut exports = Vec::with_capacity(n);
        for _ in 0..n {
            let name = StrId(self.r.u32()?);
            let at = self.r.pos;
            let tag = self.r.u8()?;
            let index = self.r.u32()?;
            let item = match tag {
                0 => ExportItem::Func(FuncId(index)),
                1 => ExportItem::Global(GlobalId(index)),
                2 => ExportItem::Type(TypeId(index)),
                tag => {
                    return self.r.fail(
                        at,
                        DecodeErrorKind::InvalidTag {
                            what: "export",
                            tag,
                        },
                    );
                }
            };
            exports.push(Export { name, item });
        }
        Ok(exports)
    }

    fn hooks(&mut self) -> Result<Vec<HookBinding>> {
        let n = self.r.count(Hook::ALL.len(), Limit::Items, 6)?;
        let mut hooks: Vec<HookBinding> = Vec::with_capacity(n);
        for _ in 0..n {
            let at = self.r.pos;
            let code = self.r.u8()?;
            let Some(hook) = Hook::from_code(code) else {
                return self.r.fail(
                    at,
                    DecodeErrorKind::InvalidTag {
                        what: "hook",
                        tag: code,
                    },
                );
            };
            if hooks.last().is_some_and(|prev| prev.hook >= hook) {
                return self.r.fail(at, DecodeErrorKind::HookOrder);
            }
            let callee_at = self.r.pos;
            let tag = self.r.u8()?;
            let index = self.r.u32()?;
            let callee = match tag {
                0 => Callee::Func(FuncId(index)),
                1 => Callee::Import(ImportId(index)),
                tag => {
                    return self.r.fail(
                        callee_at,
                        DecodeErrorKind::InvalidTag {
                            what: "callee",
                            tag,
                        },
                    );
                }
            };
            hooks.push(HookBinding { hook, callee });
        }
        Ok(hooks)
    }

    fn debug(&mut self, functions: &mut [Function]) -> Result<()> {
        let at = self.r.pos;
        let n = self.r.u32()?;
        if n as usize != functions.len() {
            let expected = u32::try_from(functions.len()).unwrap_or(u32::MAX);
            return self
                .r
                .fail(at, DecodeErrorKind::DebugCount { expected, found: n });
        }
        for f in functions {
            let rows = self.items(16)?;
            f.lines.reserve_exact(rows);
            for _ in 0..rows {
                let pc = self.r.u32()?;
                let file = StrId(self.r.u32()?);
                let line = self.r.u32()?;
                let column = self.r.u32()?;
                f.lines.push(LineRow {
                    pc,
                    file,
                    line,
                    column,
                });
            }
            let locals = self.items(14)?;
            f.locals.reserve_exact(locals);
            for _ in 0..locals {
                let reg = Reg(self.r.u16()?);
                let name = StrId(self.r.u32()?);
                let start = self.r.u32()?;
                let end = self.r.u32()?;
                f.locals.push(LocalVar {
                    reg,
                    name,
                    start,
                    end,
                });
            }
        }
        Ok(())
    }

    /// Enters section `id`: checks its header and narrows the reader to it.
    /// Returns the end of the enclosing region to restore afterwards.
    fn open(&mut self, id: u32) -> Result<usize> {
        let at = self.r.pos;
        let found = self.r.u32()?;
        if found != id {
            return self.r.fail(
                at,
                DecodeErrorKind::SectionOutOfOrder {
                    expected: id,
                    found,
                },
            );
        }
        let len = self.r.u32()? as usize;
        let start = self.r.pos;
        match start.checked_add(len).filter(|&end| end <= self.r.end) {
            Some(end) => {
                let outer = self.r.end;
                self.r.end = end;
                Ok(outer)
            }
            None => self.r.fail(start, DecodeErrorKind::UnexpectedEnd),
        }
    }

    /// Leaves section `id`, which must have been consumed exactly.
    fn close(&mut self, id: u32, outer: usize) -> Result<()> {
        if self.r.pos != self.r.end {
            return self.r.fail(self.r.pos, DecodeErrorKind::SectionLength(id));
        }
        self.r.end = outer;
        Ok(())
    }
}

/// Decodes a module; see [`crate::decode_with`].
pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Module> {
    if bytes.len() > limits.max_bytes {
        return Err(DecodeError {
            offset: 0,
            kind: DecodeErrorKind::LimitExceeded(Limit::Bytes),
        });
    }
    let mut d = Decoder {
        r: Reader {
            bytes,
            pos: 0,
            end: bytes.len(),
        },
        limits,
        total_insts: 0,
    };
    let magic: [u8; 4] = d.r.array()?;
    if magic != MAGIC {
        return d.r.fail(0, DecodeErrorKind::BadMagic);
    }
    let version = d.r.u32()?;
    if version != FORMAT_VERSION {
        return d.r.fail(4, DecodeErrorKind::UnsupportedVersion(version));
    }
    let flags = d.r.u32()?;
    if flags != 0 {
        return d.r.fail(8, DecodeErrorKind::ReservedFlags(flags));
    }

    let mut module = Module::default();
    for id in (1u32..).take(SECTIONS) {
        let outer = d.open(id)?;
        match id {
            section::STRINGS => module.strings = d.strings()?,
            section::TYPES => module.types = d.types()?,
            section::CONSTS => module.consts = d.consts()?,
            section::IMPORTS => module.imports = d.imports()?,
            section::GLOBALS => module.globals = d.globals()?,
            section::FUNCTIONS => module.functions = d.functions()?,
            section::EXPORTS => module.exports = d.exports()?,
            section::HOOKS => module.hooks = d.hooks()?,
            section::META => {
                module.name = d.r.opt_u32()?.map(StrId);
                module.start = d.r.opt_u32()?.map(FuncId);
            }
            _ => d.debug(&mut module.functions)?,
        }
        d.close(id, outer)?;
    }
    if d.r.pos != bytes.len() {
        return d.r.fail(d.r.pos, DecodeErrorKind::TrailingBytes);
    }
    Ok(module)
}
