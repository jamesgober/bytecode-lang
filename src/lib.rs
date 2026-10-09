//! # bytecode_lang
//!
//! LSB, the LexerSketch bytecode format: one register-machine instruction
//! set for every language and every execution tier, its module model, a
//! versioned binary encoding, a budgeted decoder, a deterministic
//! disassembler, and a builder that resolves branch labels.
//!
//! The VM (bvm-lang 2.0) executes LSB, code generators (codegen-lang 2.0)
//! emit it, program images (loader-lang) store it, and the debugger
//! (dap-lang) reads its line tables. This crate owns the format only; it
//! executes nothing.
//!
//! ## Quick start
//!
//! Build a function, encode the module, decode it back, and read the listing:
//!
//! ```
//! use bytecode_lang::{decode, disassemble, encode, Inst, IntOp, IntTy, ModuleBuilder, ValType};
//!
//! let mut m = ModuleBuilder::new();
//! let mut f = m.function("add", &[ValType::I64, ValType::I64], &[ValType::I64]);
//! let (a, b) = (f.param(0), f.param(1));
//! let sum = f.reg(ValType::I64);
//! f.emit(Inst::IAdd { dst: sum, lhs: a, rhs: b, op: IntOp::new(IntTy::I64) });
//! f.ret(sum);
//! m.add_function(f)?;
//! let module = m.finish()?;
//!
//! let bytes = encode(&module);
//! let back = decode(&bytes)?;
//! assert_eq!(back, module);
//! assert!(disassemble(&back).contains("iadd.i64 r2, r0, r1"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## The model
//!
//! - **Registers, typed.** Every function has a frame of 64-bit registers,
//!   each with a declared [`ValType`]: unboxed scalars for static languages,
//!   [`ValType::Dyn`] for dynamic values, and references. Declared types make
//!   GC roots exact and let the verifier check every operand.
//! - **Eight-byte instructions.** [`Inst`] is a `Copy` enum of eight bytes;
//!   a function body is a flat `&[Inst]` that an interpreter walks without
//!   re-parsing. Branch targets are resolved instruction indices.
//! - **Policies on every operation.** Integer instructions carry an
//!   [`IntOp`] (the operand type plus the overflow, division-by-zero, and
//!   shift policies), dynamic ones a [`Policy`], float-to-integer
//!   conversions a [`FloatConv`], so every tier computes the same result or
//!   raises the same [`ErrorKind`].
//! - **Dynamic languages built in.** `dyn` arithmetic and comparisons with a
//!   numeric fast path, ordered hash maps, property access, iteration, and
//!   per-module [`Hook`]s that let a language supply its own semantics for
//!   the slow path. `overflow = promote` gives PHP's int-to-float arithmetic,
//!   `shift = saturate` its shifts.
//! - **PHP's value and reference semantics.** `dsep_index` separates a nested
//!   array only when it may be shared; references into map slots and
//!   properties (`dref_index`, `dbind_index`) are transparent in their
//!   slots and survive copies as PHP's do.
//! - **Dynamic calls with real signatures.** A function or import may carry a
//!   [`ParamList`] (names, rest parameters, defaults, by-reference
//!   parameters); `dcall_shape` passes named and spread arguments through a
//!   [`CallShape`], and [`ParamList::bind`] is the one binding rule every
//!   tier applies.
//! - **Coroutines.** Stackful coroutines (`coro_new`, `yield`, `await`,
//!   `resume`, `spawn`) for generators, fibers, and async tasks; see
//!   [`CoroState`].
//!
//! The normative description of every instruction is `specs/LSB.md` in the
//! LexerSketch plan; `docs/API.md` documents every public item.
//!
//! ## Guarantees
//!
//! - [`decode`] never panics on any input, never recurses, and allocates no
//!   more than its [`Limits`] and the input length justify.
//! - `decode(&encode(&m)) == Ok(m)` for every module, and
//!   `encode(&decode(bytes)?) == bytes` for every accepted input: each module
//!   has exactly one encoding.
//! - [`encode`] and [`disassemble`] are deterministic.
//! - A function produced by [`ModuleBuilder::add_function`] has no unresolved
//!   or out-of-range branch target.
//!
//! What this release does **not** do: verify a module's meaning (index
//! ranges, register types, frame shapes). That is the v0.5 verifier, whose
//! rules are specified in `specs/LSB.md` §8. Until then a decoded module is
//! well-formed but unverified.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(unused_must_use)]
#![deny(unused_results)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::todo)]
#![deny(clippy::unimplemented)]
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
#![deny(clippy::dbg_macro)]

extern crate alloc;

#[macro_use]
mod macros;

mod builder;
mod call;
mod decode;
mod disasm;
mod encode;
mod ids;
mod inst;
mod module;
mod policy;
mod types;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub use builder::{BuildError, FunctionBuilder, Label, ModuleBuilder};
pub use call::{
    ArgItem, ArgKind, BindError, Binding, Bound, CallShape, Param, ParamError, ParamKind,
    ParamList, ShapeError,
};
pub use decode::{DecodeError, DecodeErrorKind, Limit, Limits};
pub use ids::{
    ConstId, FieldIdx, FuncId, GlobalId, ImportId, NameRef, Reg, ShapeId, StrId, TableId, Target,
    TypeId, TypeRef, UpvalIdx,
};
pub use inst::{FieldKind, FieldSpec, Inst, InstError, Opcode, Slot};
pub use module::{
    Callee, Const, ErrorKind, Export, ExportItem, Function, Global, Handler, Hook, HookBinding,
    Import, JumpTable, LineRow, LocalVar, Module,
};
pub use policy::{
    DivZero, FloatConv, FloatToInt, FloatTy, IntConv, IntOp, IntPair, IntTy, Overflow, Policy,
    Shift,
};
pub use types::{CoroState, Field, FuncType, Kind, Method, Prim, StructDef, TypeDef, ValType};

