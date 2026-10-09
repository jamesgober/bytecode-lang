//! Dense index types.
//!
//! Every cross-reference in a module is a plain integer index into a table,
//! never a pointer or a name lookup, so a decoded module is usable as is and a
//! VM resolves each reference with one bounds-checked load. Each kind of index
//! has its own newtype so a function index can never be passed where a
//! constant index is expected.
//!
//! The decoder checks only that indices are well-formed numbers; whether an
//! index is *in range* for its table is the verifier's job (see
//! `docs/API.md`), and every accessor that follows an index returns `Option`.

index_type! {
    /// A register in the running function's frame.
    ///
    /// Registers are numbered from zero. The first registers hold the
    /// function's parameters, in order; the rest are locals. Every register
    /// has a declared [`ValType`](crate::ValType). The 16-bit width allows
    /// 65,536 registers per function.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Reg;
    ///
    /// assert_eq!(Reg(3).to_string(), "r3");
    /// assert_eq!(Reg(3).index(), 3);
    /// ```
    Reg(u16) => "r"
}

index_type! {
    /// An instruction index within one function: the destination of a branch.
    ///
    /// Targets are absolute, already resolved positions in the function's
    /// code. They are never symbolic labels, so the encoded form can be
    /// executed without a fix-up table (the defect class of family issue H02).
    /// Every instruction is one slot wide, so every target is an instruction
    /// boundary.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Target;
    ///
    /// assert_eq!(Target(12).to_string(), "@12");
    /// ```
    Target(u32) => "@"
}

index_type! {
    /// An index into the module's constant pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ConstId;
    ///
    /// assert_eq!(ConstId(0).to_string(), "k0");
    /// ```
    ConstId(u32) => "k"
}

index_type! {
    /// An index into the module's function table.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::FuncId;
    ///
    /// assert_eq!(FuncId(2).to_string(), "f2");
    /// ```
    FuncId(u32) => "f"
}

index_type! {
    /// An index into the module's import table (host functions).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ImportId;
    ///
    /// assert_eq!(ImportId(1).to_string(), "imp1");
    /// ```
    ImportId(u32) => "imp"
}

index_type! {
    /// An index into the module's global table.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::GlobalId;
    ///
    /// assert_eq!(GlobalId(4).to_string(), "g4");
    /// ```
    GlobalId(u32) => "g"
}

index_type! {
    /// An index into the module's type table.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::TypeId;
    ///
    /// assert_eq!(TypeId(7).to_string(), "t7");
    /// ```
    TypeId(u32) => "t"
}

index_type! {
    /// An index into the module's string table.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::StrId;
    ///
    /// assert_eq!(StrId(5).to_string(), "s5");
    /// ```
    StrId(u32) => "s"
}

index_type! {
    /// An index into the running function's name list, which maps it to a
    /// [`StrId`].
    ///
    /// Property instructions need two registers and a name in one eight-byte
    /// instruction, which leaves 16 bits for the name. Routing names through a
    /// per-function list (as Lua and V8 do with their per-function constant
    /// pools) keeps the module's string table unbounded while each function
    /// may still name 65,536 distinct properties.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::NameRef;
    ///
    /// assert_eq!(NameRef(0).to_string(), "n0");
    /// ```
    NameRef(u16) => "n"
}

index_type! {
    /// An index into the running function's type list, which maps it to a
    /// [`TypeId`].
    ///
    /// Used by the instructions that need a type next to two or three
    /// registers ([`Inst::IsType`](crate::Inst::IsType),
    /// [`Inst::NewArray`](crate::Inst::NewArray), ...), for the same reason as
    /// [`NameRef`].
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::TypeRef;
    ///
    /// assert_eq!(TypeRef(1).to_string(), "ty1");
    /// ```
    TypeRef(u16) => "ty"
}

index_type! {
    /// An index into the running function's jump tables (see
    /// [`Inst::Switch`](crate::Inst::Switch)).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::TableId;
    ///
    /// assert_eq!(TableId(0).to_string(), "jt0");
    /// ```
    TableId(u32) => "jt"
}

index_type! {
    /// An index into the running function's call shapes (see
    /// [`Inst::DCallShape`](crate::Inst::DCallShape) and
    /// [`CallShape`](crate::CallShape)): which window arguments are positional,
    /// named, or spread. 16-bit, like [`NameRef`], so a function may use 65,536
    /// distinct shapes.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ShapeId;
    ///
    /// assert_eq!(ShapeId(2).to_string(), "cs2");
    /// ```
    ShapeId(u16) => "cs"
}

index_type! {
    /// A field slot of a struct type, counted across the whole parent chain
    /// (a struct's fields start with its parent's).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::FieldIdx;
    ///
    /// assert_eq!(FieldIdx(2).to_string(), "#2");
    /// ```
    FieldIdx(u16) => "#"
}

index_type! {
    /// A captured value of the running closure (see
    /// [`Inst::GetUpval`](crate::Inst::GetUpval)).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::UpvalIdx;
    ///
    /// assert_eq!(UpvalIdx(0).to_string(), "u0");
    /// ```
    UpvalIdx(u16) => "u"
}
