//! Register types, dynamic kinds, and the module's type table.

use alloc::vec::Vec;
use core::fmt;

use crate::ids::{FuncId, StrId, TypeId};
use crate::policy::{FloatTy, IntTy};

/// The declared type of a register, a global, a field, or a parameter.
///
/// Every register slot is 64 bits wide. Scalars (`bool`, the integers, the
/// floats, `char`) are stored unboxed in typed registers. [`Dyn`](ValType::Dyn)
/// holds a dynamically typed value (the NaN-boxed value of the dynamic
/// languages). [`Str`](ValType::Str), [`Ref`](ValType::Ref), and `Dyn`
/// holding a heap object share **one** reference representation, and `nil`
/// is the null reference, so a heap instruction works the same whichever of
/// those registers holds the object. Declared types are what make a frame's
/// garbage-collection roots exact: a register is traced if and only if its
/// type [`is_reference`](ValType::is_reference).
///
/// # Examples
///
/// ```
/// use bytecode_lang::{IntTy, TypeId, ValType};
///
/// assert_eq!(ValType::int(IntTy::U8), ValType::U8);
/// assert_eq!(ValType::I64.as_int(), Some(IntTy::I64));
/// assert!(ValType::Dyn.is_reference());
/// assert!(!ValType::F64.is_reference());
/// assert_eq!(ValType::Ref(TypeId(3)).to_string(), "ref t3");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ValType {
    /// `false` or `true`.
    Bool,
    /// Signed 8-bit integer.
    I8,
    /// Signed 16-bit integer.
    I16,
    /// Signed 32-bit integer.
    I32,
    /// Signed 64-bit integer.
    I64,
    /// Unsigned 8-bit integer.
    U8,
    /// Unsigned 16-bit integer.
    U16,
    /// Unsigned 32-bit integer.
    U32,
    /// Unsigned 64-bit integer.
    U64,
    /// IEEE 754 binary32.
    F32,
    /// IEEE 754 binary64.
    F64,
    /// A Unicode scalar value.
    Char,
    /// An immutable byte string, or `nil`.
    Str,
    /// A dynamically typed value of any [`Kind`].
    Dyn,
    /// A reference to a heap object of the given type-table entry, or `nil`.
    Ref(TypeId),
}

impl ValType {
    /// The integer value type for `ty`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntTy, ValType};
    ///
    /// assert_eq!(ValType::int(IntTy::I32), ValType::I32);
    /// ```
    #[must_use]
    pub const fn int(ty: IntTy) -> Self {
        match ty {
            IntTy::I8 => ValType::I8,
            IntTy::I16 => ValType::I16,
            IntTy::I32 => ValType::I32,
            IntTy::I64 => ValType::I64,
            IntTy::U8 => ValType::U8,
            IntTy::U16 => ValType::U16,
            IntTy::U32 => ValType::U32,
            IntTy::U64 => ValType::U64,
        }
    }

    /// The float value type for `ty`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{FloatTy, ValType};
    ///
    /// assert_eq!(ValType::float(FloatTy::F32), ValType::F32);
    /// ```
    #[must_use]
    pub const fn float(ty: FloatTy) -> Self {
        match ty {
            FloatTy::F32 => ValType::F32,
            FloatTy::F64 => ValType::F64,
        }
    }

    /// The integer type, if this is an integer value type.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntTy, ValType};
    ///
    /// assert_eq!(ValType::U64.as_int(), Some(IntTy::U64));
    /// assert_eq!(ValType::F64.as_int(), None);
    /// ```
    #[must_use]
    pub const fn as_int(self) -> Option<IntTy> {
        match self {
            ValType::I8 => Some(IntTy::I8),
            ValType::I16 => Some(IntTy::I16),
            ValType::I32 => Some(IntTy::I32),
            ValType::I64 => Some(IntTy::I64),
            ValType::U8 => Some(IntTy::U8),
            ValType::U16 => Some(IntTy::U16),
            ValType::U32 => Some(IntTy::U32),
            ValType::U64 => Some(IntTy::U64),
            _ => None,
        }
    }

    /// Whether a register of this type can hold a heap reference, and so is
    /// a garbage-collection root.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{TypeId, ValType};
    ///
    /// assert!(ValType::Str.is_reference());
    /// assert!(ValType::Ref(TypeId(0)).is_reference());
    /// assert!(!ValType::Bool.is_reference());
    /// ```
    #[must_use]
    pub const fn is_reference(self) -> bool {
        matches!(self, ValType::Str | ValType::Dyn | ValType::Ref(_))
    }

