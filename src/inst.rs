//! The instruction set.
//!
//! One table, the `isa!` invocation below, defines every instruction: its
//! opcode byte, its mnemonic, and each operand's name, type, and slot in the
//! eight-byte word. The [`Inst`] enum, [`Opcode`], the per-opcode
//! [`FieldSpec`] metadata, the encoder ([`Inst::to_bytes`]), the decoder
//! ([`Inst::from_bytes`]), and the disassembler's rendering are all generated
//! from that one table, so they cannot disagree.
//!
//! # The eight-byte word
//!
//! ```text
//! byte  0      1      2..4   4..6   6..8
//!       opcode A      B      C      D
//!                            \----W----/
//! ```
//!
//! `A` is one byte (a modifier: a type, a policy, a count), `B`, `C`, and `D`
//! are 16-bit (registers and per-function refs), and `W` is the 32-bit word
//! overlapping `C` and `D` (module indices, branch targets, immediates). All
//! little-endian. Bytes of slots an instruction does not use are zero, and the
//! decoder rejects anything else, so every instruction has exactly one
//! encoding. The decoded [`Inst`] is itself eight bytes and `Copy`: a function
//! body is a flat `&[Inst]` that an interpreter walks with a program counter
//! and never re-parses (the bvm-lang design).

use core::fmt;

use crate::call::ArgKind;
use crate::ids::{
    ConstId, FieldIdx, FuncId, GlobalId, ImportId, NameRef, Reg, ShapeId, TableId, Target, TypeRef,
    UpvalIdx,
};
use crate::module::{Const, ErrorKind, Function, Module};
use crate::policy::{FloatConv, FloatTy, IntConv, IntOp, IntPair, IntTy, Overflow, Policy};
use crate::types::{Kind, Prim};

/// Where an operand lives in the eight-byte instruction word.
///
/// # Examples
///
/// ```
/// use bytecode_lang::Slot;
///
/// assert_eq!(Slot::A.width(), 8);
/// assert_eq!(Slot::W.width(), 32);
/// assert_eq!(Slot::C.shift(), 32);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Slot {
    /// Byte 1: an 8-bit modifier or count.
    A,
    /// Bytes 2–3: a 16-bit operand.
    B,
    /// Bytes 4–5: a 16-bit operand.
    C,
    /// Bytes 6–7: a 16-bit operand.
    D,
    /// Bytes 4–7: a 32-bit operand (overlaps `C` and `D`).
    W,
}

impl Slot {
    /// The slot's bit offset within the little-endian 64-bit word.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Slot;
    ///
    /// assert_eq!(Slot::A.shift(), 8);
    /// assert_eq!(Slot::D.shift(), 48);
    /// ```
    #[must_use]
    pub const fn shift(self) -> u32 {
        match self {
            Slot::A => 8,
            Slot::B => 16,
            Slot::C | Slot::W => 32,
            Slot::D => 48,
        }
    }

    /// The slot's width in bits.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Slot;
    ///
    /// assert_eq!(Slot::B.width(), 16);
    /// ```
    #[must_use]
    pub const fn width(self) -> u32 {
        match self {
            Slot::A => 8,
            Slot::B | Slot::C | Slot::D => 16,
            Slot::W => 32,
        }
    }

    const fn mask(self) -> u64 {
        (1u64 << self.width()) - 1
    }
}

/// What an operand is: the type of one [`Inst`] field.
///
/// Modifier kinds ([`is_modifier`](FieldKind::is_modifier)) print as a
/// mnemonic suffix (`iadd.i64.wrap`); the others print as operands.
///
/// # Examples
///
/// ```
/// use bytecode_lang::FieldKind;
///
/// assert!(FieldKind::IntOp.is_modifier());
/// assert!(!FieldKind::Reg.is_modifier());
/// assert_eq!(FieldKind::Target.bits(), 32);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FieldKind {
    /// A [`Reg`].
    Reg,
    /// A branch [`Target`].
    Target,
    /// A [`ConstId`].
    Const,
    /// A [`FuncId`].
    Func,
    /// An [`ImportId`].
    Import,
    /// A [`GlobalId`].
    Global,
    /// A [`TableId`].
    Table,
    /// A [`NameRef`].
    Name,
    /// A [`TypeRef`].
    TypeRef,
    /// A [`FieldIdx`].
    Field,
    /// An [`UpvalIdx`].
    Upval,
    /// A [`ShapeId`].
    Shape,
    /// A 32-bit signed immediate.
    Imm32,
    /// An 8-bit count (arguments, string parts).
    Count,
    /// A boolean flag.
    Bool,
    /// An [`IntTy`] modifier.
    IntTy,
    /// A [`FloatTy`] modifier.
    FloatTy,
    /// A [`Policy`] modifier.
    Policy,
    /// An [`IntOp`] modifier.
    IntOp,
    /// An [`IntConv`] modifier.
    IntConv,
    /// An [`IntPair`] modifier.
    IntPair,
    /// A [`FloatConv`] modifier.
    FloatConv,
    /// A [`Kind`] modifier.
    Kind,
    /// A [`Prim`] modifier.
    Prim,
    /// An [`ErrorKind`] modifier (its numeric code, one byte; catchable
    /// kinds only).
    ErrKind,
}

impl FieldKind {
    /// Whether the field is a modifier (printed as a mnemonic suffix).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::FieldKind;
    ///
    /// assert!(FieldKind::Kind.is_modifier());
    /// assert!(!FieldKind::Count.is_modifier());
    /// ```
    #[must_use]
    pub const fn is_modifier(self) -> bool {
        matches!(
            self,
            FieldKind::IntTy
                | FieldKind::FloatTy
                | FieldKind::Policy
                | FieldKind::IntOp
                | FieldKind::IntConv
                | FieldKind::IntPair
                | FieldKind::FloatConv
                | FieldKind::Kind
                | FieldKind::Prim
                | FieldKind::ErrKind
        )
    }

    /// The field's width in bits, which is also the width of the slot it
    /// must occupy.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::FieldKind;
    ///
    /// assert_eq!(FieldKind::Reg.bits(), 16);
    /// assert_eq!(FieldKind::Policy.bits(), 8);
    /// ```
    #[must_use]
    pub const fn bits(self) -> u32 {
        match self {
            FieldKind::Reg
            | FieldKind::Name
            | FieldKind::TypeRef
            | FieldKind::Field
            | FieldKind::Upval
            | FieldKind::Shape => 16,
            FieldKind::Target
            | FieldKind::Const
            | FieldKind::Func
            | FieldKind::Import
            | FieldKind::Global
            | FieldKind::Table
            | FieldKind::Imm32 => 32,
            _ => 8,
        }
    }
}

/// One operand of an opcode: its field name, its kind, and its slot.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{FieldKind, Opcode, Slot};
///
/// let fields = Opcode::Mov.fields();
/// assert_eq!(fields[0].name, "dst");
/// assert_eq!((fields[0].kind, fields[0].slot), (FieldKind::Reg, Slot::B));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FieldSpec {
    /// The [`Inst`] field's name.
    pub name: &'static str,
    /// What the field is.
    pub kind: FieldKind,
    /// Where it lives in the instruction word.
    pub slot: Slot,
}

/// Why eight bytes are not an instruction.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Inst, InstError, Opcode};
///
/// assert_eq!(Inst::from_bytes([0xff, 0, 0, 0, 0, 0, 0, 0]), Err(InstError::UnknownOpcode(0xff)));
/// // `nop` with a non-zero unused byte has two encodings otherwise, so it is refused.
/// assert_eq!(Inst::from_bytes([0x00, 1, 0, 0, 0, 0, 0, 0]), Err(InstError::NonCanonical(Opcode::Nop)));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum InstError {
    /// The opcode byte names no instruction.
    UnknownOpcode(u8),
    /// A modifier field holds a value its type does not define (for
    /// example an integer type code above 7, or a boolean above 1).
    InvalidOperand {
        /// The instruction.
        opcode: Opcode,
        /// The field's name.
        field: &'static str,
    },
    /// A byte the instruction does not use is not zero.
    NonCanonical(Opcode),
}

impl fmt::Display for InstError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstError::UnknownOpcode(op) => write!(f, "unknown opcode {op:#04x}"),
            InstError::InvalidOperand { opcode, field } => {
                write!(f, "invalid `{field}` operand of `{}`", opcode.mnemonic())
            }
            InstError::NonCanonical(opcode) => {
                write!(
                    f,
                    "`{}` has non-zero bytes in unused slots",
                    opcode.mnemonic()
                )
            }
        }
    }
}

impl core::error::Error for InstError {}

/// Rendering context: label positions and the module, when the caller has
/// them (the module disassembler), or nothing (`Inst`'s `Display`).
#[derive(Clone, Copy)]
pub(crate) struct Cx<'a> {
    /// Sorted pcs that are branch targets; `L<i>` names `labels[i]`.
    pub(crate) labels: &'a [u32],
    pub(crate) module: Option<&'a Module>,
    pub(crate) func: Option<&'a Function>,
}

impl Cx<'_> {
    pub(crate) const EMPTY: Cx<'static> = Cx {
        labels: &[],
        module: None,
        func: None,
    };
}

/// Writes ` ; a, b` comments after an instruction, starting the comment
/// only if there is something to say.
pub(crate) struct Note<'w> {
    w: &'w mut dyn fmt::Write,
    started: bool,
}

impl Note<'_> {
    fn item(&mut self, args: fmt::Arguments<'_>) -> fmt::Result {
        self.w.write_str(if self.started { ", " } else { "  ; " })?;
        self.started = true;
        self.w.write_fmt(args)
    }
}

/// Writes `s` as a quoted, escaped string of at most 40 characters.
pub(crate) fn write_quoted(w: &mut dyn fmt::Write, s: &str) -> fmt::Result {
    const MAX: usize = 40;
    match s.char_indices().nth(MAX) {
        Some((cut, _)) => write!(w, "{:?}...", s.get(..cut).unwrap_or(s)),
        None => write!(w, "{s:?}"),
    }
}

/// A short description of a constant for disassembly comments.
pub(crate) fn write_const_preview(
    w: &mut dyn fmt::Write,
    module: &Module,
    c: &Const,
) -> fmt::Result {
    match c {
        Const::Str(id) => match module.string(*id) {
            Some(s) => write_quoted(w, s),
            None => write!(w, "str {id} <invalid>"),
        },
        Const::Bytes(bytes) => write!(w, "bytes[{}]", bytes.len()),
        Const::Array(items) => write!(w, "array[{}]", items.len()),
        Const::Map(entries) => write!(w, "map[{}]", entries.len()),
        other => write!(w, "{other}"),
    }
}

/// A type that can sit in an instruction slot.
pub(crate) trait Operand: Copy + fmt::Display {
    const KIND: FieldKind;

    fn to_raw(self) -> u32;

    /// The value for a raw slot, or `None` if the raw value names none.
    /// Strict: `to_raw(from_raw(r)?) == r` for every accepted `r`.
    fn from_raw(raw: u32) -> Option<Self>;

    /// Modifiers print `.value` after the mnemonic.
    fn suffix(self, w: &mut dyn fmt::Write) -> fmt::Result {
        if Self::KIND.is_modifier() {
            write!(w, ".{self}")
        } else {
            Ok(())
        }
    }

