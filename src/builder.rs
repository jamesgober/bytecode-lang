//! Building modules: [`ModuleBuilder`] and [`FunctionBuilder`].
//!
//! The builders own every structural guarantee a code generator should not
//! have to think about: strings, structural types, and constants are
//! deduplicated; registers are numbered and typed as they are declared;
//! branches are written against [`Label`]s and resolved when the function is
//! added, so **a built function never contains an unresolved or out-of-range
//! branch target**; and [`FunctionBuilder::parallel_move`] turns a set of
//! simultaneous moves (block arguments, the bytecode stand-in for SSA phis)
//! into a correct sequence, breaking cycles through a temporary (the defect
//! class of family issue H01, where `jump H(b, a)` was emitted as two plain
//! moves and lost a value).
//!
//! Builder methods that cannot fail on their own (declaring a register,
//! emitting an instruction) never return `Result`. When one of them does
//! hit a limit, or is handed a label from another function, the builder
//! records the first such error and reports it from
//! [`ModuleBuilder::add_function`] or [`ModuleBuilder::finish`], so a
//! code generator can emit straight-line code and check once.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::fmt;

use crate::encode::section_sizes;
use crate::ids::{
    ConstId, FuncId, GlobalId, ImportId, NameRef, Reg, StrId, TableId, Target, TypeId, TypeRef,
    UpvalIdx,
};
use crate::inst::Inst;
use crate::module::{
    Callee, Const, Export, ExportItem, Function, Global, Handler, Hook, HookBinding, Import,
    JumpTable, LineRow, LocalVar, Module, Strings,
};
use crate::policy::IntTy;
use crate::types::{FuncType, TypeDef, ValType};

/// Why a function or module could not be built.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{BuildError, ModuleBuilder};
///
/// let mut m = ModuleBuilder::new();
/// let mut f = m.function("f", &[], &[]);
/// let nowhere = f.label();
/// f.jmp(nowhere); // never bound
/// let err = m.add_function(f).unwrap_err();
/// assert!(matches!(err, BuildError::UnboundLabel { .. }));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum BuildError {
    /// A branch, table, handler, or local uses a label that was never bound.
    UnboundLabel {
        /// The function.
        func: FuncId,
        /// The label's number within the function.
        label: u32,
    },
    /// A label was bound twice.
    LabelRebound {
        /// The function.
        func: FuncId,
        /// The label's number within the function.
        label: u32,
    },
    /// A label made by another function's builder was used.
    ForeignLabel {
        /// The function whose builder saw the label.
        func: FuncId,
    },
    /// A branch target is not an instruction of the function: an emitted
    /// instruction names a target past the end, or a label used by a branch
    /// is bound after the last instruction.
    TargetOutOfRange {
        /// The function.
        func: FuncId,
        /// The target.
        target: u32,
    },
    /// A try region or local range ends before it starts.
    InvalidRange {
        /// The function.
        func: FuncId,
    },
    /// A per-function table is full: registers, captures, name refs, or
    /// type refs (65,536 each), parameters (255), or instructions, jump
    /// tables, handlers (`u32::MAX`).
    TooMany {
        /// The function.
        func: FuncId,
        /// Which table: `"registers"`, `"captures"`, ...
        what: &'static str,
    },
    /// A register was named that the function never declared.
    UnknownRegister {
        /// The function.
        func: FuncId,
        /// The register.
        reg: Reg,
    },
    /// An instruction with `overflow = promote` writes a register declared
    /// with a static type. `promote` produces an `f64` where an integer does
    /// not fit, so its result must be `dyn` (OPS §2).
    PromoteNotDynamic {
        /// The function.
        func: FuncId,
        /// The instruction's pc.
        pc: u32,
    },
    /// Two moves of one parallel move write the same register.
    ConflictingMoves {
        /// The function.
        func: FuncId,
        /// The register written twice.
        dst: Reg,
    },
    /// A function builder does not belong to this module builder, or was
    /// already added.
    ForeignFunction(FuncId),
    /// A declared function was never added.
    UndefinedFunction(FuncId),
    /// A reserved type was never defined.
    UndefinedType(TypeId),
    /// An aggregate constant refers to itself or a later constant.
    ConstForwardRef(ConstId),
    /// A module-level table would pass `u32::MAX` entries or bytes
    /// (`what` names it).
    ModuleFull(&'static str),
    /// An encoded section would exceed 4 GiB, which its length field cannot
    /// express.
    TooLarge {
        /// The section id (see `specs/LSB.md` §6).
        section: u32,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::UnboundLabel { func, label } => {
                write!(f, "label {label} of {func} is used but never bound")
            }
            BuildError::LabelRebound { func, label } => {
                write!(f, "label {label} of {func} is bound twice")
            }
            BuildError::ForeignLabel { func } => {
                write!(f, "{func} was given a label from another function")
            }
            BuildError::TargetOutOfRange { func, target } => {
                write!(f, "branch target @{target} is past the end of {func}")
            }
            BuildError::InvalidRange { func } => {
                write!(
                    f,
                    "a try region or local range of {func} ends before it starts"
                )
            }
            BuildError::TooMany { func, what } => write!(f, "{func} has too many {what}"),
            BuildError::UnknownRegister { func, reg } => {
                write!(f, "{func} names undeclared register {reg}")
            }
            BuildError::PromoteNotDynamic { func, pc } => write!(
                f,
                "instruction {pc} of {func} uses overflow = promote but its destination is not dyn"
            ),
            BuildError::ConflictingMoves { func, dst } => {
                write!(f, "a parallel move in {func} writes {dst} twice")
            }
            BuildError::ForeignFunction(func) => {
                write!(
                    f,
                    "{func} does not belong to this module builder or was added twice"
                )
            }
            BuildError::UndefinedFunction(func) => {
                write!(f, "{func} was declared but never added")
            }
            BuildError::UndefinedType(ty) => write!(f, "{ty} was reserved but never defined"),
            BuildError::ConstForwardRef(id) => write!(
                f,
                "an aggregate constant refers to {id}, which is not an earlier constant"
            ),
            BuildError::ModuleFull(what) => write!(f, "the module's {what} table is full"),
            BuildError::TooLarge { section } => {
                write!(f, "section {section} would exceed 4 GiB")
            }
        }
    }
}