    /// The encoding tag (0..=14).
    pub(crate) const fn tag(self) -> u8 {
        match self {
            ValType::Bool => 0,
            ValType::I8 => 1,
            ValType::I16 => 2,
            ValType::I32 => 3,
            ValType::I64 => 4,
            ValType::U8 => 5,
            ValType::U16 => 6,
            ValType::U32 => 7,
            ValType::U64 => 8,
            ValType::F32 => 9,
            ValType::F64 => 10,
            ValType::Char => 11,
            ValType::Str => 12,
            ValType::Dyn => 13,
            ValType::Ref(_) => 14,
        }
    }

    /// The type for a tag that carries no index, or `None` (tag 14 needs
    /// its type index read separately).
    pub(crate) const fn from_simple_tag(tag: u8) -> Option<ValType> {
        Some(match tag {
            0 => ValType::Bool,
            1 => ValType::I8,
            2 => ValType::I16,
            3 => ValType::I32,
            4 => ValType::I64,
            5 => ValType::U8,
            6 => ValType::U16,
            7 => ValType::U32,
            8 => ValType::U64,
            9 => ValType::F32,
            10 => ValType::F64,
            11 => ValType::Char,
            12 => ValType::Str,
            13 => ValType::Dyn,
            _ => return None,
        })
    }
}

impl fmt::Display for ValType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            ValType::Bool => "bool",
            ValType::I8 => "i8",
            ValType::I16 => "i16",
            ValType::I32 => "i32",
            ValType::I64 => "i64",
            ValType::U8 => "u8",
            ValType::U16 => "u16",
            ValType::U32 => "u32",
            ValType::U64 => "u64",
            ValType::F32 => "f32",
            ValType::F64 => "f64",
            ValType::Char => "char",
            ValType::Str => "str",
            ValType::Dyn => "dyn",
            ValType::Ref(ty) => return write!(f, "ref {ty}"),
        };
        f.write_str(name)
    }
}

code_enum! {
    /// The kind of a dynamic value: what [`Inst::TypeOf`](crate::Inst::TypeOf)
    /// returns and [`Inst::IsKind`](crate::Inst::IsKind) tests.
    ///
    /// Dynamic integers are 64-bit signed and dynamic floats are binary64,
    /// whatever their boxed representation.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Kind;
    ///
    /// assert_eq!(Kind::Map.code(), 7);
    /// assert_eq!(Kind::from_code(5), Some(Kind::Str));
    /// assert_eq!(Kind::Object.to_string(), "object");
    /// ```
    Kind {
        /// `nil`: the absent value, and the null reference.
        Nil = 0 => "nil",
        /// A boolean.
        Bool = 1 => "bool",
        /// A 64-bit signed integer.
        Int = 2 => "int",
        /// A binary64 float.
        Float = 3 => "float",
        /// A Unicode scalar value.
        Char = 4 => "char",
        /// An immutable byte string.
        Str = 5 => "str",
        /// An array.
        Array = 6 => "array",
        /// An ordered hash map.
        Map = 7 => "map",
        /// A struct instance.
        Object = 8 => "object",
        /// A callable: a closure, a function, or a host function.
        Function = 9 => "function",
        /// A mutable cell.
        Cell = 10 => "cell",
        /// An iterator.
        Iter = 11 => "iter",
        /// A runtime error value (see [`Inst::ErrCode`](crate::Inst::ErrCode)).
        Error = 12 => "error",
        /// A coroutine: a suspendable stack of frames (generators, async
        /// tasks; see [`Inst::CoroNew`](crate::Inst::CoroNew)).
        Coroutine = 13 => "coroutine",
        /// A reference (PHP `&`): a box holding one dynamic value, made by
        /// [`Inst::NewRef`](crate::Inst::NewRef) or by taking a reference to
        /// a container slot ([`Inst::DRefIndex`](crate::Inst::DRefIndex),
        /// [`Inst::DRefProp`](crate::Inst::DRefProp)). It is a cell of `dyn`
        /// (`cell_get`/`cell_set` read and write it), and a container slot
        /// holding one is *transparent*: reads and writes of the slot go
        /// through to the reference's value (`specs/LSB.md` §5.16).
        Reference = 14 => "reference",
    }
}

code_enum! {
    /// The state of a coroutine, as [`Inst::CoroStatus`](crate::Inst::CoroStatus)
    /// reports it (a `u8` code).
    ///
    /// ```text
    /// Created --resume--> Running --yield--> Yielded  --resume--> Running ...
    ///                        |    --await--> Awaiting --resume--> Running ...
    ///                        |    --ret----> Returned
    ///                        +--- error escapes the body --> Failed
    /// ```
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::CoroState;
    ///
    /// assert_eq!(CoroState::Awaiting.code(), 3);
    /// assert_eq!(CoroState::from_code(4), Some(CoroState::Returned));
    /// assert!(CoroState::Yielded.is_resumable());
    /// assert!(!CoroState::Failed.is_resumable());
    /// ```
    CoroState {
        /// Made by `coro_new`, never resumed: its body has not started.
        Created = 0 => "created",
        /// Executing: it is the running coroutine or one of its resumers.
        Running = 1 => "running",
        /// Suspended by `yield`.
        Yielded = 2 => "yielded",
        /// Suspended by `await`.
        Awaiting = 3 => "awaiting",
        /// Its body returned; it can never run again.
        Returned = 4 => "returned",
        /// An error escaped its body; it can never run again.
        Failed = 5 => "failed",
    }
}