/// The four bytes every encoded module starts with: `LSB\0`.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{encode, ModuleBuilder, MAGIC};
///
/// let bytes = encode(&ModuleBuilder::new().finish().unwrap());
/// assert_eq!(&bytes[..4], &MAGIC);
/// ```
pub const MAGIC: [u8; 4] = *b"LSB\0";

/// The format version this crate writes and the only one it reads.
///
/// The version changes whenever the encoding or any instruction's meaning
/// changes; [`decode`] refuses every other version rather than guess.
/// Version 2 (bytecode-lang 0.3) added parameter lists and call shapes to
/// the function and import records, repacked the policy bytes for
/// `shift = saturate`, and added the reference, separation, `pow`, `abs`,
/// shaped-call, and `raise` instructions (`specs/LSB.md` §7.4).
///
/// # Examples
///
/// ```
/// use bytecode_lang::{encode, ModuleBuilder, FORMAT_VERSION};
///
/// let bytes = encode(&ModuleBuilder::new().finish().unwrap());
/// assert_eq!(bytes[4..8], FORMAT_VERSION.to_le_bytes());
/// assert_eq!(FORMAT_VERSION, 2);
/// ```
pub const FORMAT_VERSION: u32 = 2;

/// Encodes a module into its canonical bytes.
///
/// Infallible and deterministic: a [`Module`] can only be made by
/// [`ModuleBuilder::finish`] or [`decode`], both of which guarantee every
/// section fits its 32-bit length.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{encode, ModuleBuilder};
///
/// let module = ModuleBuilder::new().finish().unwrap();
/// let bytes = encode(&module);
/// assert_eq!(bytes, encode(&module));
/// // A 12-byte header, ten 8-byte section headers, and the empty sections'
/// // counts and option flags.
/// assert_eq!(bytes.len(), 130);
/// ```
#[must_use]
pub fn encode(module: &Module) -> Vec<u8> {
    encode::encode(module)
}

/// Decodes a module with the default [`Limits`].
///
/// # Errors
///
/// A [`DecodeError`] with the byte offset and the reason when the input is
/// not a well-formed module of this format version.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{decode, encode, DecodeErrorKind, ModuleBuilder};
///
/// let module = ModuleBuilder::new().finish().unwrap();
/// let bytes = encode(&module);
/// assert_eq!(decode(&bytes).unwrap(), module);
///
/// let truncated = &bytes[..bytes.len() - 1];
/// assert_eq!(decode(truncated).unwrap_err().kind(), &DecodeErrorKind::UnexpectedEnd);
/// ```
pub fn decode(bytes: &[u8]) -> Result<Module, DecodeError> {
    decode::decode(bytes, &Limits::default())
}

/// Decodes a module with explicit budgets.
///
/// # Errors
///
/// As [`decode`], plus [`DecodeErrorKind::LimitExceeded`] when the input
/// exceeds one of `limits`.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{decode_with, encode, DecodeErrorKind, Limit, Limits, ModuleBuilder};
///
/// let bytes = encode(&ModuleBuilder::new().finish().unwrap());
/// let mut tight = Limits::default();
/// tight.max_bytes = 16;
/// assert_eq!(
///     decode_with(&bytes, &tight).unwrap_err().kind(),
///     &DecodeErrorKind::LimitExceeded(Limit::Bytes),
/// );
/// ```
pub fn decode_with(bytes: &[u8], limits: &Limits) -> Result<Module, DecodeError> {
    decode::decode(bytes, limits)
}

/// Compiles and runs the `rust` code blocks in `README.md` and `docs/API.md` as
/// part of `cargo test`, so the published examples cannot drift from the API.
///
/// Present only while collecting doctests (`#[cfg(doctest)]`); it is not part of
/// the public surface and does not appear in the built library or its docs.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
#[doc = include_str!("../docs/API.md")]
pub struct MarkdownDocTests;

/// The module's disassembly: a deterministic text listing of every table
/// and every function, with branch targets as labels and names resolved in
/// comments.
///
/// Never panics, including on unverified modules with out-of-range
/// references (they print as `<invalid>`).
///
/// # Examples
///
/// ```
/// use bytecode_lang::{disassemble, ModuleBuilder};
///
/// let mut m = ModuleBuilder::new();
/// let mut f = m.function("main", &[], &[]);
/// f.ret_void();
/// m.add_function(f).unwrap();
/// let text = disassemble(&m.finish().unwrap());
/// assert!(text.starts_with("lsb 2\n"));
/// assert!(text.contains("func f0 s0 \"main\" : t0"));
/// assert!(text.contains("  0000 ret_void"));
/// ```
#[must_use]
pub fn disassemble(module: &Module) -> String {
    module.to_string()
}