impl core::error::Error for BuildError {}

/// A position in a function's code, named before it is known.
///
/// Make one with [`FunctionBuilder::label`], use it in branches, tables,
/// handlers, and local ranges, and [`bind`](FunctionBuilder::bind) it to the
/// next instruction's position exactly once. A label belongs to the builder
/// that made it.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Inst, ModuleBuilder, Target};
///
/// let mut m = ModuleBuilder::new();
/// let mut f = m.function("f", &[], &[]);
/// let end = f.label();
/// f.jmp(end);
/// f.emit(Inst::Nop {});
/// f.bind(end);
/// f.ret_void();
/// let id = m.add_function(f).unwrap();
/// let module = m.finish().unwrap();
/// assert_eq!(module.function(id).unwrap().code()[0], Inst::Jmp { target: Target(2) });
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Label {
    func: FuncId,
    index: u32,
}

/// Builds one function. Created by [`ModuleBuilder::function`], added back
/// with [`ModuleBuilder::add_function`].
///
/// The parameters are registers `r0..rN` with the declared parameter types;
/// [`reg`](FunctionBuilder::reg) declares more, in order, so consecutive
/// calls return consecutive registers (what call windows need).
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Inst, IntOp, IntTy, ModuleBuilder, ValType};
///
/// // fn count(n: i64) -> i64 { let i = 0; while i < n { i = i + 1 } i }
/// let mut m = ModuleBuilder::new();
/// let mut f = m.function("count", &[ValType::I64], &[ValType::I64]);
/// let n = f.param(0);
/// let (i, one, more) = (f.reg(ValType::I64), f.reg(ValType::I64), f.reg(ValType::Bool));
/// let i64op = IntOp::new(IntTy::I64);
/// f.emit(Inst::LoadInt { dst: i, val: 0, ty: IntTy::I64 });
/// f.emit(Inst::LoadInt { dst: one, val: 1, ty: IntTy::I64 });
/// let (head, done) = (f.label(), f.label());
/// f.bind(head);
/// f.emit(Inst::ILt { dst: more, lhs: i, rhs: n, ty: IntTy::I64 });
/// f.jmp_if_not(more, done);
/// f.emit(Inst::IAdd { dst: i, lhs: i, rhs: one, op: i64op });
/// f.emit(Inst::Safepoint {});
/// f.jmp(head);
/// f.bind(done);
/// f.ret(i);
/// m.add_function(f).unwrap();
/// let module = m.finish().unwrap();
/// assert_eq!(module.functions()[0].code().len(), 8);
/// ```
#[derive(Clone, Debug)]
pub struct FunctionBuilder {
    id: FuncId,
    name: StrId,
    sig: TypeId,
    regs: Vec<ValType>,
    params: u16,
    captures: Vec<ValType>,
    names: Vec<StrId>,
    name_index: BTreeMap<StrId, NameRef>,
    type_refs: Vec<TypeId>,
    type_index: BTreeMap<TypeId, TypeRef>,
    code: Vec<Inst>,
    labels: Vec<Option<u32>>,
    /// Branches emitted against a label: (pc, label).
    fixups: Vec<(u32, Label)>,
    tables: Vec<(Vec<Label>, Label)>,
    handlers: Vec<(Label, Label, Label, Reg)>,
    locals: Vec<(Reg, StrId, Label, Label)>,
    lines: Vec<LineRow>,
    location: Option<(StrId, u32, u32)>,
    temps: BTreeMap<ValType, Reg>,
    error: Option<BuildError>,
}

/// The placeholder target of a branch awaiting its label; never valid, so
/// an unpatched branch could not slip through the final range check.
const UNRESOLVED: Target = Target(u32::MAX);

impl FunctionBuilder {
    fn new(id: FuncId, name: StrId, sig: TypeId, params: &[ValType]) -> Self {
        let mut f = FunctionBuilder {
            id,
            name,
            sig,
            regs: Vec::with_capacity(params.len()),
            params: 0,
            captures: Vec::new(),
            names: Vec::new(),
            name_index: BTreeMap::new(),
            type_refs: Vec::new(),
            type_index: BTreeMap::new(),
            code: Vec::new(),
            labels: Vec::new(),
            fixups: Vec::new(),
            tables: Vec::new(),
            handlers: Vec::new(),
            locals: Vec::new(),
            lines: Vec::new(),
            location: None,
            temps: BTreeMap::new(),
            error: None,
        };
        if params.len() > usize::from(u8::MAX) {
            f.fail(BuildError::TooMany {
                func: id,
                what: "parameters",
            });
        }
        for &ty in params {
            let _param = f.reg(ty);
        }
        f.params = u16::try_from(params.len()).unwrap_or(u16::MAX);
        f
    }