    fn render(self, w: &mut dyn fmt::Write, _cx: &Cx<'_>) -> fmt::Result {
        write!(w, "{self}")
    }

    fn comment(self, _cx: &Cx<'_>, _note: &mut Note<'_>) -> fmt::Result {
        Ok(())
    }

    /// The overflow policy this field carries, if it carries one.
    fn overflow(self) -> Option<Overflow> {
        None
    }

    /// The register this field names, if it is a register.
    fn as_reg(self) -> Option<Reg> {
        None
    }

    /// The branch target this field holds, if it is one.
    #[inline(always)]
    fn as_target(self) -> Option<Target> {
        None
    }

    /// Whether this field carries `overflow = promote`. A constant `false`
    /// for every kind but the three policy modifiers, so the generated
    /// per-instruction test folds to a bit test or to nothing.
    #[inline(always)]
    fn is_promote(self) -> bool {
        false
    }
}

/// Whether a field name is the conventional destination register.
fn is_dst(name: &str) -> bool {
    name == "dst"
}

macro_rules! operand_u16 {
    ($($ty:ident => $kind:ident),* $(,)?) => {$(
        impl Operand for $ty {
            const KIND: FieldKind = FieldKind::$kind;
            fn to_raw(self) -> u32 {
                u32::from(self.0)
            }
            fn from_raw(raw: u32) -> Option<Self> {
                u16::try_from(raw).ok().map($ty)
            }
        }
    )*};
}

macro_rules! operand_u32 {
    ($($ty:ident => $kind:ident),* $(,)?) => {$(
        impl Operand for $ty {
            const KIND: FieldKind = FieldKind::$kind;
            fn to_raw(self) -> u32 {
                self.0
            }
            fn from_raw(raw: u32) -> Option<Self> {
                Some($ty(raw))
            }
        }
    )*};
}

macro_rules! operand_code {
    ($($ty:ident => $kind:ident),* $(,)?) => {$(
        impl Operand for $ty {
            const KIND: FieldKind = FieldKind::$kind;
            fn to_raw(self) -> u32 {
                u32::from(self.code())
            }
            fn from_raw(raw: u32) -> Option<Self> {
                u8::try_from(raw).ok().and_then($ty::from_code)
            }
        }
    )*};
}

macro_rules! operand_bits {
    ($($ty:ident => $kind:ident),* $(,)?) => {$(
        impl Operand for $ty {
            const KIND: FieldKind = FieldKind::$kind;
            fn to_raw(self) -> u32 {
                u32::from(self.bits())
            }
            fn from_raw(raw: u32) -> Option<Self> {
                u8::try_from(raw).ok().and_then($ty::from_bits)
            }
        }
    )*};
}

operand_u16!(UpvalIdx => Upval);
operand_u32!(TableId => Table);
operand_code!(IntTy => IntTy, FloatTy => FloatTy, Kind => Kind, Prim => Prim);
operand_bits!(IntPair => IntPair, FloatConv => FloatConv);

impl Operand for Reg {
    const KIND: FieldKind = FieldKind::Reg;
    fn to_raw(self) -> u32 {
        u32::from(self.0)
    }
    fn from_raw(raw: u32) -> Option<Self> {
        u16::try_from(raw).ok().map(Reg)
    }
    fn as_reg(self) -> Option<Reg> {
        Some(self)
    }
}

impl Operand for IntOp {
    const KIND: FieldKind = FieldKind::IntOp;
    fn to_raw(self) -> u32 {
        u32::from(self.bits())
    }
    fn from_raw(raw: u32) -> Option<Self> {
        u8::try_from(raw).ok().and_then(IntOp::from_bits)
    }
    fn overflow(self) -> Option<Overflow> {
        Some(self.policy().overflow())
    }
    #[inline(always)]
    fn is_promote(self) -> bool {
        (self.bits() >> 3) & 3 == 3
    }
}

impl Operand for IntConv {
    const KIND: FieldKind = FieldKind::IntConv;
    fn to_raw(self) -> u32 {
        u32::from(self.bits())
    }
    fn from_raw(raw: u32) -> Option<Self> {
        u8::try_from(raw).ok().and_then(IntConv::from_bits)
    }
    fn overflow(self) -> Option<Overflow> {
        Some(IntConv::overflow(self))
    }
    #[inline(always)]
    fn is_promote(self) -> bool {
        self.bits() >> 6 == 3
    }
}

impl Operand for FieldIdx {
    const KIND: FieldKind = FieldKind::Field;
    fn to_raw(self) -> u32 {
        u32::from(self.0)
    }
    fn from_raw(raw: u32) -> Option<Self> {
        u16::try_from(raw).ok().map(FieldIdx)
    }
}

impl Operand for Policy {
    const KIND: FieldKind = FieldKind::Policy;
    fn to_raw(self) -> u32 {
        u32::from(self.bits())
    }
    fn from_raw(raw: u32) -> Option<Self> {
        u8::try_from(raw).ok().and_then(Policy::from_bits)
    }
    /// The default policy adds no suffix.
    fn suffix(self, w: &mut dyn fmt::Write) -> fmt::Result {
        if self.is_default() {
            Ok(())
        } else {
            write!(w, ".{self}")
        }
    }
    fn overflow(self) -> Option<Overflow> {
        Some(Policy::overflow(self))
    }
    #[inline(always)]
    fn is_promote(self) -> bool {
        self.bits() & 3 == 3
    }
}

impl Operand for Target {
    const KIND: FieldKind = FieldKind::Target;
    fn to_raw(self) -> u32 {
        self.0
    }
    fn from_raw(raw: u32) -> Option<Self> {
        Some(Target(raw))
    }
    #[inline(always)]
    fn as_target(self) -> Option<Target> {
        Some(self)
    }
    /// `L<n>` when the target has a label, `@pc` otherwise.
    fn render(self, w: &mut dyn fmt::Write, cx: &Cx<'_>) -> fmt::Result {
        match cx.labels.binary_search(&self.0) {
            Ok(label) => write!(w, "L{label}"),
            Err(_) => write!(w, "{self}"),
        }
    }
}

impl Operand for ConstId {
    const KIND: FieldKind = FieldKind::Const;
    fn to_raw(self) -> u32 {
        self.0
    }
    fn from_raw(raw: u32) -> Option<Self> {
        Some(ConstId(raw))
    }
    fn comment(self, cx: &Cx<'_>, note: &mut Note<'_>) -> fmt::Result {
        let Some(module) = cx.module else {
            return Ok(());
        };
        match module.constant(self) {
            Some(c) => {
                let mut text = alloc::string::String::new();
                write_const_preview(&mut text, module, c)?;
                note.item(format_args!("{text}"))
            }
            None => note.item(format_args!("{self} <invalid>")),
        }
    }
}

impl Operand for FuncId {
    const KIND: FieldKind = FieldKind::Func;
    fn to_raw(self) -> u32 {
        self.0
    }
    fn from_raw(raw: u32) -> Option<Self> {
        Some(FuncId(raw))
    }
    fn comment(self, cx: &Cx<'_>, note: &mut Note<'_>) -> fmt::Result {
        let Some(module) = cx.module else {
            return Ok(());
        };
        match module.function(self).and_then(|f| module.string(f.name())) {
            Some(name) => note.item(format_args!("{name:?}")),
            None => note.item(format_args!("{self} <invalid>")),
        }
    }
}

impl Operand for ImportId {
    const KIND: FieldKind = FieldKind::Import;
    fn to_raw(self) -> u32 {
        self.0
    }
    fn from_raw(raw: u32) -> Option<Self> {
        Some(ImportId(raw))
    }
    fn comment(self, cx: &Cx<'_>, note: &mut Note<'_>) -> fmt::Result {
        let Some(module) = cx.module else {
            return Ok(());
        };
        let names = module
            .import(self)
            .and_then(|i| Some((module.string(i.module)?, module.string(i.name)?)));
        match names {
            Some((m, n)) => note.item(format_args!("{m:?}.{n:?}")),
            None => note.item(format_args!("{self} <invalid>")),
        }
    }
}

impl Operand for GlobalId {
    const KIND: FieldKind = FieldKind::Global;
    fn to_raw(self) -> u32 {
        self.0
    }
    fn from_raw(raw: u32) -> Option<Self> {
        Some(GlobalId(raw))
    }
    fn comment(self, cx: &Cx<'_>, note: &mut Note<'_>) -> fmt::Result {
        let Some(module) = cx.module else {
            return Ok(());
        };
        match module.global(self).and_then(|g| module.string(g.name)) {
            Some(name) => note.item(format_args!("{name:?}")),
            None => note.item(format_args!("{self} <invalid>")),
        }
    }
}

impl Operand for NameRef {
    const KIND: FieldKind = FieldKind::Name;
    fn to_raw(self) -> u32 {
        u32::from(self.0)
    }
    fn from_raw(raw: u32) -> Option<Self> {
        u16::try_from(raw).ok().map(NameRef)
    }
    fn comment(self, cx: &Cx<'_>, note: &mut Note<'_>) -> fmt::Result {
        let (Some(module), Some(func)) = (cx.module, cx.func) else {
            return Ok(());
        };
        match func
            .names()
            .get(self.index())
            .and_then(|&s| module.string(s))
        {
            Some(name) => note.item(format_args!("{name:?}")),
            None => note.item(format_args!("{self} <invalid>")),
        }
    }
}

impl Operand for TypeRef {
    const KIND: FieldKind = FieldKind::TypeRef;
    fn to_raw(self) -> u32 {
        u32::from(self.0)
    }
    fn from_raw(raw: u32) -> Option<Self> {
        u16::try_from(raw).ok().map(TypeRef)
    }
    fn comment(self, cx: &Cx<'_>, note: &mut Note<'_>) -> fmt::Result {
        let Some(func) = cx.func else { return Ok(()) };
        match func.type_refs().get(self.index()) {
            Some(ty) => note.item(format_args!("{ty}")),
            None => note.item(format_args!("{self} <invalid>")),
        }
    }
}

impl Operand for ShapeId {
    const KIND: FieldKind = FieldKind::Shape;
    fn to_raw(self) -> u32 {
        u32::from(self.0)
    }
    fn from_raw(raw: u32) -> Option<Self> {
        u16::try_from(raw).ok().map(ShapeId)
    }
    /// The shape, with names resolved: `(_, "x":, ...)`.
    fn comment(self, cx: &Cx<'_>, note: &mut Note<'_>) -> fmt::Result {
        let Some(func) = cx.func else { return Ok(()) };
        let Some(shape) = func.shapes().get(self.index()) else {
            return note.item(format_args!("{self} <invalid>"));
        };
        let mut text = alloc::string::String::from("(");
        for (i, arg) in shape.args.iter().enumerate() {
            if i > 0 {
                text.push_str(", ");
            }
            match (arg, cx.module) {
                (ArgKind::Named(name), Some(module)) => match module.string(*name) {
                    Some(s) => {
                        write_quoted(&mut text, s)?;
                        text.push(':');
                    }
                    None => fmt::Write::write_fmt(&mut text, format_args!("{name} <invalid>:"))?,
                },
                (other, _) => fmt::Write::write_fmt(&mut text, format_args!("{other}"))?,
            }
        }
        text.push(')');
        note.item(format_args!("{text}"))
    }
}