impl CoroState {
    /// Whether `resume`, `resume_throw`, and `coro_close` would run code in
    /// a coroutine in this state (`Created`, `Yielded`, or `Awaiting`).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::CoroState;
    ///
    /// assert!(CoroState::Created.is_resumable());
    /// assert!(!CoroState::Running.is_resumable());
    /// ```
    #[must_use]
    pub const fn is_resumable(self) -> bool {
        matches!(
            self,
            CoroState::Created | CoroState::Yielded | CoroState::Awaiting
        )
    }
}

code_enum! {
    /// A static representation crossing the `dyn` boundary: the modifier of
    /// [`Inst::ToDyn`](crate::Inst::ToDyn) and
    /// [`Inst::FromDyn`](crate::Inst::FromDyn).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Prim;
    ///
    /// assert_eq!(Prim::I64.to_string(), "i64");
    /// assert_eq!(Prim::from_code(13), Some(Prim::Ref));
    /// ```
    Prim {
        /// `bool`.
        Bool = 0 => "bool",
        /// `i8`.
        I8 = 1 => "i8",
        /// `i16`.
        I16 = 2 => "i16",
        /// `i32`.
        I32 = 3 => "i32",
        /// `i64`.
        I64 = 4 => "i64",
        /// `u8`.
        U8 = 5 => "u8",
        /// `u16`.
        U16 = 6 => "u16",
        /// `u32`.
        U32 = 7 => "u32",
        /// `u64`.
        U64 = 8 => "u64",
        /// `f32`.
        F32 = 9 => "f32",
        /// `f64`.
        F64 = 10 => "f64",
        /// `char`.
        Char = 11 => "char",
        /// `str`.
        Str = 12 => "str",
        /// Any `ref` type (only `to_dyn`; `from_dyn` to a reference is a
        /// checked [`Cast`](crate::Inst::Cast)).
        Ref = 13 => "ref",
    }
}

/// An entry of the module's type table.
///
/// Types refer to each other by [`TypeId`], so recursive types (a list node
/// pointing at itself) are plain data and the table never nests.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{FuncType, TypeDef, ValType};
///
/// let sig = TypeDef::Func(FuncType { params: vec![ValType::I64], results: vec![ValType::I64] });
/// assert_eq!(sig.to_string(), "func (i64) -> (i64)");
/// assert_eq!(TypeDef::Array(ValType::Dyn).to_string(), "array dyn");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum TypeDef {
    /// A function signature, used by functions, imports, closures, and
    /// indirect calls.
    Func(FuncType),
    /// A struct (record) type: named fields at fixed slots, named methods,
    /// and an optional parent for single inheritance.
    Struct(StructDef),
    /// A growable array of one element type.
    Array(ValType),
    /// An insertion-ordered hash map (the PHP array and the Python dict).
    Map {
        /// The key type.
        key: ValType,
        /// The value type.
        value: ValType,
    },
    /// A mutable box holding one value: how closures share a mutable
    /// captured variable.
    Cell(ValType),
    /// An iterator yielding keys and values of these types.
    Iter {
        /// The key type (`i64` for arrays).
        key: ValType,
        /// The value type.
        value: ValType,
    },
    /// A coroutine handle. Values crossing a suspension (yielded, sent,
    /// returned) are always `dyn`: a coroutine is stackful, so a `yield` in a
    /// nested call cannot know its coroutine's static type.
    Coroutine,
}

impl TypeDef {
    /// The encoding tag.
    pub(crate) const fn tag(&self) -> u8 {
        match self {
            TypeDef::Func(_) => 0,
            TypeDef::Struct(_) => 1,
            TypeDef::Array(_) => 2,
            TypeDef::Map { .. } => 3,
            TypeDef::Cell(_) => 4,
            TypeDef::Iter { .. } => 5,
            TypeDef::Coroutine => 6,
        }
    }

    /// Whether two equal definitions denote the same type. Structs are
    /// nominal (two identical declarations are two types); everything else
    /// is structural, so the builder deduplicates it.
    pub(crate) const fn is_structural(&self) -> bool {
        !matches!(self, TypeDef::Struct(_))
    }
}