    /// Records the first error; later ones are consequences of it.
    fn fail(&mut self, error: BuildError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    fn too_many(&mut self, what: &'static str) {
        self.fail(BuildError::TooMany {
            func: self.id,
            what,
        });
    }

    /// The id this function will have in the module.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{FuncId, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let f = m.function("f", &[], &[]);
    /// assert_eq!(f.id(), FuncId(0));
    /// ```
    #[must_use]
    pub fn id(&self) -> FuncId {
        self.id
    }

    /// The register holding parameter `index`: parameters occupy the first
    /// registers, in order, so this is `Reg(index)`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, Reg, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let f = m.function("f", &[ValType::I64, ValType::F64], &[]);
    /// assert_eq!(f.param(1), Reg(1));
    /// assert_eq!(f.param_count(), 2);
    /// ```
    #[must_use]
    pub fn param(&self, index: u16) -> Reg {
        Reg(index)
    }

    /// The number of parameters.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// assert_eq!(m.function("f", &[ValType::Dyn], &[]).param_count(), 1);
    /// ```
    #[must_use]
    pub fn param_count(&self) -> u16 {
        self.params
    }

    /// Declares a register of type `ty` and returns it. Registers are
    /// numbered in declaration order.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, Reg, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[ValType::I64], &[]);
    /// assert_eq!(f.reg(ValType::Dyn), Reg(1));
    /// assert_eq!(f.reg(ValType::Dyn), Reg(2));
    /// ```
    pub fn reg(&mut self, ty: ValType) -> Reg {
        match u16::try_from(self.regs.len()) {
            Ok(index) => {
                self.regs.push(ty);
                Reg(index)
            }
            Err(_) => {
                self.too_many("registers");
                Reg(u16::MAX)
            }
        }
    }

    /// Declares one register per type, consecutively, and returns the
    /// first: a call window (`dst` then the arguments) in one step.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, Reg, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[], &[]);
    /// let window = f.regs(&[ValType::I64, ValType::I64, ValType::I64]);
    /// assert_eq!(window, Reg(0));
    /// assert_eq!(f.reg(ValType::Bool), Reg(3));
    /// ```
    pub fn regs(&mut self, types: &[ValType]) -> Reg {
        let first = Reg(u16::try_from(self.regs.len()).unwrap_or(u16::MAX));
        for &ty in types {
            let _reg = self.reg(ty);
        }
        first
    }

    /// Declares a captured value of type `ty` (read with
    /// [`Inst::GetUpval`]) and returns its index.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, UpvalIdx, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[], &[]);
    /// assert_eq!(f.capture(ValType::I64), UpvalIdx(0));
    /// assert_eq!(f.capture(ValType::Dyn), UpvalIdx(1));
    /// ```
    pub fn capture(&mut self, ty: ValType) -> UpvalIdx {
        match u16::try_from(self.captures.len()) {
            Ok(index) => {
                self.captures.push(ty);
                UpvalIdx(index)
            }
            Err(_) => {
                self.too_many("captures");
                UpvalIdx(u16::MAX)
            }
        }
    }

    /// The function-local [`NameRef`] for the string `name`, adding it to
    /// the function's name list on first use.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, NameRef};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let (x, y) = (m.string("x"), m.string("y"));
    /// let mut f = m.function("f", &[], &[]);
    /// assert_eq!(f.name_ref(y), NameRef(0));
    /// assert_eq!(f.name_ref(x), NameRef(1));
    /// assert_eq!(f.name_ref(y), NameRef(0));
    /// ```
    pub fn name_ref(&mut self, name: StrId) -> NameRef {
        if let Some(&r) = self.name_index.get(&name) {
            return r;
        }
        match u16::try_from(self.names.len()) {
            Ok(index) => {
                self.names.push(name);
                let r = NameRef(index);
                let _previous = self.name_index.insert(name, r);
                r
            }
            Err(_) => {
                self.too_many("name refs");
                NameRef(u16::MAX)
            }
        }
    }

    /// The function-local [`TypeRef`] for the type `ty`, adding it to the
    /// function's type list on first use.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, TypeDef, TypeRef, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let arr = m.add_type(TypeDef::Array(ValType::Dyn));
    /// let mut f = m.function("f", &[], &[]);
    /// assert_eq!(f.type_ref(arr), TypeRef(0));
    /// assert_eq!(f.type_ref(arr), TypeRef(0));
    /// ```
    pub fn type_ref(&mut self, ty: TypeId) -> TypeRef {
        if let Some(&r) = self.type_index.get(&ty) {
            return r;
        }
        match u16::try_from(self.type_refs.len()) {
            Ok(index) => {
                self.type_refs.push(ty);
                let r = TypeRef(index);
                let _previous = self.type_index.insert(ty, r);
                r
            }
            Err(_) => {
                self.too_many("type refs");
                TypeRef(u16::MAX)
            }
        }
    }

    /// Sets the source position of the instructions emitted from now on.
    /// A line-table row is added at the next instruction only if the
    /// position changed.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let file = m.string("a.mox");
    /// let mut f = m.function("f", &[], &[]);
    /// f.set_location(file, 1, 1);
    /// f.emit(Inst::Nop {});
    /// f.emit(Inst::Nop {}); // same position: no new row
    /// f.set_location(file, 2, 1);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// let pcs: Vec<u32> = module.function(id).unwrap().lines().iter().map(|r| r.pc).collect();
    /// assert_eq!(pcs, [0, 2]);
    /// ```
    pub fn set_location(&mut self, file: StrId, line: u32, column: u32) {
        self.location = Some((file, line, column));
    }

    /// The position the next instruction will have.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[], &[]);
    /// assert_eq!(f.pc(), 0);
    /// f.emit(Inst::Nop {});
    /// assert_eq!(f.pc(), 1);
    /// ```
    #[must_use]
    pub fn pc(&self) -> u32 {
        u32::try_from(self.code.len()).unwrap_or(u32::MAX)
    }

    /// Appends `inst` and returns its pc.
    ///
    /// A `jmp`, `jmp_if`, or `jmp_if_not` emitted this way carries a raw
    /// [`Target`]; it is checked against the final code length when the
    /// function is added. Prefer [`jmp`](Self::jmp) and friends, which take
    /// labels.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[], &[]);
    /// assert_eq!(f.emit(Inst::Safepoint {}), 0);
    /// assert_eq!(f.emit(Inst::RetVoid {}), 1);
    /// ```
    pub fn emit(&mut self, inst: Inst) -> u32 {
        let pc = self.pc();
        if pc == u32::MAX {
            self.too_many("instructions");
            return pc;
        }
        if let Some((file, line, column)) = self.location {
            let same = self
                .lines
                .last()
                .is_some_and(|r| (r.file, r.line, r.column) == (file, line, column));
            if !same {
                self.lines.push(LineRow {
                    pc,
                    file,
                    line,
                    column,
                });
            }
        }
        self.code.push(inst);
        pc
    }

    /// Emits `mov dst, src`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder, Reg, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[ValType::I64], &[]);
    /// let copy = f.reg(ValType::I64);
    /// f.mov(copy, Reg(0));
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().code()[0], Inst::Mov { dst: copy, src: Reg(0) });
    /// ```
    pub fn mov(&mut self, dst: Reg, src: Reg) -> u32 {
        self.emit(Inst::Mov { dst, src })
    }