impl Operand for ErrorKind {
    const KIND: FieldKind = FieldKind::ErrKind;
    fn to_raw(self) -> u32 {
        self.code()
    }
    /// Only catchable kinds: a trap is not an error value, so bytecode
    /// cannot raise `OutOfFuel` or `Unreachable` as one.
    fn from_raw(raw: u32) -> Option<Self> {
        ErrorKind::from_code(raw).filter(|k| k.is_catchable())
    }
    /// `.E0200`: the code alone, since the kind's name has a space before it.
    fn suffix(self, w: &mut dyn fmt::Write) -> fmt::Result {
        write!(w, ".E{:04}", self.code())
    }
    fn comment(self, _cx: &Cx<'_>, note: &mut Note<'_>) -> fmt::Result {
        note.item(format_args!("{}", self.name()))
    }
}

impl Operand for i32 {
    const KIND: FieldKind = FieldKind::Imm32;
    fn to_raw(self) -> u32 {
        u32::from_le_bytes(self.to_le_bytes())
    }
    fn from_raw(raw: u32) -> Option<Self> {
        Some(i32::from_le_bytes(raw.to_le_bytes()))
    }
}

impl Operand for u8 {
    const KIND: FieldKind = FieldKind::Count;
    fn to_raw(self) -> u32 {
        u32::from(self)
    }
    fn from_raw(raw: u32) -> Option<Self> {
        u8::try_from(raw).ok()
    }
}