/// A function signature.
///
/// LSB functions return zero or one value; the verifier (v0.5) rejects more.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{FuncType, ValType};
///
/// let sig = FuncType { params: vec![ValType::Dyn, ValType::Dyn], results: vec![] };
/// assert_eq!(sig.to_string(), "func (dyn, dyn) -> ()");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct FuncType {
    /// Parameter types, in order.
    pub params: Vec<ValType>,
    /// Result types (zero or one).
    pub results: Vec<ValType>,
}

/// A struct (record) type.
///
/// Field slots are numbered across the parent chain: a struct's field list
/// begins with its parent's fields, so a child instance can be used wherever
/// the parent is expected. Methods are looked up by name by
/// [`Inst::GetProp`](crate::Inst::GetProp), the struct's own first, then the
/// parent's.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Field, StrId, StructDef, ValType};
///
/// let point = StructDef {
///     name: StrId(0),
///     parent: None,
///     fields: vec![Field { name: StrId(1), ty: ValType::F64 }, Field { name: StrId(2), ty: ValType::F64 }],
///     methods: vec![],
/// };
/// assert_eq!(point.fields.len(), 2);
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct StructDef {
    /// The type's name.
    pub name: StrId,
    /// The parent struct type, if any.
    pub parent: Option<TypeId>,
    /// All fields, the parent's first.
    pub fields: Vec<Field>,
    /// Methods declared by this type (not the parent's).
    pub methods: Vec<Method>,
}

/// A named, typed field of a [`StructDef`].
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Field, StrId, ValType};
///
/// let f = Field { name: StrId(4), ty: ValType::Dyn };
/// assert_eq!(f.ty, ValType::Dyn);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Field {
    /// The field's name.
    pub name: StrId,
    /// The field's type.
    pub ty: ValType,
}

/// A named method of a [`StructDef`]: the function receives the instance as
/// its first parameter.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{FuncId, Method, StrId};
///
/// let m = Method { name: StrId(7), func: FuncId(2) };
/// assert_eq!(m.func, FuncId(2));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Method {
    /// The method's name.
    pub name: StrId,
    /// The implementing function.
    pub func: FuncId,
}

fn write_list(f: &mut fmt::Formatter<'_>, types: &[ValType]) -> fmt::Result {
    f.write_str("(")?;
    for (i, ty) in types.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{ty}")?;
    }
    f.write_str(")")
}

impl fmt::Display for FuncType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("func ")?;
        write_list(f, &self.params)?;
        f.write_str(" -> ")?;
        write_list(f, &self.results)
    }
}

impl fmt::Display for StructDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "struct {}", self.name)?;
        if let Some(parent) = self.parent {
            write!(f, " : {parent}")?;
        }
        f.write_str(" {")?;
        for (i, field) in self.fields.iter().enumerate() {
            let sep = if i == 0 { " " } else { ", " };
            write!(f, "{sep}#{i} {}: {}", field.name, field.ty)?;
        }
        f.write_str(" }")?;
        if !self.methods.is_empty() {
            f.write_str(" methods {")?;
            for (i, method) in self.methods.iter().enumerate() {
                let sep = if i == 0 { " " } else { ", " };
                write!(f, "{sep}{} = {}", method.name, method.func)?;
            }
            f.write_str(" }")?;
        }
        Ok(())
    }
}

impl fmt::Display for TypeDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeDef::Func(sig) => write!(f, "{sig}"),
            TypeDef::Struct(def) => write!(f, "{def}"),
            TypeDef::Array(elem) => write!(f, "array {elem}"),
            TypeDef::Map { key, value } => write!(f, "map {key} -> {value}"),
            TypeDef::Cell(elem) => write!(f, "cell {elem}"),
            TypeDef::Iter { key, value } => write!(f, "iter {key} -> {value}"),
            TypeDef::Coroutine => f.write_str("coroutine"),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;

    #[test]
    fn simple_tags_round_trip() {
        for tag in 0..=13u8 {
            let ty = ValType::from_simple_tag(tag).map(ValType::tag);
            assert_eq!(ty, Some(tag));
        }
        assert_eq!(ValType::from_simple_tag(14), None);
        assert_eq!(ValType::Ref(TypeId(9)).tag(), 14);
    }

    #[test]
    fn int_value_types_map_back() {
        for &ty in IntTy::ALL {
            assert_eq!(ValType::int(ty).as_int(), Some(ty));
        }
    }

    #[test]
    fn struct_display_lists_fields_and_methods() {
        let def = StructDef {
            name: StrId(1),
            parent: Some(TypeId(0)),
            fields: vec![Field {
                name: StrId(2),
                ty: ValType::I64,
            }],
            methods: vec![Method {
                name: StrId(3),
                func: FuncId(4),
            }],
        };
        assert_eq!(
            TypeDef::Struct(def).to_string(),
            "struct s1 : t0 { #0 s2: i64 } methods { s3 = f4 }"
        );
    }
}