    /// Emits `ret src`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[ValType::Dyn], &[ValType::Dyn]);
    /// let x = f.param(0);
    /// assert_eq!(f.ret(x), 0);
    /// ```
    pub fn ret(&mut self, src: Reg) -> u32 {
        self.emit(Inst::Ret { src })
    }

    /// Emits `ret_void`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[], &[]);
    /// assert_eq!(f.ret_void(), 0);
    /// ```
    pub fn ret_void(&mut self) -> u32 {
        self.emit(Inst::RetVoid {})
    }

    /// A new, unbound label.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[], &[]);
    /// let (a, b) = (f.label(), f.label());
    /// assert_ne!(a, b);
    /// ```
    pub fn label(&mut self) -> Label {
        let index = match u32::try_from(self.labels.len()) {
            Ok(index) => index,
            Err(_) => {
                self.too_many("labels");
                u32::MAX
            }
        };
        self.labels.push(None);
        Label {
            func: self.id,
            index,
        }
    }

    /// Whether `label` was made by this builder; records an error if not.
    fn owns(&mut self, label: Label) -> bool {
        let ok = label.func == self.id && (label.index as usize) < self.labels.len();
        if !ok {
            self.fail(BuildError::ForeignLabel { func: self.id });
        }
        ok
    }

    /// Binds `label` to the position of the next instruction emitted.
    /// Binding a label twice is an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{BuildError, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[], &[]);
    /// let l = f.label();
    /// f.bind(l);
    /// f.bind(l);
    /// f.ret_void();
    /// assert!(matches!(m.add_function(f), Err(BuildError::LabelRebound { .. })));
    /// ```
    pub fn bind(&mut self, label: Label) {
        if !self.owns(label) {
            return;
        }
        let pc = self.pc();
        match self.labels.get_mut(label.index as usize) {
            Some(slot @ None) => *slot = Some(pc),
            _ => self.fail(BuildError::LabelRebound {
                func: self.id,
                label: label.index,
            }),
        }
    }

    fn branch(&mut self, inst: Inst, label: Label) -> u32 {
        let owned = self.owns(label);
        let pc = self.emit(inst);
        if owned {
            self.fixups.push((pc, label));
        }
        pc
    }