impl Operand for bool {
    const KIND: FieldKind = FieldKind::Bool;
    fn to_raw(self) -> u32 {
        u32::from(self)
    }
    fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

/// The bits of the word an opcode's fields (and the opcode byte) occupy.
const fn used_bits(fields: &[FieldSpec]) -> u64 {
    let mut used = 0xff;
    let mut i = 0;
    while i < fields.len() {
        let slot = fields[i].slot;
        used |= slot.mask() << slot.shift();
        i += 1;
    }
    used
}

/// Whether `word` sets a bit outside `used`. (A function rather than an
/// inline expression: for opcodes that use every slot the mask is zero,
/// which is correct but reads as a mistake to a linter.)
#[inline(always)]
const fn has_unused_bits(word: u64, used: u64) -> bool {
    word & !used != 0
}

/// Reads one field from a raw word.
#[inline]
fn read_field<T: Operand>(
    word: u64,
    slot: Slot,
    opcode: Opcode,
    field: &'static str,
) -> Result<T, InstError> {
    // The mask keeps the value within the slot's width, which is at most 32
    // bits, so the narrowing is exact.
    let raw = ((word >> slot.shift()) & slot.mask()) as u32;
    T::from_raw(raw).ok_or(InstError::InvalidOperand { opcode, field })
}

macro_rules! isa {
    (
        $(
            $(#[$meta:meta])*
            $code:literal $name:ident $mnemonic:literal {
                $( $(#[$fmeta:meta])* $field:ident : $ty:ident @ $slot:ident ),* $(,)?
            }
        )*
    ) => {
        /// One LSB instruction, decoded.
        ///
        /// Every variant is eight bytes and `Copy`. Field order is display
        /// order; [`Opcode::fields`] gives each field's slot in the encoded
        /// word. The normative semantics of every instruction, every operand
        /// type rule, and every error it can raise are in `specs/LSB.md`;
        /// each variant's documentation summarizes them.
        ///
        /// *Window operands.* Instructions that need more registers than
        /// eight bytes hold read them from consecutive registers: a call's
        /// arguments are `dst+1 ..= dst+argc`, a closure's captures follow
        /// its destination, `str_slice` reads its end index from `range+1`.
        ///
        /// The enum is deliberately exhaustive: every execution tier must
        /// implement every instruction, and a new one is a format change.
        ///
        /// # Examples
        ///
        /// ```
        /// use bytecode_lang::{Inst, IntOp, IntTy, Opcode, Reg};
        ///
        /// let add = Inst::IAdd { op: IntOp::new(IntTy::I64), dst: Reg(2), lhs: Reg(0), rhs: Reg(1) };
        /// assert_eq!(add.opcode(), Opcode::IAdd);
        /// assert_eq!(add.to_string(), "iadd.i64 r2, r0, r1");
        /// assert_eq!(Inst::from_bytes(add.to_bytes()), Ok(add));
        /// assert_eq!(core::mem::size_of::<Inst>(), 8);
        /// ```
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum Inst {
            $(
                $(#[$meta])*
                $name { $( $(#[$fmeta])* $field: $ty ),* },
            )*
        }

        /// The operation an [`Inst`] performs: its first encoded byte.
        ///
        /// # Examples
        ///
        /// ```
        /// use bytecode_lang::Opcode;
        ///
        /// assert_eq!(Opcode::from_u8(0x01), Some(Opcode::Mov));
        /// assert_eq!(Opcode::Mov.mnemonic(), "mov");
        /// assert_eq!(Opcode::Mov as u8, 0x01);
        /// assert!(Opcode::ALL.len() > 150);
        /// ```
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        #[repr(u8)]
        pub enum Opcode {
            $(
                $(#[$meta])*
                $name = $code,
            )*
        }

        impl Opcode {
            /// Every opcode, in table order.
            pub const ALL: &'static [Opcode] = &[ $( Opcode::$name ),* ];

            /// The opcode with this byte, or `None`.
            ///
            /// # Examples
            ///
            /// ```
            /// use bytecode_lang::Opcode;
            ///
            /// assert_eq!(Opcode::from_u8(0xa0), Some(Opcode::Jmp));
            /// assert_eq!(Opcode::from_u8(0xff), None);
            /// ```
            #[must_use]
            pub const fn from_u8(byte: u8) -> Option<Opcode> {
                match byte {
                    $( $code => Some(Opcode::$name), )*
                    _ => None,
                }
            }

            /// The assembler name.
            ///
            /// # Examples
            ///
            /// ```
            /// use bytecode_lang::Opcode;
            ///
            /// assert_eq!(Opcode::TailCall.mnemonic(), "tail_call");
            /// ```
            #[must_use]
            pub const fn mnemonic(self) -> &'static str {
                match self {
                    $( Opcode::$name => $mnemonic, )*
                }
            }

            /// The opcode's operands, in display order, with their slots.
            ///
            /// # Examples
            ///
            /// ```
            /// use bytecode_lang::{FieldKind, Opcode, Slot};
            ///
            /// let f = Opcode::Jmp.fields();
            /// assert_eq!(f.len(), 1);
            /// assert_eq!((f[0].kind, f[0].slot), (FieldKind::Target, Slot::W));
            /// ```
            #[must_use]
            pub const fn fields(self) -> &'static [FieldSpec] {
                match self {
                    $(
                        Opcode::$name => {
                            const FIELDS: &[FieldSpec] = &[
                                $( FieldSpec {
                                    name: stringify!($field),
                                    kind: <$ty as Operand>::KIND,
                                    slot: Slot::$slot,
                                } ),*
                            ];
                            FIELDS
                        }
                    )*
                }
            }
        }

        impl Inst {
            /// The instruction's opcode.
            ///
            /// # Examples
            ///
            /// ```
            /// use bytecode_lang::{Inst, Opcode};
            ///
            /// assert_eq!(Inst::RetVoid {}.opcode(), Opcode::RetVoid);
            /// ```
            #[must_use]
            pub const fn opcode(&self) -> Opcode {
                match self {
                    $( Inst::$name { .. } => Opcode::$name, )*
                }
            }

            /// The canonical eight-byte encoding.
            ///
            /// # Examples
            ///
            /// ```
            /// use bytecode_lang::{Inst, Reg};
            ///
            /// let mov = Inst::Mov { dst: Reg(1), src: Reg(2) };
            /// assert_eq!(mov.to_bytes(), [0x01, 0, 1, 0, 2, 0, 0, 0]);
            /// ```
            #[must_use]
            #[inline]
            pub fn to_bytes(&self) -> [u8; 8] {
                let word: u64 = match *self {
                    $(
                        Inst::$name { $( $field ),* } => {
                            #[allow(unused_mut)]
                            let mut word = u64::from(Opcode::$name as u8);
                            $( word |= u64::from(Operand::to_raw($field)) << Slot::$slot.shift(); )*
                            word
                        }
                    )*
                };
                word.to_le_bytes()
            }

            /// Decodes eight bytes. Fails on an unknown opcode, on a modifier
            /// value its type does not define, and on non-zero bytes in
            /// unused slots, so every instruction has exactly one encoding.
            ///
            /// # Errors
            ///
            /// [`InstError`] says which of the three applies.
            ///
            /// # Examples
            ///
            /// ```
            /// use bytecode_lang::{Inst, InstError, Opcode};
            ///
            /// assert_eq!(Inst::from_bytes([0, 0, 0, 0, 0, 0, 0, 0]), Ok(Inst::Nop {}));
            /// // `load_bool` with 2 in its boolean slot.
            /// assert_eq!(
            ///     Inst::from_bytes([0x06, 2, 0, 0, 0, 0, 0, 0]),
            ///     Err(InstError::InvalidOperand { opcode: Opcode::LoadBool, field: "val" }),
            /// );
            /// ```
            #[inline]
            pub fn from_bytes(bytes: [u8; 8]) -> Result<Inst, InstError> {
                let word = u64::from_le_bytes(bytes);
                let [opcode, ..] = bytes;
                Ok(match opcode {
                    $(
                        $code => {
                            // Bits outside the opcode's slots must be zero;
                            // with every field strict (`from_raw` accepts
                            // only values it re-encodes identically), that
                            // makes the encoding unique.
                            const USED: u64 = used_bits(Opcode::$name.fields());
                            if has_unused_bits(word, USED) {
                                return Err(InstError::NonCanonical(Opcode::$name));
                            }
                            Inst::$name {
                                $( $field: read_field::<$ty>(
                                    word,
                                    Slot::$slot,
                                    Opcode::$name,
                                    stringify!($field),
                                )? ),*
                            }
                        }
                    )*
                    other => return Err(InstError::UnknownOpcode(other)),
                })
            }

            /// The overflow policy the instruction carries (in an
            /// [`IntOp`], [`Policy`], or [`IntConv`] modifier), or `None` for
            /// instructions that carry none.
            ///
            /// # Examples
            ///
            /// ```
            /// use bytecode_lang::{Inst, Overflow, Policy, Reg};
            ///
            /// let p = Policy::new().with_overflow(Overflow::Promote);
            /// let add = Inst::DAdd { dst: Reg(0), lhs: Reg(1), rhs: Reg(2), pol: p };
            /// assert_eq!(add.overflow(), Some(Overflow::Promote));
            /// assert_eq!(Inst::Nop {}.overflow(), None);
            /// ```
            #[must_use]
            #[allow(unused_mut, unused_variables, clippy::let_and_return)]
            pub fn overflow(&self) -> Option<Overflow> {
                match *self {
                    $(
                        Inst::$name { $( $field ),* } => {
                            let mut found = None;
                            $(
                                if let Some(o) = Operand::overflow($field) {
                                    found = Some(o);
                                }
                            )*
                            found
                        }
                    )*
                }
            }

            /// What the builder checks of every instruction, in one
            /// dispatch: its branch target (if it is a branch) and whether
            /// it carries `overflow = promote`. Every arm but the branches
            /// and the policy-carrying instructions folds to a constant.
            #[inline]
            #[allow(unused_variables)]
            pub(crate) fn build_checks(&self) -> (Option<Target>, bool) {
                match *self {
                    $(
                        Inst::$name { $( $field ),* } => {
                            let target: Option<Target> = None $( .or(Operand::as_target($field)) )*;
                            (target, false $( || Operand::is_promote($field) )*)
                        }
                    )*
                }
            }

            /// The register in the field named `dst`, if the instruction has one.
            #[allow(unused_mut, unused_variables, clippy::let_and_return)]
            pub(crate) fn dst(&self) -> Option<Reg> {
                match *self {
                    $(
                        Inst::$name { $( $field ),* } => {
                            let mut dst = None;
                            $(
                                if is_dst(stringify!($field)) {
                                    dst = Operand::as_reg($field);
                                }
                            )*
                            dst
                        }
                    )*
                }
            }

            /// Renders `mnemonic.modifiers op, op ...  ; comments`.
            #[allow(unused_assignments, unused_mut, unused_variables)]
            pub(crate) fn render(&self, w: &mut dyn fmt::Write, cx: &Cx<'_>) -> fmt::Result {
                match *self {
                    $(
                        Inst::$name { $( $field ),* } => {
                            w.write_str($mnemonic)?;
                            $( Operand::suffix($field, w)?; )*
                            let mut sep = " ";
                            $(
                                if !<$ty as Operand>::KIND.is_modifier() {
                                    w.write_str(sep)?;
                                    sep = ", ";
                                    Operand::render($field, w, cx)?;
                                }
                            )*
                            let mut note = Note { w, started: false };
                            $( Operand::comment($field, cx, &mut note)?; )*
                            Ok(())
                        }
                    )*
                }
            }
        }
    };
}

isa! {
    // ---------------------------------------------------------------------
    // Moves, constants, globals
    // ---------------------------------------------------------------------

    /// Does nothing.
    0x00 Nop "nop" {}

    /// `dst = src`. Both registers have the same declared type; the 64-bit
    /// slot is copied as is.
    0x01 Mov "mov" {
        /// Destination register.
        dst: Reg @ B,
        /// Source register.
        src: Reg @ C,
    }

    /// `dst = constants[k]` in the destination's typed representation (an
    /// `int` constant into an integer register it fits, a `str` or `bytes`
    /// constant into a `str` register, an aggregate into a matching `ref`).
    0x02 LoadConst "load_const" {
        /// Destination register.
        dst: Reg @ B,
        /// The constant.
        k: ConstId @ W,
    }

    /// `dst = constants[k]` boxed as a `dyn` value (integers become dynamic
    /// ints and must fit `i64`; floats become dynamic floats; aggregates
    /// become dynamic arrays and maps).
    0x03 DLoadConst "dload_const" {
        /// Destination `dyn` register.
        dst: Reg @ B,
        /// The constant.
        k: ConstId @ W,
    }

    /// `dst = val` as an integer of type `ty`; the immediate is
    /// sign-extended and must be representable in `ty`.
    0x04 LoadInt "load_int" {
        /// Destination integer register.
        dst: Reg @ B,
        /// The immediate.
        val: i32 @ W,
        /// The destination's integer type.
        ty: IntTy @ A,
    }

    /// `dst = val` as a dynamic integer.
    0x05 DLoadInt "dload_int" {
        /// Destination `dyn` register.
        dst: Reg @ B,
        /// The immediate.
        val: i32 @ W,
    }

    /// `dst = val`.
    0x06 LoadBool "load_bool" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// The immediate.
        val: bool @ A,
    }

    /// `dst = nil`: the dynamic nil, or the null `str`/`ref`.
    0x07 LoadNil "load_nil" {
        /// Destination `dyn`, `str`, or `ref` register.
        dst: Reg @ B,
    }

    /// `dst =` a callable reference to a host function: a first-class value
    /// every dynamic call form accepts, bound by the import's parameter
    /// list if it has one.
    0x08 LoadImport "load_import" {
        /// Destination `ref` (function type) or `dyn` register.
        dst: Reg @ B,
        /// The import.
        import: ImportId @ W,
    }

    /// `dst = globals[global]`.
    0x09 GetGlobal "get_global" {
        /// Destination register of the global's type.
        dst: Reg @ B,
        /// The global.
        global: GlobalId @ W,
    }

    /// `globals[global] = src`; the global must be mutable.
    0x0A SetGlobal "set_global" {
        /// The global.
        global: GlobalId @ W,
        /// Source register of the global's type.
        src: Reg @ B,
    }

    // ---------------------------------------------------------------------
    // Integer arithmetic (OPS §3). Every one carries an IntOp: the operand
    // type and the complete policy set.
    // ---------------------------------------------------------------------

    /// `dst = lhs + rhs`; overflow per `op.policy().overflow()`.
    0x10 IAdd "iadd" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs - rhs`; overflow per policy.
    0x11 ISub "isub" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs * rhs`; overflow per policy.
    0x12 IMul "imul" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs / rhs`, truncating toward zero; `rhs == 0` per `div_zero`,
    /// signed `MIN / -1` per `overflow` (`wrap` gives `MIN`).
    0x13 IDiv "idiv" {
        /// Destination register.
        dst: Reg @ B,
        /// Dividend.
        lhs: Reg @ C,
        /// Divisor.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs rem rhs`, the sign following the dividend; `rhs == 0` per
    /// `div_zero`; signed `MIN rem -1` is `0`, never an error.
    0x14 IRem "irem" {
        /// Destination register.
        dst: Reg @ B,
        /// Dividend.
        lhs: Reg @ C,
        /// Divisor.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = floor(lhs / rhs)` (Python `//`); zero and overflow rules as
    /// `idiv`.
    0x15 IFloorDiv "ifloor_div" {
        /// Destination register.
        dst: Reg @ B,
        /// Dividend.
        lhs: Reg @ C,
        /// Divisor.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs - rhs * floor_div(lhs, rhs)`, the sign following the
    /// divisor; `rhs == 0` per `div_zero`; signed `MIN floor_mod -1` is `0`.
    0x16 IFloorMod "ifloor_mod" {
        /// Destination register.
        dst: Reg @ B,
        /// Dividend.
        lhs: Reg @ C,
        /// Divisor.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs & rhs`.
    0x17 IAnd "iand" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs | rhs`.
    0x18 IOr "ior" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs ^ rhs`.
    0x19 IXor "ixor" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs << rhs`; bits shifted out are discarded; an amount at least
    /// the width (or negative, for a signed type) per `shift`.
    0x1A IShl "ishl" {
        /// Destination register.
        dst: Reg @ B,
        /// Value to shift.
        lhs: Reg @ C,
        /// Shift amount, of the same type.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs >> rhs`, arithmetic for signed types and logical for
    /// unsigned; amount rule as `ishl`.
    0x1B IShr "ishr" {
        /// Destination register.
        dst: Reg @ B,
        /// Value to shift.
        lhs: Reg @ C,
        /// Shift amount, of the same type.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = min(lhs, rhs)`.
    0x1C IMin "imin" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = max(lhs, rhs)`.
    0x1D IMax "imax" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = 0 - src`; `neg(MIN)` per `overflow`.
    0x1E INeg "ineg" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = !src` (bitwise complement; HIR's `bit_not` on integers).
    0x1F IBitNot "ibit_not" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = |src|`; `abs(MIN)` per `overflow`.
    0x20 IAbs "iabs" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs ** rhs` (OPS v2 `pow`): the exact power by repeated
    /// squaring, `0 ** 0 = 1`; a result that does not fit per `overflow`
    /// (`wrap` keeps the low bits); a negative exponent of a signed type
    /// raises `NegativeExponent` (E0006) under every policy (`promote` is
    /// never valid here). The exponent has the operand type.
    0x27 IPow "ipow" {
        /// Destination register.
        dst: Reg @ B,
        /// Base.
        lhs: Reg @ C,
        /// Exponent.
        rhs: Reg @ D,
        /// Operand type and policy.
        op: IntOp @ A,
    }

    /// `dst = lhs == rhs` (bool).
    0x21 IEq "ieq" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type (signedness decides ordering).
        ty: IntTy @ A,
    }

    /// `dst = lhs != rhs` (bool).
    0x22 INe "ine" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: IntTy @ A,
    }

    /// `dst = lhs < rhs` (bool), signed or unsigned per `ty`.
    0x23 ILt "ilt" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: IntTy @ A,
    }

    /// `dst = lhs <= rhs` (bool).
    0x24 ILe "ile" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: IntTy @ A,
    }

    /// `dst = lhs > rhs` (bool).
    0x25 IGt "igt" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: IntTy @ A,
    }

    /// `dst = lhs >= rhs` (bool).
    0x26 IGe "ige" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: IntTy @ A,
    }

    // ---------------------------------------------------------------------
    // Float arithmetic (OPS §4). Never an error: IEEE results throughout.
    // ---------------------------------------------------------------------

    /// `dst = lhs + rhs` (round to nearest, ties to even).
    0x30 FAdd "fadd" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = lhs - rhs`.
    0x31 FSub "fsub" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = lhs * rhs`.
    0x32 FMul "fmul" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = lhs / rhs`; division by zero gives ±inf or NaN.
    0x33 FDiv "fdiv" {
        /// Destination register.
        dst: Reg @ B,
        /// Dividend.
        lhs: Reg @ C,
        /// Divisor.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = fmod(lhs, rhs)`: the truncated remainder, sign of the dividend.
    0x34 FRem "frem" {
        /// Destination register.
        dst: Reg @ B,
        /// Dividend.
        lhs: Reg @ C,
        /// Divisor.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = remainder(lhs, rhs)`: IEEE 754 remainder (nearest quotient).
    0x35 FIeeeRem "fieee_rem" {
        /// Destination register.
        dst: Reg @ B,
        /// Dividend.
        lhs: Reg @ C,
        /// Divisor.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = min(lhs, rhs)`; NaN if either is NaN; `min(-0, +0) = -0`.
    0x36 FMin "fmin" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = max(lhs, rhs)`; NaN if either is NaN; `max(-0, +0) = +0`.
    0x37 FMax "fmax" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = fma(lhs, rhs, dst)`: fused multiply-add, correctly rounded.
    /// `dst` is both the addend and the result.
    0x38 FFma "ffma" {
        /// Addend in, result out.
        dst: Reg @ B,
        /// First factor.
        lhs: Reg @ C,
        /// Second factor.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = -src` (sign flip; payload preserved).
    0x39 FNeg "fneg" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = |src|` (sign clear; payload preserved).
    0x3A FAbs "fabs" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = sqrt(src)`, correctly rounded.
    0x3B FSqrt "fsqrt" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = floor(src)`.
    0x3C FFloor "ffloor" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = ceil(src)`.
    0x3D FCeil "fceil" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = trunc(src)` (toward zero).
    0x3E FTrunc "ftrunc" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = round(src)`, ties away from zero.
    0x3F FRound "fround" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = round(src)`, ties to even.
    0x40 FRoundEven "fround_even" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = lhs == rhs` (bool); false if either is NaN.
    0x41 FEq "feq" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = lhs != rhs` (bool); true if either is NaN.
    0x42 FNe "fne" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = lhs < rhs` (bool); false if either is NaN.
    0x43 FLt "flt" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = lhs <= rhs` (bool); false if either is NaN.
    0x44 FLe "fle" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = lhs > rhs` (bool); false if either is NaN.
    0x45 FGt "fgt" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = lhs >= rhs` (bool); false if either is NaN.
    0x46 FGe "fge" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = pow(lhs, rhs)` (OPS v2 float `pow`): C99 Annex F special
    /// cases, finite results by the family's shared `ls_pow` routine so every
    /// tier is bit-identical; `f32` computes `ls_pow` on the operands widened
    /// to `f64` and rounds once. Never an error.
    0x48 FPow "fpow" {
        /// Destination register.
        dst: Reg @ B,
        /// Base.
        lhs: Reg @ C,
        /// Exponent.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    /// `dst = totalOrder(lhs, rhs)` as an `i8` of -1, 0, or 1 (IEEE 754
    /// total order: `-NaN < -inf < ... < -0 < +0 < ... < +inf < +NaN`).
    0x47 FTotalCmp "ftotal_cmp" {
        /// Destination `i8` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Operand type.
        ty: FloatTy @ A,
    }

    // ---------------------------------------------------------------------
    // Booleans, chars, references
    // ---------------------------------------------------------------------

    /// `dst = !src` (logical).
    0x50 BNot "bnot" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
    }

    /// `dst = lhs & rhs` (both evaluated; short-circuit is control flow).
    0x51 BAnd "band" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst = lhs | rhs`.
    0x52 BOr "bor" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst = lhs ^ rhs` (also boolean inequality).
    0x53 BXor "bxor" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst = lhs == rhs` on `char` scalar values.
    0x54 CEq "ceq" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst = lhs != rhs` on `char` scalar values.
    0x55 CNe "cne" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst = lhs < rhs` on `char` scalar values.
    0x56 CLt "clt" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst = lhs <= rhs` on `char` scalar values.
    0x57 CLe "cle" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst = lhs > rhs` on `char` scalar values.
    0x58 CGt "cgt" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst = lhs >= rhs` on `char` scalar values.
    0x59 CGe "cge" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst = lhs` and `rhs` are the same object (or both `nil`). Operands
    /// are `str` or `ref`; string contents are not compared.
    0x5A RefEq "ref_eq" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    // ---------------------------------------------------------------------
    // Conversions (OPS §5)
    // ---------------------------------------------------------------------

    /// `dst = src` converted between integer types: exact if it fits, else
    /// per the conversion's `overflow` (`wrap` truncates or sign-extends).
    0x60 IntCast "int_cast" {
        /// Destination register of type `conv.to()`.
        dst: Reg @ B,
        /// Source register of type `conv.from()`.
        src: Reg @ C,
        /// Source type, destination type, overflow policy.
        conv: IntConv @ A,
    }

    /// Zero-extends a narrower integer to a wider one; never an error.
    0x61 Zext "zext" {
        /// Destination register.
        dst: Reg @ B,
        /// Source register.
        src: Reg @ C,
        /// Source and destination types (destination at least as wide).
        pair: IntPair @ A,
    }

    /// Sign-extends a narrower integer to a wider one; never an error.
    0x62 Sext "sext" {
        /// Destination register.
        dst: Reg @ B,
        /// Source register.
        src: Reg @ C,
        /// Source and destination types (destination at least as wide).
        pair: IntPair @ A,
    }

    /// Keeps the low bits of a wider integer; never an error.
    0x63 Trunc "trunc" {
        /// Destination register.
        dst: Reg @ B,
        /// Source register.
        src: Reg @ C,
        /// Source and destination types (destination at most as wide).
        pair: IntPair @ A,
    }

    /// `dst = src` as `f32`, rounded to nearest, ties to even.
    0x64 IntToF32 "int_to_f32" {
        /// Destination `f32` register.
        dst: Reg @ B,
        /// Source integer register.
        src: Reg @ C,
        /// The source's integer type.
        ty: IntTy @ A,
    }

    /// `dst = src` as `f64`, rounded to nearest, ties to even.
    0x65 IntToF64 "int_to_f64" {
        /// Destination `f64` register.
        dst: Reg @ B,
        /// Source integer register.
        src: Reg @ C,
        /// The source's integer type.
        ty: IntTy @ A,
    }

    /// `dst = src` truncated toward zero to the integer type `conv.ty()`;
    /// NaN or out of range per `conv.float_to_int()`.
    0x66 F32ToInt "f32_to_int" {
        /// Destination integer register.
        dst: Reg @ B,
        /// Source `f32` register.
        src: Reg @ C,
        /// Destination type and policy.
        conv: FloatConv @ A,
    }

    /// `dst = src` truncated toward zero to the integer type `conv.ty()`;
    /// NaN or out of range per `conv.float_to_int()`.
    0x67 F64ToInt "f64_to_int" {
        /// Destination integer register.
        dst: Reg @ B,
        /// Source `f64` register.
        src: Reg @ C,
        /// Destination type and policy.
        conv: FloatConv @ A,
    }

    /// `dst = src` widened to `f64` (exact).
    0x68 F32ToF64 "f32_to_f64" {
        /// Destination `f64` register.
        dst: Reg @ B,
        /// Source `f32` register.
        src: Reg @ C,
    }

    /// `dst = src` narrowed to `f32`, rounded to nearest, ties to even.
    0x69 F64ToF32 "f64_to_f32" {
        /// Destination `f32` register.
        dst: Reg @ B,
        /// Source `f64` register.
        src: Reg @ C,
    }

    /// `dst` = the IEEE bits of `src` as an integer of the same width
    /// (`ty` is a 32- or 64-bit integer type).
    0x6A FloatToBits "float_to_bits" {
        /// Destination integer register.
        dst: Reg @ B,
        /// Source float register.
        src: Reg @ C,
        /// The destination's integer type.
        ty: IntTy @ A,
    }

    /// `dst` = the float whose IEEE bits are `src` (`ty` is a 32- or 64-bit
    /// integer type; the float has the same width).
    0x6B BitsToFloat "bits_to_float" {
        /// Destination float register.
        dst: Reg @ B,
        /// Source integer register.
        src: Reg @ C,
        /// The source's integer type.
        ty: IntTy @ A,
    }

    /// `dst = char(src)`; `InvalidChar` (E0005) if `src` (a `u32`) is not a
    /// Unicode scalar value.
    0x6C CharFromU32 "char_from_u32" {
        /// Destination `char` register.
        dst: Reg @ B,
        /// Source `u32` register.
        src: Reg @ C,
    }

    /// `dst = src` as its `u32` scalar value. Total; an LSB addition to OPS.
    0x6D CharToU32 "char_to_u32" {
        /// Destination `u32` register.
        dst: Reg @ B,
        /// Source `char` register.
        src: Reg @ C,
    }

    /// `dst = 1` if `src` is true, else `0`.
    0x6E BoolToInt "bool_to_int" {
        /// Destination integer register.
        dst: Reg @ B,
        /// Source `bool` register.
        src: Reg @ C,
        /// The destination's integer type.
        ty: IntTy @ A,
    }

    // ---------------------------------------------------------------------
    // Dynamic values. Operands and results are `dyn`. Numbers take a fast
    // path per OPS (int is i64, mixed int/float computes in f64); every
    // other operand combination calls the module's hook or raises TypeError.
    // ---------------------------------------------------------------------

    /// Dynamic `+`.
    0x70 DAdd "dadd" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic `-`.
    0x71 DSub "dsub" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic `*`.
    0x72 DMul "dmul" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic division: truncating for two ints, IEEE otherwise.
    0x73 DDiv "ddiv" {
        /// Destination register.
        dst: Reg @ B,
        /// Dividend.
        lhs: Reg @ C,
        /// Divisor.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic remainder: `irem` for two ints, `frem` otherwise.
    0x74 DRem "drem" {
        /// Destination register.
        dst: Reg @ B,
        /// Dividend.
        lhs: Reg @ C,
        /// Divisor.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic floor division (Python `//`).
    0x75 DFloorDiv "dfloor_div" {
        /// Destination register.
        dst: Reg @ B,
        /// Dividend.
        lhs: Reg @ C,
        /// Divisor.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic floor modulo (Python `%`).
    0x76 DFloorMod "dfloor_mod" {
        /// Destination register.
        dst: Reg @ B,
        /// Dividend.
        lhs: Reg @ C,
        /// Divisor.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic bitwise and (ints only, else hook).
    0x77 DAnd "dand" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic bitwise or.
    0x78 DOr "dor" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic bitwise xor.
    0x79 DXor "dxor" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic shift left (64-bit; amount rule per `shift`, `saturate` for
    /// PHP).
    0x7A DShl "dshl" {
        /// Destination register.
        dst: Reg @ B,
        /// Value to shift.
        lhs: Reg @ C,
        /// Shift amount.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic arithmetic shift right.
    0x7B DShr "dshr" {
        /// Destination register.
        dst: Reg @ B,
        /// Value to shift.
        lhs: Reg @ C,
        /// Shift amount.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic negation.
    0x7C DNeg "dneg" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic bitwise complement (PHP/Python `~`; HIR's `bit_not`): ints
    /// only, else the `bit_not` hook.
    0x7D DBitNot "dbit_not" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic equality into a `bool` register (numbers exactly, strings by
    /// content, other kinds through the `eq` hook or by identity).
    0x7E DEq "deq" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// Dynamic inequality: the negation of `deq`.
    0x7F DNe "dne" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// Dynamic `<` into a `bool` register (numbers, strings bytewise, chars;
    /// else the `lt` hook or TypeError).
    0x80 DLt "dlt" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// Dynamic `<=`.
    0x81 DLe "dle" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// Dynamic `>` (`lt` with the operands swapped).
    0x82 DGt "dgt" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// Dynamic `>=` (`le` with the operands swapped).
    0x83 DGe "dge" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst` = the truthiness of `src` as a `bool`: nil is false, bools are
    /// themselves, numbers are true unless zero; other kinds go to the
    /// `truthy` hook, or are true unless an empty string, array, or map.
    0x84 DTruthy "dtruthy" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
    }

    /// Dynamic string concatenation (two strings, else the `concat` hook).
    0x85 DConcat "dconcat" {
        /// Destination register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst = src` boxed into a `dyn` (`from` names `src`'s static type;
    /// `u64` above `i64::MAX` raises ArithOverflow; `str`/`ref` are a no-op).
    0x86 ToDyn "to_dyn" {
        /// Destination `dyn` register.
        dst: Reg @ B,
        /// Source register.
        src: Reg @ C,
        /// The source's static type.
        from: Prim @ A,
    }

    /// `dst = src` unboxed to the static type `to`; TypeError if the kind
    /// does not match, ArithOverflow if an int does not fit. (`ref` targets
    /// use `cast`.)
    0x87 FromDyn "from_dyn" {
        /// Destination register.
        dst: Reg @ B,
        /// Source `dyn` register.
        src: Reg @ C,
        /// The destination's static type.
        to: Prim @ A,
    }

    /// `dst` = the [`Kind`] code of `src`, as a `u8`.
    0x88 TypeOf "type_of" {
        /// Destination `u8` register.
        dst: Reg @ B,
        /// Source `dyn`, `str`, or `ref` register.
        src: Reg @ C,
    }

    /// `dst` = whether `src` is of kind `kind`.
    0x89 IsKind "is_kind" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Source `dyn`, `str`, or `ref` register.
        src: Reg @ C,
        /// The kind to test.
        kind: Kind @ A,
    }

    /// `dst` = whether `src` is a non-nil instance of struct type `ty` or of
    /// a type that has it as an ancestor.
    0x8A IsType "is_type" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Source `dyn` or `ref` register.
        src: Reg @ C,
        /// The struct type.
        ty: TypeRef @ D,
    }

    /// `dst = src` as `ref ty`; TypeError if `src` is not nil and not an
    /// instance of `ty` (or a descendant).
    0x8B Cast "cast" {
        /// Destination `ref ty` register.
        dst: Reg @ B,
        /// Source `dyn` or `ref` register.
        src: Reg @ C,
        /// The target type.
        ty: TypeRef @ D,
    }

    /// Dynamic indexing `dst = obj[key]`: arrays by int, maps by key, strings
    /// by int (the byte); misses and other kinds go to the `get_index` hook.
    /// A reference slot reads as its reference's value.
    0x8C DGetIndex "dget_index" {
        /// Destination `dyn` register.
        dst: Reg @ B,
        /// The indexed value.
        obj: Reg @ C,
        /// The key.
        key: Reg @ D,
    }

    /// Dynamic indexed store `obj[key] = src`; a reference slot is written
    /// through, and a reference `src` stores its value.
    0x8D DSetIndex "dset_index" {
        /// The indexed value.
        obj: Reg @ B,
        /// The key.
        key: Reg @ C,
        /// The value to store.
        src: Reg @ D,
    }

    /// Dynamic property read `dst = obj.name`: a struct field or method by
    /// name, a map entry with a string key, else the `get_prop` hook.
    0x8E GetProp "get_prop" {
        /// Destination `dyn` register.
        dst: Reg @ B,
        /// The object.
        obj: Reg @ C,
        /// The property name.
        name: NameRef @ D,
    }

    /// Dynamic property write `obj.name = src`.
    0x8F SetProp "set_prop" {
        /// The object.
        obj: Reg @ B,
        /// The property name.
        name: NameRef @ D,
        /// The value to store.
        src: Reg @ C,
    }

    /// `dst` = whether `obj` has property `name`.
    0x90 HasProp "has_prop" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// The object.
        obj: Reg @ C,
        /// The property name.
        name: NameRef @ D,
    }

    /// Dynamic call: `dst = callee(dst+1, ..., dst+argc)` with positional
    /// `dyn` arguments, bound to the callee's parameter list (exactly
    /// `dcall_shape` with an all-positional shape): ArgumentError (E0114)
    /// when they do not bind, TypeError when one does not convert to its
    /// parameter's type; non-callables go to the `call` hook.
    0x91 DCall "dcall" {
        /// Result register (`dyn`); arguments follow it.
        dst: Reg @ B,
        /// The callee.
        callee: Reg @ C,
        /// The number of arguments.
        argc: u8 @ A,
    }

    /// `dst` = a dynamic iterator over `src` (array or map; else the `iter`
    /// hook).
    0x92 DIterNew "diter_new" {
        /// Destination `dyn` register.
        dst: Reg @ B,
        /// The value to iterate.
        src: Reg @ C,
    }

    /// `dst` = the length of `src` as an `i64` (string bytes, array or map
    /// entries; else the `len` hook).
    0x93 DLen "dlen" {
        /// Destination `i64` register.
        dst: Reg @ B,
        /// The value.
        src: Reg @ C,
    }

    /// Dynamic logical not (PHP `!`, Python `not`; HIR's `not`): `dst =
    /// !truthy(src)` as a `bool`, with exactly the truthiness rules of
    /// `dtruthy` (the `truthy` hook included). `dbit_not` is the bitwise `~`.
    0x94 DNot "dnot" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
    }

    /// Dynamic `**` (OPS v2 `pow`): two ints → integer `pow` at `i64` under
    /// `pol` (`promote` gives the nearest `f64` of the exact power, and the
    /// `f64` power for a negative exponent; other policies raise
    /// `NegativeExponent` for one); numbers with a float → `fpow` in `f64`;
    /// else the `pow` hook, else TypeError.
    0x95 DPow "dpow" {
        /// Destination register.
        dst: Reg @ B,
        /// Base.
        lhs: Reg @ C,
        /// Exponent.
        rhs: Reg @ D,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Dynamic absolute value: an int → `abs` at `i64` (`abs(MIN)` per
    /// `overflow`; `promote` gives `9.223372036854775808e18`); a float →
    /// `fabs`; else the `abs` hook, else TypeError.
    0x96 DAbs "dabs" {
        /// Destination register.
        dst: Reg @ B,
        /// Operand.
        src: Reg @ C,
        /// Policy for the integer path.
        pol: Policy @ A,
    }

    /// Separate an element for a nested write (PHP `$a[k][] = v`): `obj`
    /// (an array or map) is written, so its own copy-on-write store is made
    /// unique first; then if `obj[key]` is an array or map that another
    /// container may share, it is replaced by a copy (O(1), copy-on-write);
    /// `dst` = the element, safe to write in place. A reference slot
    /// separates the container inside the reference. An absent key gives
    /// nil (nothing is stored); other kinds of `obj` behave exactly as
    /// `dget_index`.
    0x97 DSepIndex "dsep_index" {
        /// Destination `dyn` register.
        dst: Reg @ B,
        /// The container.
        obj: Reg @ C,
        /// The key.
        key: Reg @ D,
    }

    /// `dsep_index` for a property (PHP `$o->items[] = v`): a struct field
    /// of type `dyn` or a map entry with that string key; an absent map
    /// entry gives nil; other cases behave exactly as `get_prop`.
    0x98 DSepProp "dsep_prop" {
        /// Destination `dyn` register.
        dst: Reg @ B,
        /// The object.
        obj: Reg @ C,
        /// The property name.
        name: NameRef @ D,
    }

    /// Dynamic call with a call shape: `dst = callee(...)` with the window
    /// `dst+1 ..= dst+n` laid out by `shapes[shape]` (positional, named,
    /// spread, named spread), bound to the callee's parameter list
    /// (`ArgumentError` E0114 when they do not bind); non-callables go to
    /// the `call_shape` hook, or to `call` when no argument is named.
    0x99 DCallShape "dcall_shape" {
        /// Result register (`dyn`); arguments follow it.
        dst: Reg @ B,
        /// The callee.
        callee: Reg @ C,
        /// The call shape.
        shape: ShapeId @ D,
    }

    /// `dst` = whether a positional argument at position `pos` (an `i64`)
    /// would be taken by reference by `callee`, so a code generator can
    /// send a reference or a value (PHP decides per argument at run time).
    /// False for non-callables, callees without a parameter list, and
    /// negative positions. Never raises.
    0x9A DParamRef "dparam_ref" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// The callee.
        callee: Reg @ C,
        /// The 0-based position (`i64`).
        pos: Reg @ D,
    }

    /// `dst` = whether a named argument `name` would be taken by reference
    /// by `callee`. Never raises.
    0x9B DParamRefNamed "dparam_ref_named" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// The callee.
        callee: Reg @ C,
        /// The argument's name.
        name: NameRef @ D,
    }

    /// `dst` = the payload of a runtime error value (what `raise` gave it;
    /// nil for errors raised by instructions), or nil for any other value.
    0x9C ErrPayload "err_payload" {
        /// Destination `dyn` register.
        dst: Reg @ B,
        /// The caught value.
        src: Reg @ C,
    }

    // ---------------------------------------------------------------------
    // Control flow, calls, exceptions
    // ---------------------------------------------------------------------

    /// Continue at `target`.
    0xA0 Jmp "jmp" {
        /// The next instruction.
        target: Target @ W,
    }

    /// Continue at `target` if `cond` is true, else fall through.
    0xA1 JmpIf "jmp_if" {
        /// A `bool` register.
        cond: Reg @ B,
        /// Taken when `cond` is true.
        target: Target @ W,
    }

    /// Continue at `target` if `cond` is false, else fall through.
    0xA2 JmpIfNot "jmp_if_not" {
        /// A `bool` register.
        cond: Reg @ B,
        /// Taken when `cond` is false.
        target: Target @ W,
    }

    /// Continue at `tables[table].targets[src]` if `0 <= src < len`, else at
    /// the table's default.
    0xA3 Switch "switch" {
        /// The selector.
        src: Reg @ B,
        /// The jump table.
        table: TableId @ W,
        /// The selector's integer type.
        ty: IntTy @ A,
    }

    /// `dst = func(dst+1, ..., dst+argc)`; a call to a function with no
    /// result leaves `dst` untouched.
    0xA4 Call "call" {
        /// Result register; arguments follow it.
        dst: Reg @ B,
        /// The callee.
        func: FuncId @ W,
        /// The number of arguments.
        argc: u8 @ A,
    }

    /// `dst = callee(dst+1, ..., dst+argc)` through a typed function
    /// reference (`ref` of a function type).
    0xA5 CallIndirect "call_indirect" {
        /// Result register; arguments follow it.
        dst: Reg @ B,
        /// The function reference.
        callee: Reg @ C,
        /// The number of arguments.
        argc: u8 @ A,
    }

    /// `dst = import(dst+1, ..., dst+argc)`: a host call.
    0xA6 CallImport "call_import" {
        /// Result register; arguments follow it.
        dst: Reg @ B,
        /// The host function.
        import: ImportId @ W,
        /// The number of arguments.
        argc: u8 @ A,
    }

    /// Replaces the running frame with a call to `func(args, ...,
    /// args+argc-1)`; the callee's results equal the caller's.
    0xA7 TailCall "tail_call" {
        /// The callee.
        func: FuncId @ W,
        /// First argument register.
        args: Reg @ B,
        /// The number of arguments.
        argc: u8 @ A,
    }

    /// Tail call through a typed function reference.
    0xA8 TailCallIndirect "tail_call_indirect" {
        /// The function reference.
        callee: Reg @ B,
        /// First argument register.
        args: Reg @ C,
        /// The number of arguments.
        argc: u8 @ A,
    }

    /// Return `src` to the caller.
    0xA9 Ret "ret" {
        /// The result register.
        src: Reg @ B,
    }

    /// Return with no result.
    0xAA RetVoid "ret_void" {}

    /// Raise `src` as an error: control moves to the innermost covering
    /// handler, in this frame or a caller's.
    0xAB Throw "throw" {
        /// The error value (`dyn` or `ref`).
        src: Reg @ B,
    }

    /// `dst` = the numeric [`ErrorKind`](crate::ErrorKind) code of a caught
    /// runtime error, or 0 for any other value.
    0xAC ErrCode "err_code" {
        /// Destination `u32` register.
        dst: Reg @ B,
        /// The caught value.
        src: Reg @ C,
    }

    /// A point where the garbage collector may run and fuel is charged.
    /// Calls and allocations are safepoints too; every loop must pass one.
    0xAD Safepoint "safepoint" {}

    /// Abort the run with `Unreachable` (E0109).
    0xAE Unreachable "unreachable" {}

    /// Raise the runtime error `kind` (any catchable kind) with `src` as its
    /// payload, at this pc: how a code generator raises HIR's `NoMatch`
    /// (E0200) with the scrutinee, or an `ArgumentError` from a prologue.
    0xAF Raise "raise" {
        /// The payload (`dyn`).
        src: Reg @ B,
        /// The error kind.
        kind: ErrorKind @ A,
    }

    // ---------------------------------------------------------------------
    // Closures and cells
    // ---------------------------------------------------------------------

    /// `dst` = a closure of `func` capturing, by value, the registers
    /// `dst+1 ..= dst+n` (`n` = the function's capture count).
    0xB0 MakeClosure "make_closure" {
        /// Destination register; the captured values follow it.
        dst: Reg @ B,
        /// The function.
        func: FuncId @ W,
    }

    /// `dst` = the running closure's captured value `idx`.
    0xB1 GetUpval "get_upval" {
        /// Destination register of the capture's type.
        dst: Reg @ B,
        /// The capture.
        idx: UpvalIdx @ C,
    }

    /// `dst` = a new cell of type `ty` holding `src`.
    0xB2 NewCell "new_cell" {
        /// Destination `ref` register.
        dst: Reg @ B,
        /// The initial value.
        src: Reg @ C,
        /// The cell type.
        ty: TypeRef @ D,
    }

    /// `dst` = the value in `cell` (a `ref` to a cell type, or a `dyn`
    /// holding a cell or a reference; TypeError otherwise).
    0xB3 CellGet "cell_get" {
        /// Destination register.
        dst: Reg @ B,
        /// The cell.
        cell: Reg @ C,
    }

    /// Store `src` into `cell` (operand rules as `cell_get`).
    0xB4 CellSet "cell_set" {
        /// The cell.
        cell: Reg @ B,
        /// The value.
        src: Reg @ C,
    }

    // ---------------------------------------------------------------------
    // References (PHP `&`). A reference is a cell of `dyn` of kind
    // `reference`; a container slot holding one is transparent: value reads
    // and writes of the slot go through to the reference's value.
    // ---------------------------------------------------------------------

    /// `dst` = a new reference holding `src` (a variable taken by reference,
    /// PHP `$r = &$x` when `$x` is not yet one). (A)
    0xB5 NewRef "new_ref" {
        /// Destination `dyn` or `ref cell dyn` register.
        dst: Reg @ B,
        /// The initial value (`dyn`).
        src: Reg @ C,
    }

    /// `dst` = a reference to the slot `obj[key]` (PHP `&$a[k]`): the slot's
    /// reference if it holds one; else the slot (created with nil if
    /// absent in a map) is separated as by `dsep_index` and replaced by a
    /// new reference holding its value. (A)
    0xB6 DRefIndex "dref_index" {
        /// Destination `dyn` or `ref cell dyn` register.
        dst: Reg @ B,
        /// The array or map.
        obj: Reg @ C,
        /// The key.
        key: Reg @ D,
    }

    /// `dst` = a reference to the property `obj.name` (PHP `&$o->p`): a
    /// struct field of type `dyn`, or a map entry with that string key
    /// (created with nil if absent). (A)
    0xB7 DRefProp "dref_prop" {
        /// Destination `dyn` or `ref cell dyn` register.
        dst: Reg @ B,
        /// The object.
        obj: Reg @ C,
        /// The property name.
        name: NameRef @ D,
    }

    /// Make the slot `obj[key]` the reference `src` (PHP `$a[k] = &$x`),
    /// replacing what the slot held (a previous reference is unbound, not
    /// written through). TypeError if `src` is not a reference. (A)
    0xB8 DBindIndex "dbind_index" {
        /// The array or map.
        obj: Reg @ B,
        /// The key.
        key: Reg @ C,
        /// The reference.
        src: Reg @ D,
    }

    /// Make the property `obj.name` the reference `src` (PHP `$o->p =
    /// &$x`). (A)
    0xB9 DBindProp "dbind_prop" {
        /// The object.
        obj: Reg @ B,
        /// The property name.
        name: NameRef @ D,
        /// The reference.
        src: Reg @ C,
    }

    /// If the slot `obj[key]` holds a reference, replace it by the
    /// reference's current value (a container value separated as by `dup`);
    /// else nothing. Lets a code generator end a reference's hold on a slot
    /// it can prove no other holder uses (PHP drops such references when it
    /// copies the array).
    0xBA DUnrefIndex "dunref_index" {
        /// The array or map.
        obj: Reg @ B,
        /// The key.
        key: Reg @ C,
    }

    /// `dunref_index` for a property.
    0xBB DUnrefProp "dunref_prop" {
        /// The object.
        obj: Reg @ B,
        /// The property name.
        name: NameRef @ D,
    }

    // ---------------------------------------------------------------------
    // Typed heap objects. A nil object raises NullReference (E0101).
    // ---------------------------------------------------------------------

    /// `dst` = a new instance of struct type `ty`, every field at its
    /// type's default.
    0xC0 NewStruct "new_struct" {
        /// Destination `ref` register.
        dst: Reg @ B,
        /// The struct type.
        ty: TypeRef @ C,
    }

    /// `dst = obj.field`.
    0xC1 GetField "get_field" {
        /// Destination register of the field's type.
        dst: Reg @ B,
        /// The instance.
        obj: Reg @ C,
        /// The field slot.
        field: FieldIdx @ D,
    }

    /// `obj.field = src`.
    0xC2 SetField "set_field" {
        /// The instance.
        obj: Reg @ B,
        /// The field slot.
        field: FieldIdx @ D,
        /// The value.
        src: Reg @ C,
    }

    /// `dst` = a new array of type `ty` with `len` (an `i64`, at least 0)
    /// default elements.
    0xC3 NewArray "new_array" {
        /// Destination `ref` register.
        dst: Reg @ B,
        /// The length.
        len: Reg @ C,
        /// The array type.
        ty: TypeRef @ D,
    }

    /// `dst` = the array's length (`i64`).
    0xC4 ArrayLen "array_len" {
        /// Destination `i64` register.
        dst: Reg @ B,
        /// The array.
        arr: Reg @ C,
    }

    /// `dst = arr[idx]`; IndexOutOfBounds unless `0 <= idx < len`.
    0xC5 ArrayGet "array_get" {
        /// Destination register of the element type.
        dst: Reg @ B,
        /// The array.
        arr: Reg @ C,
        /// The `i64` index.
        idx: Reg @ D,
    }

    /// `arr[idx] = src`; IndexOutOfBounds unless `0 <= idx < len`.
    0xC6 ArraySet "array_set" {
        /// The array.
        arr: Reg @ B,
        /// The `i64` index.
        idx: Reg @ C,
        /// The value.
        src: Reg @ D,
    }

    /// Append `src` to the array.
    0xC7 ArrayPush "array_push" {
        /// The array.
        arr: Reg @ B,
        /// The value.
        src: Reg @ C,
    }

    /// `dst` = the removed last element; IndexOutOfBounds if empty.
    0xC8 ArrayPop "array_pop" {
        /// Destination register of the element type.
        dst: Reg @ B,
        /// The array.
        arr: Reg @ C,
    }

    /// `dst` = a new, empty map of type `ty`.
    0xC9 NewMap "new_map" {
        /// Destination `ref` register.
        dst: Reg @ B,
        /// The map type.
        ty: TypeRef @ C,
    }

    /// `dst` = the number of entries (`i64`).
    0xCA MapLen "map_len" {
        /// Destination `i64` register.
        dst: Reg @ B,
        /// The map.
        map: Reg @ C,
    }

    /// `dst = map[key]`; KeyNotFound if absent.
    0xCB MapGet "map_get" {
        /// Destination register of the value type.
        dst: Reg @ B,
        /// The map.
        map: Reg @ C,
        /// The key.
        key: Reg @ D,
    }

    /// `dst = map[key]`, or `nil` if absent (value type `dyn`, `str`, or
    /// `ref`).
    0xCC MapFind "map_find" {
        /// Destination register of the value type.
        dst: Reg @ B,
        /// The map.
        map: Reg @ C,
        /// The key.
        key: Reg @ D,
    }

    /// `dst` = whether `key` is present.
    0xCD MapHas "map_has" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// The map.
        map: Reg @ C,
        /// The key.
        key: Reg @ D,
    }

    /// `map[key] = src`: a new key goes last; an existing key keeps its
    /// position.
    0xCE MapSet "map_set" {
        /// The map.
        map: Reg @ B,
        /// The key.
        key: Reg @ C,
        /// The value.
        src: Reg @ D,
    }

    /// Remove `key` if present; the other entries keep their order.
    0xCF MapDel "map_del" {
        /// The map.
        map: Reg @ B,
        /// The key.
        key: Reg @ C,
    }

    /// Append `src` under the next integer key (PHP `$a[] = v`): one more
    /// than the largest integer key ever inserted, or 0.
    0xD0 MapPush "map_push" {
        /// The map.
        map: Reg @ B,
        /// The value.
        src: Reg @ C,
    }

    /// `dst` = a typed iterator over the array or map `src`.
    0xD1 IterNew "iter_new" {
        /// Destination `ref` (iterator type) register.
        dst: Reg @ B,
        /// The collection.
        src: Reg @ C,
    }

    /// Advance: if another element exists, `val` = its value and `has` =
    /// true; else `has` = false and `val` is untouched.
    0xD2 IterNext "iter_next" {
        /// Destination `bool` register.
        has: Reg @ B,
        /// The iterator.
        iter: Reg @ C,
        /// Destination register of the value type.
        val: Reg @ D,
    }

    /// `dst` = the key of the element the last `iter_next` produced.
    0xD3 IterKey "iter_key" {
        /// Destination register of the key type.
        dst: Reg @ B,
        /// The iterator.
        iter: Reg @ C,
    }

    /// `dst` = a shallow copy of `src` (array, map, struct, or cell; a new
    /// identity, so value semantics such as PHP's array assignment are
    /// explicit; O(1), the contents copied on the first write). Reference
    /// slots stay shared between the copies. Strings and callables are
    /// returned as is; iterators, coroutines, and references raise
    /// TypeError.
    0xD4 Dup "dup" {
        /// Destination register.
        dst: Reg @ B,
        /// Source register.
        src: Reg @ C,
    }

    // ---------------------------------------------------------------------
    // Strings (immutable byte strings)
    // ---------------------------------------------------------------------

    /// `dst` = the length in bytes (`i64`).
    0xE0 StrLen "str_len" {
        /// Destination `i64` register.
        dst: Reg @ B,
        /// The string.
        s: Reg @ C,
    }

    /// `dst = lhs ++ rhs`.
    0xE1 StrConcat "str_concat" {
        /// Destination `str` register.
        dst: Reg @ B,
        /// First part.
        lhs: Reg @ C,
        /// Second part.
        rhs: Reg @ D,
    }

    /// `dst` = the concatenation of the `count` strings in `first ..
    /// first+count` (the empty string if `count` is 0).
    0xE2 StrConcatN "str_concat_n" {
        /// Destination `str` register.
        dst: Reg @ B,
        /// First part.
        first: Reg @ C,
        /// Number of parts.
        count: u8 @ A,
    }

    /// `dst` = whether the strings have equal bytes.
    0xE3 StrEq "str_eq" {
        /// Destination `bool` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst` = -1, 0, or 1 (`i8`) by bytewise lexicographic order.
    0xE4 StrCmp "str_cmp" {
        /// Destination `i8` register.
        dst: Reg @ B,
        /// Left operand.
        lhs: Reg @ C,
        /// Right operand.
        rhs: Reg @ D,
    }

    /// `dst = s[start..end]` by byte offset, `start` in `range` and `end` in
    /// `range+1` (both `i64`); IndexOutOfBounds unless `0 <= start <= end <=
    /// len`; with `utf8`, InvalidStrIndex if a bound splits a character.
    0xE5 StrSlice "str_slice" {
        /// Destination `str` register.
        dst: Reg @ B,
        /// The string.
        s: Reg @ C,
        /// Start index; the end index is in the next register.
        range: Reg @ D,
        /// Whether bounds must fall on UTF-8 character boundaries.
        utf8: bool @ A,
    }

    /// `dst` = the byte at `idx` (`u8`); IndexOutOfBounds unless `0 <= idx
    /// < len`.
    0xE6 StrByte "str_byte" {
        /// Destination `u8` register.
        dst: Reg @ B,
        /// The string.
        s: Reg @ C,
        /// The `i64` index.
        idx: Reg @ D,
    }

    // ---------------------------------------------------------------------
    // Coroutines, generators, async. Coroutines are stackful and asymmetric:
    // `yield`/`await` suspend the innermost running coroutine with every
    // frame above its body; `resume` continues it. Values crossing a
    // suspension are `dyn`. Every instruction here is a safepoint.
    // ---------------------------------------------------------------------

    /// `dst` = a new coroutine, state `created`, that will run
    /// `func(dst+1, ..., dst+argc)` when first resumed. The arguments are
    /// copied now. (A)
    0xF0 CoroNew "coro_new" {
        /// Destination `dyn` or `ref coroutine` register; arguments follow it.
        dst: Reg @ B,
        /// The body.
        func: FuncId @ W,
        /// The number of arguments.
        argc: u8 @ A,
    }

    /// `dst` = a new coroutine running the closure or function reference
    /// `callee` with the arguments `dst+1 ..= dst+argc` (converted from
    /// `dyn` as `dcall` does when `callee` is `dyn`). (A)
    0xF1 CoroNewIndirect "coro_new_indirect" {
        /// Destination `dyn` or `ref coroutine` register; arguments follow it.
        dst: Reg @ B,
        /// The callable.
        callee: Reg @ C,
        /// The number of arguments.
        argc: u8 @ A,
    }

    /// Suspend the running coroutine in state `yielded`, handing `src` to
    /// its resumer under the next automatic key (PHP's generator rule: one
    /// more than the largest integer key yielded so far, never below 0, so
    /// after only `yield -5 => x` it is 0); when resumed,
    /// `dst` = the value sent. The yield is an unwind point: `resume_throw`
    /// and `coro_close` raise here. CannotSuspend (E0111) outside a
    /// coroutine or across a host frame.
    0xF2 Yield "yield" {
        /// Receives the value sent by the next `resume` (`dyn`).
        dst: Reg @ B,
        /// The value yielded (`dyn`).
        src: Reg @ C,
    }

    /// Suspend the running coroutine in state `awaiting`, handing the
    /// awaitable `src` to its resumer (the scheduler); when resumed, `dst` =
    /// the value sent (the awaited result), or the error sent by
    /// `resume_throw` is raised here. CannotSuspend as `yield`.
    0xF3 Await "await" {
        /// Receives the awaited result (`dyn`).
        dst: Reg @ B,
        /// The awaitable (`dyn`).
        src: Reg @ C,
    }

    /// Resume `coro` (state `created`, `yielded`, or `awaiting`), sending
    /// `src`; when it next suspends or finishes, `dst` = the value it
    /// yielded, the awaitable it awaits, or its return value (nil for a void
    /// body). An error escaping its body marks it `failed` and is raised
    /// here. InvalidCoroState (E0110) for any other state.
    0xF4 Resume "resume" {
        /// Receives the yielded, awaited, or returned value (`dyn`).
        dst: Reg @ B,
        /// The coroutine.
        coro: Reg @ C,
        /// The value sent (`dyn`; ignored by a `created` coroutine).
        src: Reg @ D,
    }

    /// Resume `coro` by raising `src` at its suspension point (in a
    /// `created` coroutine: before its first instruction, where no handler
    /// covers it). Otherwise as `resume`.
    0xF5 ResumeThrow "resume_throw" {
        /// Receives the next yielded, awaited, or returned value (`dyn`).
        dst: Reg @ B,
        /// The coroutine.
        coro: Reg @ C,
        /// The error value raised inside it (`dyn`).
        src: Reg @ D,
    }

    /// `dst` = the [`CoroState`](crate::CoroState) code of `coro`, as a
    /// `u8`.
    0xF6 CoroStatus "coro_status" {
        /// Destination `u8` register.
        dst: Reg @ B,
        /// The coroutine.
        coro: Reg @ C,
    }

    /// `dst` = the running coroutine, or nil on the main stack.
    0xF7 CoroCurrent "coro_current" {
        /// Destination `dyn` or `ref coroutine` register.
        dst: Reg @ B,
    }

    /// As `yield`, with an explicit key (PHP `yield $k => $v`); an integer
    /// key larger than any before it moves the automatic-key counter.
    0xF9 YieldKv "yield_kv" {
        /// Receives the value sent by the next `resume` (`dyn`).
        dst: Reg @ B,
        /// The key (`dyn`).
        key: Reg @ C,
        /// The value yielded (`dyn`).
        src: Reg @ D,
    }

    /// Close `coro` (Python `close()`/`aclose()`, PHP generator
    /// destruction): `created` → `returned` without running; `returned` or
    /// `failed` → nothing. A suspended coroutine is marked *closing* and
    /// `src` (the close signal, e.g. `GeneratorExit`) is raised at its
    /// suspension point, so its `finally` blocks run. If the body then
    /// returns, or the error escaping it is `src` itself, the close
    /// succeeds (`returned`; `dst` = the return value or nil). Another
    /// error marks it `failed` and is raised here. An `await` while closing
    /// suspends normally (`dst` = the awaitable; drive it with `resume`); a
    /// `yield` while closing leaves it suspended there and raises
    /// CloseIgnored (E0113) here.
    0xFA CoroClose "coro_close" {
        /// Receives the return value, the awaitable, or nil (`dyn`).
        dst: Reg @ B,
        /// The coroutine.
        coro: Reg @ C,
        /// The close signal raised inside it (`dyn`).
        src: Reg @ D,
    }

    /// `dst` = the key of the value `coro` most recently yielded, or nil
    /// before its first yield (PHP `Generator::key()`).
    0xFB CoroKey "coro_key" {
        /// Destination `dyn` register.
        dst: Reg @ B,
        /// The coroutine.
        coro: Reg @ C,
    }

    /// `dst` = the return value of a `returned` coroutine (Python's
    /// `StopIteration.value`, PHP `Generator::getReturn()`), nil for a void
    /// body. InvalidCoroState (E0110) in any other state.
    0xFC CoroResult "coro_result" {
        /// Destination `dyn` register.
        dst: Reg @ B,
        /// The coroutine.
        coro: Reg @ C,
    }

    /// Start a task: make a coroutine as `coro_new_indirect` does and pass
    /// it to the module's `spawn` hook (the host scheduler); `dst` = the
    /// hook's result (a task handle). NoScheduler (E0112) without the hook.
    /// (A)
    0xF8 Spawn "spawn" {
        /// Receives the task handle (`dyn`); arguments follow it.
        dst: Reg @ B,
        /// The callable.
        callee: Reg @ C,
        /// The number of arguments.
        argc: u8 @ A,
    }
}

impl Inst {
    /// The branch target of `jmp`, `jmp_if`, or `jmp_if_not`, if this is
    /// one. (Switch targets live in the function's jump tables.)
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, Reg, Target};
    ///
    /// assert_eq!(Inst::Jmp { target: Target(4) }.branch_target(), Some(Target(4)));
    /// assert_eq!(Inst::Mov { dst: Reg(0), src: Reg(1) }.branch_target(), None);
    /// ```
    #[must_use]
    pub const fn branch_target(&self) -> Option<Target> {
        match *self {
            Inst::Jmp { target } | Inst::JmpIf { target, .. } | Inst::JmpIfNot { target, .. } => {
                Some(target)
            }
            _ => None,
        }
    }

    /// The assembler name of this instruction's opcode.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Inst;
    ///
    /// assert_eq!(Inst::Safepoint {}.mnemonic(), "safepoint");
    /// ```
    #[must_use]
    pub const fn mnemonic(&self) -> &'static str {
        self.opcode().mnemonic()
    }
}

/// `mnemonic.modifiers operand, operand`, with raw `@pc` branch targets.
impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.render(f, &Cx::EMPTY)
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    #[test]
    fn decoded_instructions_stay_eight_bytes() {
        assert_eq!(core::mem::size_of::<Inst>(), 8);
    }

    #[test]
    fn opcodes_round_trip_through_their_byte() {
        for &op in Opcode::ALL {
            assert_eq!(Opcode::from_u8(op as u8), Some(op));
        }
    }

    #[test]
    fn modifiers_print_as_suffixes() {
        let i = Inst::IntCast {
            dst: Reg(1),
            src: Reg(2),
            conv: IntConv::new(IntTy::I64, IntTy::U8, crate::Overflow::Wrap),
        };
        assert_eq!(i.to_string(), "int_cast.i64.u8.wrap r1, r2");
        let d = Inst::DAdd {
            dst: Reg(0),
            lhs: Reg(1),
            rhs: Reg(2),
            pol: Policy::new(),
        };
        assert_eq!(d.to_string(), "dadd r0, r1, r2");
        let c = Inst::Call {
            dst: Reg(3),
            func: FuncId(1),
            argc: 2,
        };
        assert_eq!(c.to_string(), "call r3, f1, 2");
    }

    #[test]
    fn error_kinds_print_their_code_and_name() {
        let r = Inst::Raise {
            src: Reg(3),
            kind: ErrorKind::NoMatch,
        };
        assert_eq!(r.to_string(), "raise.E0200 r3  ; NoMatch");
        assert_eq!(Inst::from_bytes(r.to_bytes()), Ok(r));
        // Traps are not error values.
        let trap = [0xAF, 107, 0, 0, 0, 0, 0, 0];
        assert!(Inst::from_bytes(trap).is_err());
    }

    #[test]
    fn immediates_keep_their_sign() {
        let i = Inst::LoadInt {
            dst: Reg(0),
            val: -5,
            ty: IntTy::I32,
        };
        assert_eq!(Inst::from_bytes(i.to_bytes()), Ok(i));
        assert_eq!(i.to_string(), "load_int.i32 r0, -5");
    }
}