    /// Emits `jmp label`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder, Target};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[], &[]);
    /// let top = f.label();
    /// f.bind(top);
    /// f.emit(Inst::Safepoint {});
    /// f.jmp(top);
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().code()[1], Inst::Jmp { target: Target(0) });
    /// ```
    pub fn jmp(&mut self, label: Label) -> u32 {
        self.branch(Inst::Jmp { target: UNRESOLVED }, label)
    }

    /// Emits `jmp_if cond, label`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder, Target, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[ValType::Bool], &[]);
    /// let c = f.param(0);
    /// let out = f.label();
    /// f.jmp_if(c, out);
    /// f.bind(out);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().code()[0], Inst::JmpIf { cond: c, target: Target(1) });
    /// ```
    pub fn jmp_if(&mut self, cond: Reg, label: Label) -> u32 {
        self.branch(
            Inst::JmpIf {
                cond,
                target: UNRESOLVED,
            },
            label,
        )
    }

    /// Emits `jmp_if_not cond, label`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder, Target, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[ValType::Bool], &[]);
    /// let c = f.param(0);
    /// let out = f.label();
    /// f.jmp_if_not(c, out);
    /// f.bind(out);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().code()[0], Inst::JmpIfNot { cond: c, target: Target(1) });
    /// ```
    pub fn jmp_if_not(&mut self, cond: Reg, label: Label) -> u32 {
        self.branch(
            Inst::JmpIfNot {
                cond,
                target: UNRESOLVED,
            },
            label,
        )
    }

    /// Emits `switch.ty src, jtN` with a new jump table: `targets[v]` for a
    /// selector `v` in range, `default` otherwise. Returns the table's id.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntTy, ModuleBuilder, TableId, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[ValType::U8], &[]);
    /// let sel = f.param(0);
    /// let (zero, other) = (f.label(), f.label());
    /// assert_eq!(f.switch(IntTy::U8, sel, &[zero], other), TableId(0));
    /// f.bind(zero);
    /// f.bind(other);
    /// f.ret_void();
    /// assert!(m.add_function(f).is_ok());
    /// ```
    pub fn switch(&mut self, ty: IntTy, src: Reg, targets: &[Label], default: Label) -> TableId {
        let all_owned = targets.iter().all(|&l| self.owns(l)) & self.owns(default);
        let table = match u32::try_from(self.tables.len()) {
            Ok(index) => TableId(index),
            Err(_) => {
                self.too_many("jump tables");
                TableId(u32::MAX)
            }
        };
        if all_owned {
            self.tables.push((targets.to_vec(), default));
        }
        let _pc = self.emit(Inst::Switch { src, table, ty });
        table
    }

    /// Declares a try region: an error raised at a pc in `start..end`
    /// continues at `handler` with the error value in `catch` (a `dyn`
    /// register). Declare inner regions before the regions that enclose
    /// them; the first matching region wins.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder, Reg, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[ValType::Dyn], &[]);
    /// let err = f.reg(ValType::Dyn);
    /// let (start, end, handler) = (f.label(), f.label(), f.label());
    /// f.bind(start);
    /// f.emit(Inst::Throw { src: Reg(0) });
    /// f.bind(end);
    /// f.bind(handler);
    /// f.ret_void();
    /// f.try_region(start, end, handler, err);
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().handlers()[0].catch, err);
    /// ```
    pub fn try_region(&mut self, start: Label, end: Label, handler: Label, catch: Reg) {
        if self.owns(start) & self.owns(end) & self.owns(handler) {
            self.handlers.push((start, end, handler, catch));
        }
    }

    /// Names the register `reg` as the source variable `name` over the
    /// range `start..end`, for debuggers.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let name = m.string("count");
    /// let mut f = m.function("f", &[ValType::I64], &[]);
    /// let (s, e) = (f.label(), f.label());
    /// f.bind(s);
    /// f.ret_void();
    /// f.bind(e);
    /// f.local(f.param(0), name, s, e);
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.function(id).unwrap().locals()[0].name, name);
    /// ```
    pub fn local(&mut self, reg: Reg, name: StrId, start: Label, end: Label) {
        if self.owns(start) & self.owns(end) {
            self.locals.push((reg, name, start, end));
        }
    }

    /// A temporary of type `ty` for breaking move cycles, guaranteed not to
    /// be any register in `named` (the registers the current move set
    /// mentions). One temporary per type is cached and reused, since it is
    /// only live inside one move sequence; if a move set names the cached
    /// one (or names undeclared registers a fresh one would collide with),
    /// fresh registers are declared until one is free. That terminates
    /// within `named.len() + 1` declarations.
    fn temp(&mut self, ty: ValType, named: &BTreeSet<Reg>) -> Reg {
        if let Some(&r) = self.temps.get(&ty) {
            if !named.contains(&r) {
                return r;
            }
        }
        loop {
            let r = self.reg(ty);
            if self.error.is_some() || !named.contains(&r) {
                let _previous = self.temps.insert(ty, r);
                return r;
            }
        }
    }

    /// Emits moves that copy every `src` to its `dst` **simultaneously**:
    /// each destination ends up with the value its source held before any
    /// move ran, even when sources and destinations overlap or form cycles
    /// (`(a, b), (b, a)` swaps). Moves whose source is their destination
    /// are dropped. Cycles are broken through a temporary register of the
    /// saved register's type. Two moves writing one register are an error.
    ///
    /// This is the correct lowering of block arguments (the bytecode form of
    /// SSA phis): a back edge that permutes its own parameters must not be
    /// emitted as plain sequential moves.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Inst, ModuleBuilder, Reg, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("swap", &[ValType::I64, ValType::I64], &[]);
    /// let (a, b) = (f.param(0), f.param(1));
    /// f.parallel_move(&[(a, b), (b, a)]);
    /// f.ret_void();
    /// let id = m.add_function(f).unwrap();
    /// let module = m.finish().unwrap();
    /// let tmp = Reg(2); // allocated to break the cycle
    /// assert_eq!(
    ///     &module.function(id).unwrap().code()[..3],
    ///     &[
    ///         Inst::Mov { dst: tmp, src: a },
    ///         Inst::Mov { dst: a, src: b },
    ///         Inst::Mov { dst: b, src: tmp },
    ///     ],
    /// );
    /// ```
    pub fn parallel_move(&mut self, moves: &[(Reg, Reg)]) {
        // Pending moves (dst, src), self-moves dropped.
        let mut pending: Vec<(Reg, Reg)> = moves.iter().copied().filter(|(d, s)| d != s).collect();
        // Every register the move set mentions: a temporary must be none of them.
        let named: BTreeSet<Reg> = moves.iter().flat_map(|&(d, s)| [d, s]).collect();
        let n = pending.len();
        // dst -> index of the move writing it (destinations are unique).
        let mut writer: BTreeMap<Reg, usize> = BTreeMap::new();
        for (i, &(dst, _)) in pending.iter().enumerate() {
            if writer.insert(dst, i).is_some() {
                self.fail(BuildError::ConflictingMoves { func: self.id, dst });
                return;
            }
        }
        // src -> moves reading it, and how many of those are still pending.
        let mut readers: BTreeMap<Reg, Vec<usize>> = BTreeMap::new();
        for (i, &(_, src)) in pending.iter().enumerate() {
            readers.entry(src).or_default().push(i);
        }
        let mut live_readers: BTreeMap<Reg, usize> =
            readers.iter().map(|(&r, v)| (r, v.len())).collect();
        let mut done = alloc::vec![false; n];
        // A move is ready when nothing pending still needs its destination.
        let mut ready: Vec<usize> = (0..n)
            .filter(|&i| {
                pending
                    .get(i)
                    .is_some_and(|&(d, _)| !live_readers.contains_key(&d))
            })
            .collect();
        let mut remaining = n;
        let mut scan = 0usize;
        while remaining > 0 {
            while let Some(i) = ready.pop() {
                let Some(&(dst, src)) = pending.get(i) else {
                    continue;
                };
                let _pc = self.emit(Inst::Mov { dst, src });
                if let Some(flag) = done.get_mut(i) {
                    *flag = true;
                }
                remaining -= 1;
                // `src` lost a reader; if none are left and `src` is itself
                // a pending destination, that move may now run.
                if let Some(count) = live_readers.get_mut(&src) {
                    *count -= 1;
                    if *count == 0 {
                        let _gone = live_readers.remove(&src);
                        if let Some(&w) = writer.get(&src) {
                            if !done.get(w).copied().unwrap_or(true) {
                                ready.push(w);
                            }
                        }
                    }
                }
            }
            if remaining == 0 {
                break;
            }
            // Every pending move now lies on a cycle (each destination still
            // has exactly one pending reader). Save one destination in a
            // temporary and point its reader at the temporary; the cycle
            // becomes a chain and drains through `ready`.
            while done.get(scan).copied().unwrap_or(false) {
                scan += 1;
            }
            let Some(&(saved, _)) = pending.get(scan) else {
                return;
            };
            let Some(&ty) = self.regs.get(saved.index()) else {
                self.fail(BuildError::UnknownRegister {
                    func: self.id,
                    reg: saved,
                });
                return;
            };
            let tmp = self.temp(ty, &named);
            let _pc = self.emit(Inst::Mov {
                dst: tmp,
                src: saved,
            });
            if let Some(list) = readers.get(&saved) {
                for &j in list {
                    if !done.get(j).copied().unwrap_or(true) {
                        if let Some(m) = pending.get_mut(j) {
                            m.1 = tmp;
                        }
                    }
                }
            }
            let _gone = live_readers.remove(&saved);
            ready.push(scan);
        }
    }

    /// Resolves labels and produces the function.
    fn finish(mut self) -> Result<(FuncId, Function), BuildError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let func = self.id;
        let len = self.pc();
        let labels = core::mem::take(&mut self.labels);
        let resolve =
            |label: Label| -> Result<u32, BuildError> {
                labels.get(label.index as usize).copied().flatten().ok_or(
                    BuildError::UnboundLabel {
                        func,
                        label: label.index,
                    },
                )
            };
        let in_code = |pc: u32| -> Result<Target, BuildError> {
            if pc < len {
                Ok(Target(pc))
            } else {
                Err(BuildError::TargetOutOfRange { func, target: pc })
            }
        };

        for &(pc, label) in &self.fixups {
            let target = in_code(resolve(label)?)?;
            if let Some(inst) = self.code.get_mut(pc as usize) {
                *inst = match *inst {
                    Inst::Jmp { .. } => Inst::Jmp { target },
                    Inst::JmpIf { cond, .. } => Inst::JmpIf { cond, target },
                    Inst::JmpIfNot { cond, .. } => Inst::JmpIfNot { cond, target },
                    other => other,
                };
            }
        }
        // One pass over the code, one dispatch per instruction:
        // - every branch, label-built or emitted raw, must land on an
        //   instruction (this is the guarantee; the fix-ups above only fill
        //   the placeholders);
        // - `overflow = promote` yields an f64 where an integer does not fit,
        //   so it is only valid with a `dyn` destination. Where the
        //   destination's declared type is known here, enforce that; an
        //   undeclared register is left to the verifier.
        for (pc, inst) in self.code.iter().enumerate() {
            let (target, promote) = inst.build_checks();
            if let Some(target) = target {
                let _target = in_code(target.0)?;
            }
            if promote {
                let declared = inst.dst().and_then(|d| self.regs.get(d.index()));
                if declared.is_some_and(|&ty| ty != ValType::Dyn) {
                    let pc = u32::try_from(pc).unwrap_or(u32::MAX);
                    return Err(BuildError::PromoteNotDynamic { func, pc });
                }
            }
        }

        let mut tables = Vec::with_capacity(self.tables.len());
        for (targets, default) in &self.tables {
            let mut resolved = Vec::with_capacity(targets.len());
            for &l in targets {
                resolved.push(in_code(resolve(l)?)?);
            }
            tables.push(JumpTable {
                targets: resolved,
                default: in_code(resolve(*default)?)?,
            });
        }

        let mut handlers = Vec::with_capacity(self.handlers.len());
        for &(start, end, handler, catch) in &self.handlers {
            let (start, end) = (resolve(start)?, resolve(end)?);
            if start > end {
                return Err(BuildError::InvalidRange { func });
            }
            let target = in_code(resolve(handler)?)?;
            handlers.push(Handler {
                start,
                end,
                target,
                catch,
            });
        }

        let mut locals = Vec::with_capacity(self.locals.len());
        for &(reg, name, start, end) in &self.locals {
            let (start, end) = (resolve(start)?, resolve(end)?);
            if start > end {
                return Err(BuildError::InvalidRange { func });
            }
            locals.push(LocalVar {
                reg,
                name,
                start,
                end,
            });
        }

        Ok((
            func,
            Function {
                name: self.name,
                sig: self.sig,
                regs: self.regs,
                captures: self.captures,
                names: self.names,
                type_refs: self.type_refs,
                tables,
                handlers,
                code: self.code,
                lines: self.lines,
                locals,
            },
        ))
    }
}

/// Builds a [`Module`].
///
/// Strings, structural types (everything but structs), and constants are
/// deduplicated. Ids are dense and assigned in creation order. Functions are
/// declared with [`function`](Self::function), which fixes their
/// [`FuncId`] up front so they can call each other in any order, and are
/// added with [`add_function`](Self::add_function).
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Const, ExportItem, Inst, ModuleBuilder, ValType};
///
/// let mut m = ModuleBuilder::new();
/// let greeting = m.string("hello");
/// let k = m.constant(Const::Str(greeting));
/// let mut f = m.function("greet", &[], &[ValType::Str]);
/// let s = f.reg(ValType::Str);
/// f.emit(Inst::LoadConst { dst: s, k });
/// f.ret(s);
/// let greet = m.add_function(f).unwrap();
/// m.export("greet", ExportItem::Func(greet));
/// let module = m.finish().unwrap();
/// assert_eq!(module.string(greeting), Some("hello"));
/// ```
#[derive(Clone, Debug, Default)]
pub struct ModuleBuilder {
    strings: Strings,
    string_index: BTreeMap<Box<str>, StrId>,
    types: Vec<Option<TypeDef>>,
    type_index: BTreeMap<TypeDef, TypeId>,
    consts: Vec<Const>,
    const_index: BTreeMap<Const, ConstId>,
    imports: Vec<Import>,
    globals: Vec<Global>,
    functions: Vec<Option<Function>>,
    /// The (name, signature) each declared function slot was created with.
    declared: Vec<(StrId, TypeId)>,
    exports: Vec<Export>,
    hooks: BTreeMap<Hook, Callee>,
    name: Option<StrId>,
    start: Option<FuncId>,
    error: Option<BuildError>,
}

/// The next dense id for a table of `len` entries, or `None` when the
/// table is full.
fn next_id(len: usize) -> Option<u32> {
    u32::try_from(len).ok().filter(|&id| id != u32::MAX)
}

impl ModuleBuilder {
    /// An empty module builder.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// let module = ModuleBuilder::new().finish().unwrap();
    /// assert!(module.functions().is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn fail(&mut self, error: BuildError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    /// The id of the string `s`, adding it on first use.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, StrId};
    ///
    /// let mut m = ModuleBuilder::new();
    /// assert_eq!(m.string("a"), StrId(0));
    /// assert_eq!(m.string("b"), StrId(1));
    /// assert_eq!(m.string("a"), StrId(0));
    /// ```
    pub fn string(&mut self, s: &str) -> StrId {
        if let Some(&id) = self.string_index.get(s) {
            return id;
        }
        let Some(id) = next_id(self.strings.len()) else {
            self.fail(BuildError::ModuleFull("string"));
            return StrId(u32::MAX);
        };
        if !self.strings.push(s) {
            self.fail(BuildError::ModuleFull("string"));
            return StrId(u32::MAX);
        }
        let _previous = self.string_index.insert(Box::from(s), StrId(id));
        StrId(id)
    }

    /// Adds a type and returns its id. Structural types (functions, arrays,
    /// maps, cells, iterators) are deduplicated; every struct is a new type.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, StructDef, TypeDef, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let a = m.add_type(TypeDef::Array(ValType::I64));
    /// assert_eq!(m.add_type(TypeDef::Array(ValType::I64)), a);
    /// let s1 = m.add_type(TypeDef::Struct(StructDef::default()));
    /// let s2 = m.add_type(TypeDef::Struct(StructDef::default()));
    /// assert_ne!(s1, s2);
    /// ```
    pub fn add_type(&mut self, def: TypeDef) -> TypeId {
        if def.is_structural() {
            if let Some(&id) = self.type_index.get(&def) {
                return id;
            }
        }
        let Some(id) = next_id(self.types.len()) else {
            self.fail(BuildError::ModuleFull("type"));
            return TypeId(u32::MAX);
        };
        if def.is_structural() {
            let _previous = self.type_index.insert(def.clone(), TypeId(id));
        }
        self.types.push(Some(def));
        TypeId(id)
    }

    /// Reserves a type id to be defined later with
    /// [`define_type`](Self::define_type): how a struct refers to itself
    /// (a list node's `next` field) or two structs refer to each other.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Field, ModuleBuilder, StructDef, TypeDef, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let node = m.reserve_type();
    /// let (name, next) = (m.string("Node"), m.string("next"));
    /// m.define_type(node, TypeDef::Struct(StructDef {
    ///     name,
    ///     parent: None,
    ///     fields: vec![Field { name: next, ty: ValType::Ref(node) }],
    ///     methods: vec![],
    /// }));
    /// assert!(m.finish().is_ok());
    /// ```
    pub fn reserve_type(&mut self) -> TypeId {
        let Some(id) = next_id(self.types.len()) else {
            self.fail(BuildError::ModuleFull("type"));
            return TypeId(u32::MAX);
        };
        self.types.push(None);
        TypeId(id)
    }

    /// Defines a type reserved with [`reserve_type`](Self::reserve_type).
    /// Defining an id that was not reserved, or defining it twice, is
    /// recorded as [`UndefinedType`](BuildError::UndefinedType).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, TypeDef, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let t = m.reserve_type();
    /// m.define_type(t, TypeDef::Cell(ValType::Dyn));
    /// assert_eq!(m.finish().unwrap().type_def(t), Some(&TypeDef::Cell(ValType::Dyn)));
    /// ```
    pub fn define_type(&mut self, id: TypeId, def: TypeDef) {
        match self.types.get_mut(id.index()) {
            Some(slot @ None) => *slot = Some(def),
            _ => self.fail(BuildError::UndefinedType(id)),
        }
    }

    /// The id of the function type `(params) -> (results)`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let a = m.func_type(&[ValType::Dyn], &[ValType::Dyn]);
    /// assert_eq!(m.func_type(&[ValType::Dyn], &[ValType::Dyn]), a);
    /// ```
    pub fn func_type(&mut self, params: &[ValType], results: &[ValType]) -> TypeId {
        self.add_type(TypeDef::Func(FuncType {
            params: params.to_vec(),
            results: results.to_vec(),
        }))
    }

    /// Adds a constant and returns its id; equal constants share one id.
    /// An aggregate may only refer to constants that already exist
    /// (otherwise [`ConstForwardRef`](BuildError::ConstForwardRef)).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Const, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let one = m.constant(Const::Int(1));
    /// let two = m.constant(Const::Int(2));
    /// let list = m.constant(Const::Array(vec![one, two, one]));
    /// assert_eq!(m.constant(Const::Int(1)), one);
    /// assert_eq!(m.finish().unwrap().consts().len(), 3);
    /// # let _ = list;
    /// ```
    pub fn constant(&mut self, c: Const) -> ConstId {
        if let Some(&id) = self.const_index.get(&c) {
            return id;
        }
        let len = self.consts.len();
        if let Some(bad) = c.children().find(|child| child.index() >= len) {
            self.fail(BuildError::ConstForwardRef(bad));
            return ConstId(u32::MAX);
        }
        let Some(id) = next_id(len) else {
            self.fail(BuildError::ModuleFull("constant"));
            return ConstId(u32::MAX);
        };
        let _previous = self.const_index.insert(c.clone(), ConstId(id));
        self.consts.push(c);
        ConstId(id)
    }

    /// Adds a host-function import.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ImportId, ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let sig = m.func_type(&[ValType::Str], &[]);
    /// assert_eq!(m.import("ls.io", "print", sig), ImportId(0));
    /// ```
    pub fn import(&mut self, module: &str, name: &str, sig: TypeId) -> ImportId {
        let (module, name) = (self.string(module), self.string(name));
        let Some(id) = next_id(self.imports.len()) else {
            self.fail(BuildError::ModuleFull("import"));
            return ImportId(u32::MAX);
        };
        self.imports.push(Import { module, name, sig });
        ImportId(id)
    }

    /// Adds a global.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Const, GlobalId, ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let zero = m.constant(Const::Int(0));
    /// assert_eq!(m.global("hits", ValType::I64, true, Some(zero)), GlobalId(0));
    /// ```
    pub fn global(
        &mut self,
        name: &str,
        ty: ValType,
        mutable: bool,
        init: Option<ConstId>,
    ) -> GlobalId {
        let name = self.string(name);
        let Some(id) = next_id(self.globals.len()) else {
            self.fail(BuildError::ModuleFull("global"));
            return GlobalId(u32::MAX);
        };
        self.globals.push(Global {
            name,
            ty,
            mutable,
            init,
        });
        GlobalId(id)
    }

    /// Declares a function and returns its builder. The function's id is
    /// fixed now ([`FunctionBuilder::id`]), so other functions can call it
    /// before it is added.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{FuncId, ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let even = m.function("even", &[ValType::I64], &[ValType::Bool]);
    /// let odd = m.function("odd", &[ValType::I64], &[ValType::Bool]);
    /// assert_eq!((even.id(), odd.id()), (FuncId(0), FuncId(1)));
    /// ```
    pub fn function(
        &mut self,
        name: &str,
        params: &[ValType],
        results: &[ValType],
    ) -> FunctionBuilder {
        let name = self.string(name);
        let sig = self.func_type(params, results);
        let id = match next_id(self.functions.len()) {
            Some(id) => {
                self.functions.push(None);
                self.declared.push((name, sig));
                FuncId(id)
            }
            None => {
                self.fail(BuildError::ModuleFull("function"));
                FuncId(u32::MAX)
            }
        };
        FunctionBuilder::new(id, name, sig, params)
    }

    /// Resolves the function's labels and places it in its slot.
    ///
    /// # Errors
    ///
    /// The first error the function builder recorded, an unbound label, a
    /// branch target past the end, an inverted range, or
    /// [`ForeignFunction`](BuildError::ForeignFunction) if the builder was
    /// not declared by this module builder or was already added.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{BuildError, Inst, ModuleBuilder, Target};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("f", &[], &[]);
    /// f.emit(Inst::Jmp { target: Target(5) }); // raw target past the end
    /// assert_eq!(
    ///     m.add_function(f),
    ///     Err(BuildError::TargetOutOfRange { func: bytecode_lang::FuncId(0), target: 5 }),
    /// );
    /// ```
    pub fn add_function(&mut self, f: FunctionBuilder) -> Result<FuncId, BuildError> {
        let id = f.id;
        let declared = self.declared.get(id.index()) == Some(&(f.name, f.sig));
        let empty = matches!(self.functions.get(id.index()), Some(None));
        if !declared || !empty {
            return Err(BuildError::ForeignFunction(id));
        }
        let (id, function) = f.finish()?;
        if let Some(slot) = self.functions.get_mut(id.index()) {
            *slot = Some(function);
        }
        Ok(id)
    }

    /// Exports `item` under `name`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ExportItem, ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let g = m.global("version", ValType::I32, false, None);
    /// m.export("version", ExportItem::Global(g));
    /// assert_eq!(m.finish().unwrap().exports().len(), 1);
    /// ```
    pub fn export(&mut self, name: &str, item: ExportItem) {
        let name = self.string(name);
        if next_id(self.exports.len()).is_none() {
            self.fail(BuildError::ModuleFull("export"));
            return;
        }
        self.exports.push(Export { name, item });
    }

    /// Binds a dynamic-operation hook, replacing any earlier binding.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Callee, Hook, ModuleBuilder, ValType};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let sig = m.func_type(&[ValType::Dyn, ValType::Dyn], &[ValType::Dyn]);
    /// let loose_eq = m.import("mox.rt", "loose_eq", sig);
    /// m.hook(Hook::Eq, Callee::Import(loose_eq));
    /// let module = m.finish().unwrap();
    /// assert_eq!(module.hook(Hook::Eq), Some(Callee::Import(loose_eq)));
    /// ```
    pub fn hook(&mut self, hook: Hook, callee: Callee) {
        let _previous = self.hooks.insert(hook, callee);
    }

    /// Names the module (the name the loader links imports against).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ModuleBuilder;
    ///
    /// let mut m = ModuleBuilder::new();
    /// m.set_name("app");
    /// assert!(m.finish().unwrap().name().is_some());
    /// ```
    pub fn set_name(&mut self, name: &str) {
        self.name = Some(self.string(name));
    }

    /// Sets the initializer the loader runs after binding imports.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{FuncId, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let mut f = m.function("init", &[], &[]);
    /// f.ret_void();
    /// let init = m.add_function(f).unwrap();
    /// m.set_start(init);
    /// assert_eq!(m.finish().unwrap().start(), Some(FuncId(0)));
    /// ```
    pub fn set_start(&mut self, func: FuncId) {
        self.start = Some(func);
    }

    /// Produces the module.
    ///
    /// # Errors
    ///
    /// The first error recorded while building, a declared function never
    /// added ([`UndefinedFunction`](BuildError::UndefinedFunction)), a
    /// reserved type never defined ([`UndefinedType`](BuildError::UndefinedType)),
    /// or a section too large to encode ([`TooLarge`](BuildError::TooLarge)).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{BuildError, FuncId, ModuleBuilder};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let _forgotten = m.function("f", &[], &[]);
    /// assert_eq!(m.finish().unwrap_err(), BuildError::UndefinedFunction(FuncId(0)));
    /// ```
    pub fn finish(self) -> Result<Module, BuildError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let mut functions = Vec::with_capacity(self.functions.len());
        for (i, f) in self.functions.into_iter().enumerate() {
            let id = FuncId(u32::try_from(i).unwrap_or(u32::MAX));
            functions.push(f.ok_or(BuildError::UndefinedFunction(id))?);
        }
        let mut types = Vec::with_capacity(self.types.len());
        for (i, t) in self.types.into_iter().enumerate() {
            let id = TypeId(u32::try_from(i).unwrap_or(u32::MAX));
            types.push(t.ok_or(BuildError::UndefinedType(id))?);
        }
        let module = Module {
            strings: self.strings,
            types,
            consts: self.consts,
            imports: self.imports,
            globals: self.globals,
            functions,
            exports: self.exports,
            hooks: self
                .hooks
                .into_iter()
                .map(|(hook, callee)| HookBinding { hook, callee })
                .collect(),
            name: self.name,
            start: self.start,
        };
        for (section, size) in (1u32..).zip(section_sizes(&module)) {
            if size > u64::from(u32::MAX) {
                return Err(BuildError::TooLarge { section });
            }
        }
        Ok(module)
    }
}
