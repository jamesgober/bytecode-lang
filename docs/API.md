# bytecode-lang &mdash; API Reference

> Complete reference for every public item in `bytecode-lang` 0.3.0 (LSB format version 2), with
> examples.
> **Status: pre-1.0.** The surface is designed across the 0.x series and frozen at `1.0`, after a
> VM runs Mox programs from it (LexerSketch decision D18). The normative format and instruction
> semantics are in the LexerSketch spec `specs/LSB.md`; this file documents the Rust API.

<sub>Copyright &copy; 2026 <strong>James Gober</strong>.</sub>

## Table of contents

- [Overview](#overview)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Concepts](#concepts)
  - [Registers and value types](#registers-and-value-types)
  - [The instruction word](#the-instruction-word)
  - [Window operands](#window-operands)
  - [Policies](#policies)
  - [Coroutines, generators, async](#coroutines-generators-async)
  - [Dynamic instructions and hooks](#dynamic-instructions-and-hooks)
  - [Dynamic calls: parameter lists and call shapes](#dynamic-calls-parameter-lists-and-call-shapes)
  - [Value semantics: separation and references](#value-semantics-separation-and-references)
  - [What decoding checks, and what it does not](#what-decoding-checks-and-what-it-does-not)
- [Free functions and constants](#free-functions-and-constants)
- [`ModuleBuilder`](#modulebuilder)
- [`FunctionBuilder`](#functionbuilder)
- [`Label`](#label)
- [`BuildError`](#builderror)
- [`Module`](#module)
- [`Function`](#function)
- [Module data](#module-data): `Const`, `Import`, `Global`, `Export`, `ExportItem`, `Hook`,
  `Callee`, `HookBinding`, `JumpTable`, `Handler`, `LineRow`, `LocalVar`
- [Types](#types): `ValType`, `CoroState`, `Kind`, `Prim`, `TypeDef`, `FuncType`, `StructDef`, `Field`, `Method`
- [Dynamic-call signatures](#dynamic-call-signatures): `ParamList`, `Param`, `ParamKind`,
  `ParamError`, `CallShape`, `ArgKind`, `ShapeError`, `ArgItem`, `Bound`, `Binding`, `BindError`
- [Policies and modifiers](#policies-and-modifiers): `IntTy`, `FloatTy`, `Overflow`, `DivZero`,
  `Shift`, `FloatToInt`, `Policy`, `IntOp`, `FloatConv`, `IntConv`, `IntPair`
- [Instructions](#instructions): `Inst`, `Opcode`, `FieldSpec`, `FieldKind`, `Slot`, `InstError`
- [Index types](#index-types)
- [Decoding](#decoding): `Limits`, `Limit`, `DecodeError`, `DecodeErrorKind`
- [`ErrorKind`](#errorkind)
- [Instruction table](#instruction-table)
- [Feature flags](#feature-flags)
- [Stability](#stability)

## Overview

`bytecode-lang` defines LSB, the single bytecode format of the LexerSketch toolchain. A
[`Module`](#module) holds tables (strings, types, constants, imports, globals, exports, hooks) and
functions; a function is a typed register frame plus a flat array of eight-byte
[`Inst`](#inst)ructions. [`encode`](#encode) turns a module into canonical bytes,
[`decode`](#decode) turns bytes back into a module under explicit budgets,
[`disassemble`](#disassemble) prints it, and [`ModuleBuilder`](#modulebuilder) builds one with
label-resolved branches.

| Item | Kind | Purpose |
|---|---|---|
| [`encode`](#encode), [`decode`](#decode), [`decode_with`](#decode_with), [`disassemble`](#disassemble) | functions | The Tier-1 entry points. |
| [`ModuleBuilder`](#modulebuilder), [`FunctionBuilder`](#functionbuilder), [`Label`](#label) | structs | Build modules; resolve branch labels; parallel moves. |
| [`Module`](#module), [`Function`](#function) | structs | The read-only module model. |
| [`Inst`](#inst), [`Opcode`](#opcode) | enums | The instruction set and its metadata. |
| [`ValType`](#valtype), [`TypeDef`](#typedef-functype-structdef-field-method) | enums | Register types and the type table. |
| [`IntOp`](#policies-and-modifiers), [`Policy`](#policies-and-modifiers), [`FloatConv`](#policies-and-modifiers) | structs | Integer types plus OPS policies, carried per instruction. |
| [`ParamList`](#paramlist), [`CallShape`](#callshape-argkind-shapeerror) | structs | Dynamic-call signatures and call-site layouts; [`ParamList::bind`](#paramlistbind) is the binding rule every tier shares. |
| [`Limits`](#limits) | struct | Decoding budgets. |
| [`BuildError`](#builderror), [`DecodeError`](#decodeerror-decodeerrorkind), [`InstError`](#insterror) | errors | Why building or decoding failed. |
| [`ErrorKind`](#errorkind) | enum | Runtime error kinds and their stable codes, shared by every tier. |

## Installation

```toml
[dependencies]
bytecode-lang = "0.3"
```

The crate is `no_std` (it needs `alloc`), has no dependencies, and forbids `unsafe`:

```toml
[dependencies]
bytecode-lang = { version = "0.3", default-features = false }
```

## Quick start

```rust
use bytecode_lang::{decode, disassemble, encode, Inst, IntTy, ModuleBuilder, ValType};

let mut m = ModuleBuilder::new();
let mut f = m.function("max", &[ValType::I64, ValType::I64], &[ValType::I64]);
let (a, b) = (f.param(0), f.param(1));
let less = f.reg(ValType::Bool);
let take_b = f.label();
f.emit(Inst::ILt { dst: less, lhs: a, rhs: b, ty: IntTy::I64 });
f.jmp_if(less, take_b);
f.ret(a);
f.bind(take_b);
f.ret(b);
m.add_function(f).unwrap();
let module = m.finish().unwrap();

let bytes = encode(&module);
assert_eq!(decode(&bytes).unwrap(), module);
let text = disassemble(&module);
assert!(text.contains("jmp_if r2, L0"));
assert!(text.contains("L0:\n  0003 ret r1"));
```

## Concepts

### Registers and value types

Every function has a frame of 64-bit registers. Each register has a declared
[`ValType`](#valtype): unboxed scalars (`bool`, the eight integer types, `f32`, `f64`, `char`),
`str` (an immutable byte string), `dyn` (a dynamically typed value), or `ref t` (an object of
type-table entry `t`). `str`, `ref`, and `dyn` share one reference representation and `nil` is the
null reference. Parameters occupy the first registers. Declared types make garbage-collection roots
exact and let the verifier (v0.5) check every operand without dataflow inference.

### The instruction word

```text
byte    0        1       2..4    4..6    6..8
        opcode   A       B       C       D
                                 \------W------/
```

`A` is a byte (type, policy, kind, count, flag), `B`/`C`/`D` are 16-bit (registers and
per-function refs), `W` is 32-bit (module indices, branch targets, immediates). Unused bytes must be
zero. The decoded [`Inst`](#inst) is also eight bytes: a function body is executed as a flat
`&[Inst]`.

### Window operands

Calls read their arguments from the registers after their destination (`dst+1 ..= dst+argc`) and
write the result to `dst`; `make_closure` reads its captures the same way; `str_concat_n` reads
`count` consecutive parts; `str_slice` reads its end index from `range+1`.
[`FunctionBuilder::regs`](#functionbuilderregs) allocates consecutive registers.

### Policies

Each integer instruction carries an [`IntOp`](#policies-and-modifiers): its operand type and the
policies integer arithmetic consults (overflow, division by zero, shift range, `saturate`
included). Dynamic arithmetic carries a [`Policy`](#policies-and-modifiers) (the complete OPS set,
float-to-int included) for its integer path, and `f32_to_int`/`f64_to_int` carry a
[`FloatConv`](#policies-and-modifiers) (type and float-to-int policy). Every tier computes exactly
OPS's result under that policy, so `iadd.i64.wrap` and `iadd.i64` are different instructions.
`shift = saturate` is PHP's shift (an amount at or above the width gives 0, or -1 for `>>` of a
negative value; a negative amount is still an error). `overflow = promote` (OPS §2, for PHP) produces the nearest `f64` when an
integer result does not fit; it is valid only where the result register is `dyn`, so the builder
refuses it on a declared static destination ([`BuildError::PromoteNotDynamic`](#builderror)) and
the verifier (v0.5) refuses it everywhere else.

### Coroutines, generators, async

Coroutines are stackful and asymmetric. `coro_new` makes one from a function and arguments;
`resume` runs it until it executes `yield`/`yield_kv` (a generator value, with an automatic or
explicit key) or `await` (an awaitable for the scheduler), or returns; `resume_throw` resumes it by
raising an error at its suspension point; `coro_close` closes it, running its `finally` blocks;
`coro_key` and `coro_result` read its last key and its return value; `coro_status` reads its
[`CoroState`](#corostate); `spawn` hands a new coroutine to the host scheduler through the `spawn`
[hook](#module-data). Values crossing a suspension are `dyn`. A `yield` without a key uses PHP's
generator rule: one more than the largest integer key yielded so far, never below 0 (after only
`yield -5 => x` the next automatic key is 0; format version 1 said -4). bvm-lang 2.0 executes the
coroutine instructions; this crate defines, encodes, decodes, and disassembles them. The full
model, including what a suspended coroutine owns and how the GC traces it, is `specs/LSB.md` §5.13.

```rust
use bytecode_lang::{decode, encode, Inst, ModuleBuilder, ValType};

let mut m = ModuleBuilder::new();
// fn numbers(first) { yield first; return nil }
let mut body = m.function("numbers", &[ValType::Dyn], &[ValType::Dyn]);
let sent = body.reg(ValType::Dyn);
body.emit(Inst::Yield { dst: sent, src: body.param(0) });
body.emit(Inst::LoadNil { dst: sent });
body.ret(sent);
let numbers = m.add_function(body).unwrap();

let mut f = m.function("main", &[ValType::Dyn], &[ValType::Dyn]);
let window = f.regs(&[ValType::Dyn, ValType::Dyn]); // the coroutine, then its argument
let got = f.reg(ValType::Dyn);
f.mov(bytecode_lang::Reg(window.0 + 1), f.param(0));
f.emit(Inst::CoroNew { dst: window, func: numbers, argc: 1 });
f.emit(Inst::Resume { dst: got, coro: window, src: got });
f.ret(got);
m.add_function(f).unwrap();
let module = m.finish().unwrap();
assert_eq!(decode(&encode(&module)).unwrap(), module);
```

### Dynamic instructions and hooks

`dadd`, `deq`, `get_prop`, `dcall`, and the other `d*` instructions work on `dyn` registers. Numbers
take a built-in fast path; every other combination calls the module's [`Hook`](#module-data) for
that operation, through which a language supplies its own semantics (PHP's loose `==`, Python's
list equality), or raises `TypeError` when none is bound. `dpow` and `dabs` (with their `pow` and
`abs` hooks) complete the OPS v2 operations on dynamic values. The names of the two "not"s follow
HIR: `dnot` is the logical not (PHP `!`), `dbit_not` the bitwise complement (PHP `~`), and
`ibit_not` the typed complement.

A `dyn` float holds one canonical NaN (`0x7FF8_0000_0000_0000`), so NaN sign and payload are not
observable on dynamic values: unboxing and `float_to_bits` always see that NaN, and `ftotal_cmp`
orders it as a positive NaN (`specs/LSB.md` §5.6).

### Dynamic calls: parameter lists and call shapes

A function or an import may carry a [`ParamList`](#paramlist): names, kinds (positional-only,
normal, named-only, rest, rest map, named rest), by-reference flags, and defaults, with a trailing
`i64` presence mask in the signature when a parameter has a default. A `dcall_shape` call site
names a [`CallShape`](#callshape-argkind-shapeerror) saying which window registers are positional,
named, or spread. The VM binds one to the other by one rule, which
[`ParamList::bind`](#paramlistbind) implements; `dcall` is the all-positional case.
`dparam_ref`/`dparam_ref_named` tell a PHP code generator, at run time, whether to send a reference
for an argument. `load_import` makes any import a first-class function value.

```rust
use bytecode_lang::{
    ArgKind, decode, disassemble, encode, Inst, ModuleBuilder, Param, ParamKind, ParamList, Prim,
    Reg, ValType,
};

let mut m = ModuleBuilder::new();
let (format, flag) = (m.string("format"), m.string("flag"));
let d = ValType::Dyn;
// The host's printf(string $format, mixed ...$values), usable as a value.
let sig = m.func_type(&[d, d], &[d]);
let printf = m.import_with_params(
    "php.std",
    "printf",
    sig,
    ParamList::new(vec![Param::normal(format), Param::new(ParamKind::RestMap, None)]),
);
// $f(...$args, flag: true), with $f = 'printf' resolved to the host function
let mut f = m.function("call", &[d], &[d]);
let (callee, yes) = (f.reg(d), f.reg(ValType::Bool));
let window = f.regs(&[d, d, d]); // result, the spread, the named argument
f.emit(Inst::LoadImport { dst: callee, import: printf });
f.mov(Reg(window.0 + 1), f.param(0));
f.emit(Inst::LoadBool { dst: yes, val: true });
f.emit(Inst::ToDyn { dst: Reg(window.0 + 2), src: yes, from: Prim::Bool });
f.dcall_shape(window, callee, &[ArgKind::Spread, ArgKind::Named(flag)]);
f.ret(window);
let id = m.add_function(f).unwrap();
let module = m.finish().unwrap();
assert_eq!(decode(&encode(&module)).unwrap(), module);
assert_eq!(module.function(id).unwrap().shapes()[0].args, [ArgKind::Spread, ArgKind::Named(flag)]);
assert!(disassemble(&module).contains("params (s0, ...map _)"));
```

### Value semantics: separation and references

PHP arrays are values: a code generator `dup`s a container on every transfer (O(1), copy-on-write).
For a nested write (`$a[$k][] = $v`), `dsep_index` makes `$a[$k]` safe to write in place, copying
it only when another container may share it; `dsep_prop` does the same for a property. References
(PHP `&`) are boxes of kind `reference`: `new_ref` makes one, `dref_index`/`dref_prop` make a map
slot or property into one (`&$a['k']`, `foreach ($a as &$v)`), `dbind_index`/`dbind_prop` bind a
slot to an existing one (`$a['k'] = &$x`), and `dunref_index`/`dunref_prop` unbind it. A slot
holding a reference is transparent: reads and writes go through to the reference's value, and
copies of the container share the reference, as in PHP. `cell_get`/`cell_set` read and write a
reference (`specs/LSB.md` §5.16–5.17).

```rust
use bytecode_lang::{disassemble, Inst, ModuleBuilder, Policy, ValType};

// function push(&$a, $k, $v) { $a[$k][] = $v; return $a[$k]; }  (the core of it)
let d = ValType::Dyn;
let mut m = ModuleBuilder::new();
let mut f = m.function("push", &[d, d, d], &[d]);
let (a, k, v) = (f.param(0), f.param(1), f.param(2));
let (arr, inner, slot) = (f.reg(d), f.reg(d), f.reg(d));
f.emit(Inst::CellGet { dst: arr, cell: a }); // $a is a reference
f.emit(Inst::DSepIndex { dst: inner, obj: arr, key: k }); // $a[$k], unshared
f.emit(Inst::DRefIndex { dst: slot, obj: inner, key: v }); // &$a[$k][$v]
f.emit(Inst::DAbs { dst: v, src: v, pol: Policy::new() });
f.ret(inner);
m.add_function(f).unwrap();
let text = disassemble(&m.finish().unwrap());
assert!(text.contains("dsep_index r4, r3, r1"));
assert!(text.contains("dref_index r5, r4, r2"));
```

### What decoding checks, and what it does not

[`decode`](#decode) checks the **format**: header, version, section order and lengths, every tag,
UTF-8, `char` validity, canonical instruction words, constant references pointing backwards, the
constant nesting depth, and every count against [`Limits`](#limits) and the remaining bytes. It
does **not** check **meaning**: whether an index is in range for its table or whether registers are
used at their declared types. That is the verifier's job (v0.5). Every accessor that follows an
index returns `Option`, and [`disassemble`](#disassemble) prints out-of-range references as
`<invalid>`, so an unverified module is safe to inspect.

## Free functions and constants

### `encode`

```rust,ignore
pub fn encode(module: &Module) -> Vec<u8>
```

Encodes a module into its canonical bytes. Infallible and deterministic: equal modules give equal
bytes, and `decode(&encode(&m)) == Ok(m)` for every module.

```rust
use bytecode_lang::{encode, ModuleBuilder};

let module = ModuleBuilder::new().finish().unwrap();
assert_eq!(encode(&module), encode(&module));
assert_eq!(encode(&module).len(), 130); // the empty module
```

### `decode`

```rust,ignore
pub fn decode(bytes: &[u8]) -> Result<Module, DecodeError>
```

Decodes with the default [`Limits`](#limits). Never panics on any input. Every accepted input is
canonical: `encode(&decode(bytes)?) == bytes`.

**Errors:** [`DecodeError`](#decodeerror-decodeerrorkind), with the byte offset and the reason.

```rust
use bytecode_lang::{decode, encode, DecodeErrorKind, ModuleBuilder};

let bytes = encode(&ModuleBuilder::new().finish().unwrap());
assert!(decode(&bytes).is_ok());
let mut bad = bytes.clone();
bad.push(0);
assert_eq!(decode(&bad).unwrap_err().kind(), &DecodeErrorKind::TrailingBytes);
```

### `decode_with`

```rust,ignore
pub fn decode_with(bytes: &[u8], limits: &Limits) -> Result<Module, DecodeError>
```

Decodes with explicit budgets. **Errors:** as [`decode`](#decode), plus
`DecodeErrorKind::LimitExceeded(limit)` when the input exceeds one.

```rust
use bytecode_lang::{decode_with, encode, DecodeErrorKind, Limit, Limits, ModuleBuilder};

let mut m = ModuleBuilder::new();
for name in ["a", "b", "c"] {
    let mut f = m.function(name, &[], &[]);
    f.ret_void();
    m.add_function(f).unwrap();
}
let bytes = encode(&m.finish().unwrap());
let mut limits = Limits::default();
limits.max_functions = 2;
assert_eq!(
    decode_with(&bytes, &limits).unwrap_err().kind(),
    &DecodeErrorKind::LimitExceeded(Limit::Functions),
);
```

### `disassemble`

```rust,ignore
pub fn disassemble(module: &Module) -> String
```

A deterministic text listing: header, every table, then every function with its register types,
tables, handlers, locals, line rows, and code. Branch targets become labels (`L0`), modifiers
become mnemonic suffixes (`iadd.i64.wrap`), and names and constant previews appear as `;`
comments. Equal to `module.to_string()`. Never panics; invalid references print `<invalid>`.

```rust
use bytecode_lang::{disassemble, Inst, ModuleBuilder};

let mut m = ModuleBuilder::new();
let mut f = m.function("spin", &[], &[]);
let top = f.label();
f.bind(top);
f.emit(Inst::Safepoint {});
f.jmp(top);
m.add_function(f).unwrap();
let text = disassemble(&m.finish().unwrap());
assert!(text.contains("L0:\n  0000 safepoint\n  0001 jmp L0\n"));
```

### `MAGIC`, `FORMAT_VERSION`

```rust,ignore
pub const MAGIC: [u8; 4] = *b"LSB\0";
pub const FORMAT_VERSION: u32 = 2;
```

Every encoded module starts with `MAGIC` followed by `FORMAT_VERSION` (little-endian). The decoder
accepts exactly this version; a version 1 file (bytecode-lang 0.2) is refused with
`UnsupportedVersion(1)` and must be regenerated (`specs/LSB.md` §7.4 lists the changes).

```rust
use bytecode_lang::{encode, ModuleBuilder, FORMAT_VERSION, MAGIC};

let bytes = encode(&ModuleBuilder::new().finish().unwrap());
assert_eq!(bytes[..4], MAGIC);
assert_eq!(bytes[4..8], FORMAT_VERSION.to_le_bytes());
```

## `ModuleBuilder`

Builds a [`Module`](#module). Strings, structural types (everything but structs), and constants are
deduplicated; ids are dense, in creation order. Methods that cannot fail on their own never return
`Result`; a limit hit (a table passing `u32::MAX`) is recorded and reported by
[`finish`](#modulebuilderfinish). `Clone`, `Debug`, `Default`.

### `ModuleBuilder::new`

```rust,ignore
pub fn new() -> ModuleBuilder
```

An empty builder.

```rust
let module = bytecode_lang::ModuleBuilder::new().finish().unwrap();
assert!(module.functions().is_empty());
```

### `ModuleBuilder::string`

```rust,ignore
pub fn string(&mut self, s: &str) -> StrId
```

The id of `s` in the string table, adding it on first use.

```rust
let mut m = bytecode_lang::ModuleBuilder::new();
assert_eq!(m.string("x"), m.string("x"));
```

### `ModuleBuilder::add_type`

```rust,ignore
pub fn add_type(&mut self, def: TypeDef) -> TypeId
```

Adds a type. Structural types are deduplicated; every struct is a new type.

```rust
use bytecode_lang::{ModuleBuilder, TypeDef, ValType};

let mut m = ModuleBuilder::new();
let a = m.add_type(TypeDef::Map { key: ValType::Dyn, value: ValType::Dyn });
assert_eq!(m.add_type(TypeDef::Map { key: ValType::Dyn, value: ValType::Dyn }), a);
```

### `ModuleBuilder::reserve_type`

```rust,ignore
pub fn reserve_type(&mut self) -> TypeId
```

Reserves an id to define later with [`define_type`](#modulebuilderdefine_type): how a struct
refers to itself or two structs to each other. An id never defined makes `finish` fail with
`BuildError::UndefinedType`.

### `ModuleBuilder::define_type`

```rust,ignore
pub fn define_type(&mut self, id: TypeId, def: TypeDef)
```

Defines a reserved type. Defining an unreserved id, or one twice, is recorded as
`BuildError::UndefinedType(id)`.

```rust
use bytecode_lang::{Field, ModuleBuilder, StructDef, TypeDef, ValType};

let mut m = ModuleBuilder::new();
let node = m.reserve_type();
let (name, next) = (m.string("Node"), m.string("next"));
m.define_type(node, TypeDef::Struct(StructDef {
    name,
    parent: None,
    fields: vec![Field { name: next, ty: ValType::Ref(node) }],
    methods: vec![],
}));
assert!(m.finish().is_ok());
```

### `ModuleBuilder::func_type`

```rust,ignore
pub fn func_type(&mut self, params: &[ValType], results: &[ValType]) -> TypeId
```

The id of the function type `(params) -> (results)` (deduplicated).

```rust
use bytecode_lang::{ModuleBuilder, ValType};

let mut m = ModuleBuilder::new();
assert_eq!(m.func_type(&[ValType::Dyn], &[]), m.func_type(&[ValType::Dyn], &[]));
```

### `ModuleBuilder::constant`

```rust,ignore
pub fn constant(&mut self, c: Const) -> ConstId
```

Adds a constant (deduplicated). An aggregate may refer only to constants that already exist;
otherwise `finish` fails with `BuildError::ConstForwardRef`. Floats deduplicate by bit pattern.

```rust
use bytecode_lang::{Const, ModuleBuilder};

let mut m = ModuleBuilder::new();
let one = m.constant(Const::Int(1));
let pair = m.constant(Const::Array(vec![one, one]));
assert_ne!(one, pair);
assert_ne!(m.constant(Const::f64(0.0)), m.constant(Const::f64(-0.0)));
```

### `ModuleBuilder::import`

```rust,ignore
pub fn import(&mut self, module: &str, name: &str, sig: TypeId) -> ImportId
```

Adds a host function, called with `call_import` and bound by the loader by `module` and `name`.

```rust
use bytecode_lang::{ModuleBuilder, ValType};

let mut m = ModuleBuilder::new();
let sig = m.func_type(&[ValType::Str], &[]);
let print = m.import("ls.io", "print", sig);
assert_eq!(m.finish().unwrap().import(print).unwrap().sig, sig);
```

### `ModuleBuilder::import_with_params`

```rust,ignore
pub fn import_with_params(&mut self, module: &str, name: &str, sig: TypeId, params: ParamList) -> ImportId
```

Adds a host function with a dynamic-call signature, so it can be a first-class value taking named,
spread, extra, missing, and by-reference arguments through `dcall`/`dcall_shape`. `sig` must
already be a function type of this builder; the list is checked by
[`ParamList::validate`](#paramlist) and [`ParamList::fits`](#paramlist) against it.

**Errors** (reported by [`finish`](#modulebuilderfinish)): `InvalidParams { owner: Callee::Import(id), error }`.

```rust
use bytecode_lang::{BuildError, Callee, ModuleBuilder, Param, ParamError, ParamList, ValType};

let mut m = ModuleBuilder::new();
let x = m.string("x");
let sig = m.func_type(&[ValType::Dyn], &[]); // no room for the presence mask
let list = ParamList::new(vec![Param::normal(x).with_default()]);
let id = m.import_with_params("host", "f", sig, list);
assert_eq!(
    m.finish().unwrap_err(),
    BuildError::InvalidParams { owner: Callee::Import(id), error: ParamError::Signature { index: 1 } },
);
```

### `ModuleBuilder::global`

```rust,ignore
pub fn global(&mut self, name: &str, ty: ValType, mutable: bool, init: Option<ConstId>) -> GlobalId
```

Adds a global. `init: None` means the type's default.

```rust
use bytecode_lang::{ModuleBuilder, ValType};

let mut m = ModuleBuilder::new();
let g = m.global("hits", ValType::I64, true, None);
assert!(m.finish().unwrap().global(g).unwrap().mutable);
```

### `ModuleBuilder::function`

```rust,ignore
pub fn function(&mut self, name: &str, params: &[ValType], results: &[ValType]) -> FunctionBuilder
```

Declares a function and returns its builder. The [`FuncId`](#index-types) is fixed now
([`FunctionBuilder::id`](#functionbuilderid-param-param_count)), so functions can call each other
before they are added. A declared function that is never added makes `finish` fail.

### `ModuleBuilder::add_function`

```rust,ignore
pub fn add_function(&mut self, f: FunctionBuilder) -> Result<FuncId, BuildError>
```

Resolves the function's labels, checks every branch target against the final code length, and
places it in its slot.

**Errors:** the first error the builder recorded; `UnboundLabel`; `TargetOutOfRange` (a raw target
past the end, or a branch to a label bound after the last instruction); `InvalidRange`;
`InvalidParams` (the parameter list breaks its rules or does not fit the signature);
`InvalidShape` (a malformed call shape); `PromoteNotDynamic`; `ForeignFunction` (not declared by
this builder).

```rust
use bytecode_lang::{BuildError, FuncId, Inst, ModuleBuilder, Target};

let mut m = ModuleBuilder::new();
let mut f = m.function("f", &[], &[]);
f.emit(Inst::Jmp { target: Target(3) });
assert_eq!(m.add_function(f), Err(BuildError::TargetOutOfRange { func: FuncId(0), target: 3 }));
```

### `ModuleBuilder::export`

```rust,ignore
pub fn export(&mut self, name: &str, item: ExportItem)
```

Exports a function, global, or type under `name`.

### `ModuleBuilder::hook`

```rust,ignore
pub fn hook(&mut self, hook: Hook, callee: Callee)
```

Binds a dynamic-operation hook, replacing an earlier binding. Bindings are stored sorted by hook.

```rust
use bytecode_lang::{Callee, Hook, ModuleBuilder, ValType};

let mut m = ModuleBuilder::new();
let sig = m.func_type(&[ValType::Dyn, ValType::Dyn], &[ValType::Dyn]);
let eq = m.import("mox.rt", "loose_eq", sig);
m.hook(Hook::Eq, Callee::Import(eq));
assert_eq!(m.finish().unwrap().hook(Hook::Eq), Some(Callee::Import(eq)));
```

### `ModuleBuilder::set_name`, `ModuleBuilder::set_start`

```rust,ignore
pub fn set_name(&mut self, name: &str)
pub fn set_start(&mut self, func: FuncId)
```

The module's name (what the loader links imports against) and its initializer.

### `ModuleBuilder::finish`

```rust,ignore
pub fn finish(self) -> Result<Module, BuildError>
```

**Errors:** the first recorded error; `UndefinedFunction` (declared, never added); `UndefinedType`
(reserved, never defined); `TooLarge` (a section over 4 GiB, which a 32-bit length cannot express).

```rust
use bytecode_lang::{BuildError, FuncId, ModuleBuilder};

let mut m = ModuleBuilder::new();
let _f = m.function("forgotten", &[], &[]);
assert_eq!(m.finish().unwrap_err(), BuildError::UndefinedFunction(FuncId(0)));
```

## `FunctionBuilder`

Builds one function. Created by [`ModuleBuilder::function`](#modulebuilderfunction), returned with
[`add_function`](#modulebuilderadd_function). Methods never fail on the spot; the first problem is
recorded and reported by `add_function`. `Clone`, `Debug`.

### `FunctionBuilder::id`, `param`, `param_count`

```rust,ignore
pub fn id(&self) -> FuncId
pub fn param(&self, index: u16) -> Reg
pub fn param_count(&self) -> u16
```

The function's id, the register of parameter `index` (`Reg(index)`), and the parameter count.

### `FunctionBuilder::reg`

```rust,ignore
pub fn reg(&mut self, ty: ValType) -> Reg
```

Declares a register; registers are numbered in order. The 65,537th records
`BuildError::TooMany { what: "registers" }`.

### `FunctionBuilder::regs`

```rust,ignore
pub fn regs(&mut self, types: &[ValType]) -> Reg
```

Declares consecutive registers and returns the first: a call window in one step.

```rust
use bytecode_lang::{Inst, IntTy, ModuleBuilder, Reg, ValType};

let mut m = ModuleBuilder::new();
let mut callee = m.function("id", &[ValType::I64], &[ValType::I64]);
let x = callee.param(0);
callee.ret(x);
let id = m.add_function(callee).unwrap();

let mut f = m.function("main", &[], &[ValType::I64]);
let window = f.regs(&[ValType::I64, ValType::I64]); // result, then the argument
f.emit(Inst::LoadInt { dst: Reg(window.0 + 1), val: 41, ty: IntTy::I64 });
f.emit(Inst::Call { dst: window, func: id, argc: 1 });
f.ret(window);
assert!(m.add_function(f).is_ok());
```

### `FunctionBuilder::capture`

```rust,ignore
pub fn capture(&mut self, ty: ValType) -> UpvalIdx
```

Declares a captured value (read with `get_upval`), for functions used as closures.

### `FunctionBuilder::name_ref`, `type_ref`

```rust,ignore
pub fn name_ref(&mut self, name: StrId) -> NameRef
pub fn type_ref(&mut self, ty: TypeId) -> TypeRef
```

The function-local reference for a module string or type, added on first use (deduplicated). Used
by `get_prop`/`set_prop`/`has_prop` and by `is_type`, `cast`, `new_struct`, `new_array`, `new_map`,
`new_cell`.

```rust
use bytecode_lang::{Inst, ModuleBuilder, ValType};

let mut m = ModuleBuilder::new();
let x = m.string("x");
let mut f = m.function("get_x", &[ValType::Dyn], &[ValType::Dyn]);
let obj = f.param(0);
let out = f.reg(ValType::Dyn);
let name = f.name_ref(x);
f.emit(Inst::GetProp { dst: out, obj, name });
f.ret(out);
assert!(m.add_function(f).is_ok());
```

### `FunctionBuilder::set_params`

```rust,ignore
pub fn set_params(&mut self, list: ParamList)
```

Gives the function a dynamic-call signature ([`ParamList`](#paramlist)), checked when the function
is added: its own rules, and that it fits the signature (one parameter per entry, a trailing `i64`
presence mask when an entry has a default, rest parameters `dyn`, by-reference parameters `dyn` or
`ref`). Violations: `InvalidParams { owner: Callee::Func(id), error }`.

```rust
use bytecode_lang::{ModuleBuilder, Param, ParamKind, ParamList, ValType};

// PHP: function f($x, $y = 1, ...$rest)
let mut m = ModuleBuilder::new();
let (x, y) = (m.string("x"), m.string("y"));
let d = ValType::Dyn;
let mut f = m.function("f", &[d, d, d, ValType::I64], &[]);
f.set_params(ParamList::new(vec![
    Param::normal(x),
    Param::normal(y).with_default(),
    Param::new(ParamKind::RestMap, None),
]));
f.ret_void();
assert!(m.add_function(f).is_ok());
```

### `FunctionBuilder::call_shape`, `dcall_shape`

```rust,ignore
pub fn call_shape(&mut self, args: &[ArgKind]) -> ShapeId
pub fn dcall_shape(&mut self, dst: Reg, callee: Reg, args: &[ArgKind]) -> u32
```

`call_shape` interns a [`CallShape`](#callshape-argkind-shapeerror) in the function's shape table
(deduplicated; 65,536 at most, then `TooMany { what: "call shapes" }`); a malformed shape records
`InvalidShape`. `dcall_shape` interns the shape and emits `dcall_shape dst, callee, csN`, whose
window `dst+1 ..= dst+args.len()` holds the arguments.

```rust
use bytecode_lang::{ArgKind, ModuleBuilder, ShapeId, ValType};

let mut m = ModuleBuilder::new();
let name = m.string("name");
let mut f = m.function("f", &[ValType::Dyn], &[]);
assert_eq!(f.call_shape(&[ArgKind::Positional, ArgKind::Named(name)]), ShapeId(0));
assert_eq!(f.call_shape(&[ArgKind::Positional, ArgKind::Named(name)]), ShapeId(0));
f.ret_void();
assert!(m.add_function(f).is_ok());
```

### `FunctionBuilder::set_location`

```rust,ignore
pub fn set_location(&mut self, file: StrId, line: u32, column: u32)
```

The source position of the instructions emitted next; a line row is added when it changes.

### `FunctionBuilder::pc`, `emit`, `mov`, `ret`, `ret_void`

```rust,ignore
pub fn pc(&self) -> u32
pub fn emit(&mut self, inst: Inst) -> u32
pub fn mov(&mut self, dst: Reg, src: Reg) -> u32
pub fn ret(&mut self, src: Reg) -> u32
pub fn ret_void(&mut self) -> u32
```

`pc` is the next instruction's position; the others append an instruction and return its pc. A
raw branch emitted with `emit` is range-checked when the function is added.

### `FunctionBuilder::label`, `bind`

```rust,ignore
pub fn label(&mut self) -> Label
pub fn bind(&mut self, label: Label)
```

A new label, and binding it to the next instruction's position. Binding twice records
`LabelRebound`; a label from another function records `ForeignLabel`.

### `FunctionBuilder::jmp`, `jmp_if`, `jmp_if_not`

```rust,ignore
pub fn jmp(&mut self, label: Label) -> u32
pub fn jmp_if(&mut self, cond: Reg, label: Label) -> u32
pub fn jmp_if_not(&mut self, cond: Reg, label: Label) -> u32
```

Branches to a label, resolved by `add_function`.

### `FunctionBuilder::switch`

```rust,ignore
pub fn switch(&mut self, ty: IntTy, src: Reg, targets: &[Label], default: Label) -> TableId
```

Emits `switch` with a new jump table: `targets[v]` for a selector `v` in range, `default` otherwise.

```rust
use bytecode_lang::{IntTy, ModuleBuilder, Target, ValType};

let mut m = ModuleBuilder::new();
let mut f = m.function("f", &[ValType::U8], &[]);
let (zero, other) = (f.label(), f.label());
f.switch(IntTy::U8, f.param(0), &[zero], other);
f.bind(zero);
f.ret_void();
f.bind(other);
f.ret_void();
let id = m.add_function(f).unwrap();
let module = m.finish().unwrap();
let table = &module.function(id).unwrap().tables()[0];
assert_eq!((table.targets[0], table.default), (Target(1), Target(2)));
```

### `FunctionBuilder::try_region`

```rust,ignore
pub fn try_region(&mut self, start: Label, end: Label, handler: Label, catch: Reg)
```

An error raised at a pc in `start..end` continues at `handler` with the error value in `catch`
(`dyn`). Declare inner regions first. `start` after `end` makes `add_function` fail with
`InvalidRange`.

### `FunctionBuilder::local`

```rust,ignore
pub fn local(&mut self, reg: Reg, name: StrId, start: Label, end: Label)
```

Names a register as a source variable over `start..end`, for debuggers.

### `FunctionBuilder::parallel_move`

```rust,ignore
pub fn parallel_move(&mut self, moves: &[(Reg, Reg)])
```

Emits moves with **parallel-assignment** semantics: each destination receives the value its source
held before any of the moves, even when they overlap or form cycles. Self-moves are dropped; cycles
are broken through a temporary register of the saved register's declared type; exactly one extra
move is emitted per cycle. Two moves writing one register record `ConflictingMoves`; a cycle
through an undeclared register records `UnknownRegister`. This is the correct lowering of block
arguments (family issue H01).

```rust
use bytecode_lang::{Inst, ModuleBuilder, ValType};

let mut m = ModuleBuilder::new();
let mut f = m.function("rotate", &[ValType::I64, ValType::I64, ValType::I64], &[]);
let (a, b, c) = (f.param(0), f.param(1), f.param(2));
f.parallel_move(&[(a, b), (b, c), (c, a)]);
f.ret_void();
let id = m.add_function(f).unwrap();
let module = m.finish().unwrap();

let mut regs = [1, 2, 3, 0];
for inst in module.function(id).unwrap().code() {
    if let Inst::Mov { dst, src } = *inst {
        regs[dst.index()] = regs[src.index()];
    }
}
assert_eq!(regs[..3], [2, 3, 1]);
```

## `Label`

```rust,ignore
pub struct Label { /* private */ }
```

A position named before it is known; belongs to the builder that made it. `Clone`, `Copy`, `Eq`,
`Ord`, `Hash`, `Debug`.

## `BuildError`

```rust,ignore
#[non_exhaustive]
pub enum BuildError { /* ... */ }
```

| Variant | Meaning |
|---|---|
| `UnboundLabel { func, label }` | A label is used but never bound. |
| `LabelRebound { func, label }` | A label is bound twice. |
| `ForeignLabel { func }` | A label from another function's builder. |
| `TargetOutOfRange { func, target }` | A branch target is not an instruction of the function. |
| `InvalidRange { func }` | A try region or local range ends before it starts. |
| `TooMany { func, what }` | A per-function table is full (registers, captures, name refs, type refs, parameters, ...). |
| `UnknownRegister { func, reg }` | A move cycle passes an undeclared register. |
| `ConflictingMoves { func, dst }` | A parallel move writes one register twice. |
| `InvalidParams { owner, error }` | A function's or import's [`ParamList`](#paramlist) breaks its rules or does not fit its signature ([`ParamError`](#paramerror)). |
| `InvalidShape { func, error }` | A malformed [`CallShape`](#callshape-argkind-shapeerror) ([`ShapeError`](#callshape-argkind-shapeerror)). |
| `ForeignFunction(id)` | A function builder not declared by this module builder. |
| `UndefinedFunction(id)` | Declared, never added. |
| `UndefinedType(id)` | Reserved, never defined (or defined twice). |
| `ConstForwardRef(id)` | An aggregate constant refers to a constant that does not exist yet. |
| `ModuleFull(what)` | A module table would pass `u32::MAX` entries. |
| `TooLarge { section }` | A section would exceed 4 GiB encoded. |
| `PromoteNotDynamic { func, pc }` | An instruction with `overflow = promote` writes a register declared with a static type (`promote` yields an `f64` where an integer does not fit, so its result must be `dyn`). Undeclared destinations are left to the verifier. |

`Display` gives an actionable message; implements `core::error::Error`.

## `Module`

The read-only module. `Clone`, `PartialEq`, `Eq`, `Debug`, `Default`; `Display` is the disassembly.

| Method | Returns |
|---|---|
| `string(StrId) -> Option<&str>`, `string_count() -> usize`, `strings() -> impl Iterator<Item = &str>` | the string table |
| `types() -> &[TypeDef]`, `type_def(TypeId) -> Option<&TypeDef>` | the type table |
| `consts() -> &[Const]`, `constant(ConstId) -> Option<&Const>` | the constant pool |
| `imports() -> &[Import]`, `import(ImportId) -> Option<&Import>` | imports |
| `globals() -> &[Global]`, `global(GlobalId) -> Option<&Global>` | globals |
| `functions() -> &[Function]`, `function(FuncId) -> Option<&Function>` | functions |
| `exports() -> &[Export]` | exports |
| `hooks() -> &[HookBinding]`, `hook(Hook) -> Option<Callee>` | hook bindings, sorted |
| `name() -> Option<StrId>`, `start() -> Option<FuncId>` | meta |

```rust
use bytecode_lang::{Const, ModuleBuilder, StrId};

let mut m = ModuleBuilder::new();
let s = m.string("hi");
let k = m.constant(Const::Str(s));
let module = m.finish().unwrap();
assert_eq!(module.string(s), Some("hi"));
assert_eq!(module.constant(k), Some(&Const::Str(s)));
assert_eq!(module.string(StrId(9)), None);
```

## `Function`

One function, read-only. `Clone`, `PartialEq`, `Eq`, `Debug`.

| Method | Returns |
|---|---|
| `name() -> StrId`, `sig() -> TypeId` | name and signature |
| `params() -> Option<&ParamList>` | the dynamic-call signature, if any |
| `regs() -> &[ValType]` | register types; parameters first |
| `captures() -> &[ValType]` | captured value types |
| `names() -> &[StrId]`, `type_refs() -> &[TypeId]` | the per-function lists `NameRef`/`TypeRef` index |
| `tables() -> &[JumpTable]` | jump tables |
| `shapes() -> &[CallShape]` | call shapes, indexed by `ShapeId` |
| `handlers() -> &[Handler]` | try regions, innermost first |
| `code() -> &[Inst]` | the instructions |
| `lines() -> &[LineRow]`, `locals() -> &[LocalVar]` | debug information |

## Module data

| Type | Fields / variants | Notes |
|---|---|---|
| `Const` | `Bool(bool)`, `Int(i64)`, `UInt(u64)`, `F32(u32)`, `F64(u64)`, `Char(char)`, `Str(StrId)`, `Bytes(Vec<u8>)`, `Array(Vec<ConstId>)`, `Map(Vec<(ConstId, ConstId)>)` | floats as IEEE bits; `Const::f32(v)`, `Const::f64(v)` build them; aggregates refer to earlier constants; `Display` |
| `Import` | `module`, `name: StrId`, `sig: TypeId`, `params: Option<ParamList>` | `Clone` (no longer `Copy`, since 0.3); `load_import` makes a function value of it |
| `Global` | `name`, `ty: ValType`, `mutable: bool`, `init: Option<ConstId>` | |
| `Export` | `name: StrId`, `item: ExportItem` | |
| `ExportItem` | `Func(FuncId)`, `Global(GlobalId)`, `Type(TypeId)` | |
| `Hook` | 31 operations, codes 0–30 (`Add` … `Call`, `Spawn`, `Pow`, `Abs`, `CallShape`) | code enum: `ALL`, `from_code`, `code`, `name` |
| `Callee` | `Func(FuncId)`, `Import(ImportId)` | |
| `HookBinding` | `hook: Hook`, `callee: Callee` | |
| `JumpTable` | `targets: Vec<Target>`, `default: Target` | |
| `Handler` | `start: u32`, `end: u32`, `target: Target`, `catch: Reg` | covers `start..end` |
| `LineRow` | `pc`, `file: StrId`, `line`, `column` | |
| `LocalVar` | `reg`, `name: StrId`, `start`, `end` | |

```rust
use bytecode_lang::{Const, Hook};

assert_eq!(Const::f64(1.5).to_string(), "f64 1.5");
assert_eq!(Const::Bytes(vec![0xff]).to_string(), "bytes b\"\\xff\"");
assert_eq!(Hook::from_code(18), Some(Hook::Concat));
assert_eq!(Hook::ALL.len(), 31);
assert_eq!(Hook::Spawn.code(), 27);
assert_eq!(Hook::CallShape.code(), 30);
```

## Types

### `ValType`

`Bool`, `I8` … `U64`, `F32`, `F64`, `Char`, `Str`, `Dyn`, `Ref(TypeId)`. Methods:
`int(IntTy)`, `float(FloatTy)`, `as_int() -> Option<IntTy>`, `is_reference() -> bool` (the GC-root
test). `Display` prints `i64`, `dyn`, `ref t3`.

```rust
use bytecode_lang::{IntTy, TypeId, ValType};

assert_eq!(ValType::int(IntTy::U8), ValType::U8);
assert!(ValType::Ref(TypeId(0)).is_reference());
assert_eq!(ValType::Dyn.to_string(), "dyn");
```

### `CoroState`

The state of a coroutine as `coro_status` reports it: `Created` (0), `Running` (1), `Yielded` (2),
`Awaiting` (3), `Returned` (4), `Failed` (5). A code enum, plus `is_resumable()` (true for
`Created`, `Yielded`, `Awaiting`).

```rust
use bytecode_lang::CoroState;

assert_eq!(CoroState::Awaiting.code(), 3);
assert!(!CoroState::Returned.is_resumable());
```

### `Kind`, `Prim`

`Kind`: the 15 dynamic kinds (`Nil` = 0 … `Error` = 12, `Coroutine` = 13, `Reference` = 14),
returned by `type_of` and tested by `is_kind`. A `Reference` is PHP's `&`: a box holding one
`dyn` value, transparent in container slots (`specs/LSB.md` §5.17). `Prim`: the 14 static representations crossing the `dyn` boundary (`Bool` … `Char`,
`Str`, `Ref`), the modifier of `to_dyn`/`from_dyn`. Both are code enums (`ALL`, `from_code`,
`code`, `name`, `Display`).

### `TypeDef`, `FuncType`, `StructDef`, `Field`, `Method`

`TypeDef`: `Func(FuncType)`, `Struct(StructDef)`, `Array(ValType)`, `Map { key, value }`,
`Cell(ValType)`, `Iter { key, value }`, `Coroutine` (a coroutine handle; values crossing a
suspension are `dyn`). `FuncType { params, results }`. `StructDef { name, parent,
fields, methods }` (fields include the parent's, first). `Field { name, ty }`. `Method { name,
func }`. All `Display`.

```rust
use bytecode_lang::{FuncType, TypeDef, ValType};

let sig = TypeDef::Func(FuncType { params: vec![ValType::Dyn], results: vec![ValType::Bool] });
assert_eq!(sig.to_string(), "func (dyn) -> (bool)");
```

## Dynamic-call signatures

The binding of a dynamic call (`dcall`, `dcall_shape`) to its callee, `specs/LSB.md` §5.15.

### `ParamList`

```rust,ignore
pub struct ParamList { pub params: Vec<Param>, pub ignore_extra: bool }
```

The dynamic-call signature of a function or import. `Clone`, `Eq`, `Ord`, `Hash`, `Debug`,
`Default`, `Display` (`(&s1, s2 = ?, ...map _) ignore_extra`).

| Method | |
|---|---|
| `new(params) -> ParamList` | a list raising on extra arguments |
| `ignoring_extra(self) -> ParamList` | extra positional arguments are dropped (PHP user functions) |
| `has_defaults() -> bool` | whether the signature ends with the `i64` presence mask |
| `signature_len() -> usize` | entries, plus one for the presence mask |
| `validate() -> Result<(), ParamError>` | the list's own rules: at most 255 entries (64 with a default); kinds in order (positional-only, normal, one `Rest`/`RestMap`, named-only, one `RestNamed`); normal and named-only entries named; names unique; no default on a rest |
| `fits(&[ValType]) -> Result<(), ParamError>` | fits a signature: one parameter per entry, the trailing `i64` mask, rest parameters `dyn`, by-reference parameters `dyn` or `ref` |
| `positional_by_ref(pos: u64) -> bool` | the semantics of `dparam_ref` |
| `named_by_ref(&Module, name: &[u8]) -> bool` | the semantics of `dparam_ref_named` |
| `bind(&Module, &[ArgItem]) -> Result<Binding, BindError>` | the binding rule (below) |

### `ParamList::bind`

```rust,ignore
pub fn bind(&self, module: &Module, items: &[ArgItem<'_>]) -> Result<Binding, BindError>
```

Binds a dynamic call's arguments (after spreads were expanded into items) to the list: positional
items fill the positional-only and normal parameters in order, extras go to the rest parameter or
are dropped under `ignore_extra` or are `TooMany`; a positional item after a named one is
`PositionalAfterNamed`; a named item fills the normal or named-only parameter of that name
(`Duplicate` if filled), else the named rest, else the rest map, else `UnknownName`; an empty
parameter takes its default or is `Missing`. `module` is the callee's module (parameter names are
its strings). `O(items × params)` plus `O(n log n)` for rest-collected names. A VM raises
`ArgumentError` (E0114) for every `BindError`.

```rust
use bytecode_lang::{ArgItem, BindError, Bound, ModuleBuilder, Param, ParamList};

let mut m = ModuleBuilder::new();
let (a, b) = (m.string("a"), m.string("b"));
let module = m.finish().unwrap();
let list = ParamList::new(vec![Param::normal(a), Param::normal(b).with_default()]);
let bound = list.bind(&module, &[ArgItem::Named(b"b"), ArgItem::Named(b"a")]).unwrap();
assert_eq!(bound.slots(), &[Bound::Arg(1), Bound::Arg(0)]);
assert_eq!(bound.presence(), 0b11);
assert_eq!(
    list.bind(&module, &[ArgItem::Positional, ArgItem::Named(b"a")]),
    Err(BindError::Duplicate { item: 1 }),
);
```

### `Param`, `ParamKind`

`Param { name: Option<StrId>, kind: ParamKind, by_ref: bool, default: bool }` with `new(kind,
name)`, `normal(name)`, `by_ref()`, `with_default()`. `ParamKind` is a code enum:
`PositionalOnly` (0), `Normal` (1), `NamedOnly` (2), `Rest` (3, a `dyn` array of extra positional
arguments), `RestMap` (4, a `dyn` map of extra positional arguments under `0, 1, ...` and, without
a `RestNamed`, unknown named ones under their names), `RestNamed` (5, a `dyn` map of unknown named
arguments); `is_rest()`, `takes_names()`.

### `ParamError`

Why a list is invalid: `TooMany`, `OutOfOrder { index }`, `Unnamed { index }`,
`DuplicateName { index }`, `RestDefault { index }`, `Signature { index }`. `#[non_exhaustive]`,
`Display`, `Error`.

### `CallShape`, `ArgKind`, `ShapeError`

`CallShape { args: Vec<ArgKind> }` (`new`, `validate`, `Display` `(_, s3:, ..., **)`), one entry per
window register of a `dcall_shape`. `ArgKind`: `Positional`, `Named(StrId)`, `Spread` (an array's
elements positionally; a map's int-keyed values positionally and str-keyed ones by name),
`SpreadNamed` (a map with `str` keys only). `validate` checks at most 255 entries, no positional
entry or spread after a named entry or named spread, and no repeated name; violations are
`ShapeError::{TooMany, PositionalAfterNamed { index }, DuplicateName { index }}`
(`#[non_exhaustive]`).

### `ArgItem`, `Bound`, `Binding`, `BindError`

`ArgItem::{Positional, Named(&[u8])}` is one argument as `bind` sees it. `Binding` has `slots() ->
&[Bound]` (per parameter: `Arg(item)`, `Default`, or `Collected(Vec<item>)`) and `presence() ->
u64`. `BindError::{TooMany { item }, PositionalAfterNamed { item }, UnknownName { item },
Duplicate { item }, Missing { param }}` (`#[non_exhaustive]`, `Display`, `Error`).

## Policies and modifiers

| Type | Values / layout | Methods |
|---|---|---|
| `IntTy` | `I8 I16 I32 I64 U8 U16 U32 U64` (codes 0–7) | `bits`, `is_signed`, code enum |
| `FloatTy` | `F32`, `F64` | code enum |
| `Overflow` | `Error` (default), `Wrap`, `Trap`, `Promote` (codes 0–3) | code enum; `Promote` (OPS §2): the nearest `f64` when the integer result does not fit, valid only with a `dyn` destination |
| `DivZero` | `Error` (default), `Trap` | code enum |
| `Shift` | `Error` (default), `Mask`, `Saturate` (PHP: an amount at or above the width gives 0, or -1 for `>>` of a negative value; a negative amount is still `ShiftOutOfRange`) | code enum |
| `FloatToInt` | `Error` (default), `Saturate` | code enum |
| `Policy` | 6 bits: overflow (2), div_zero (1), shift (2; code 3 reserved), float_to_int (1): 48 valid values | `new`, `DEFAULT`, getters, `with_*`, `bits`, `from_bits`, `is_default`; `Display` lists non-defaults (`wrap.mask`, `promote.shsat`, `sat`) |
| `IntOp` | `IntTy` (3 bits) + overflow (2) + div_zero (1) + shift (2): 192 valid bytes | `new(ty)`, `with_policy` (keeps every part but `float_to_int`), `ty`, `policy` (`float_to_int` reads as `Error`), `bits`, `from_bits`; `Display` `i64.wrap` |
| `FloatConv` | `IntTy` (3 bits) + float_to_int (1): the modifier of `f32_to_int`/`f64_to_int` | `new(ty)`, `with_float_to_int`, `with_policy`, `ty`, `float_to_int`, `bits`, `from_bits`; `Display` `i32.sat` |
| `IntConv` | from, to, overflow | `new`, `from`, `to`, `overflow`, `bits`, `from_bits` |
| `IntPair` | from, to | `new`, `from`, `to`, `bits`, `from_bits` |

`from_bits` refuses patterns that name no policy, so a decoded instruction always carries a
meaningful one.

```rust
use bytecode_lang::{DivZero, IntOp, IntTy, Overflow, Policy};

let p = Policy::new().with_overflow(Overflow::Wrap).with_div_zero(DivZero::Trap);
let op = IntOp::new(IntTy::U32).with_policy(p);
assert_eq!(op.to_string(), "u32.wrap.divtrap");
assert_eq!(IntOp::from_bits(op.bits()), Some(op));
```

## Instructions

### `Inst`

```rust,ignore
pub enum Inst { Nop {}, Mov { dst: Reg, src: Reg }, /* 200 variants */ }
```

One instruction, eight bytes, `Copy`, `Eq`, `Hash`, `Debug`, `Display`. Deliberately exhaustive:
every tier must implement every instruction. Fields are listed in display order; the
[instruction table](#instruction-table) gives every variant.

| Method | |
|---|---|
| `opcode() -> Opcode` | the opcode |
| `mnemonic() -> &'static str` | the assembler name |
| `to_bytes() -> [u8; 8]` | canonical encoding |
| `from_bytes([u8; 8]) -> Result<Inst, InstError>` | strict decoding |
| `branch_target() -> Option<Target>` | the target of `jmp`/`jmp_if`/`jmp_if_not` |
| `overflow() -> Option<Overflow>` | the overflow policy carried in an `IntOp`, `Policy`, or `IntConv` modifier |

```rust
use bytecode_lang::{Inst, IntOp, IntTy, Reg};

let i = Inst::IMul { dst: Reg(0), lhs: Reg(1), rhs: Reg(2), op: IntOp::new(IntTy::I32) };
assert_eq!(Inst::from_bytes(i.to_bytes()), Ok(i));
assert_eq!(i.to_string(), "imul.i32 r0, r1, r2");
```

### `Opcode`

`#[repr(u8)]` fieldless mirror of `Inst`: `ALL`, `from_u8`, `mnemonic`, `fields() -> &[FieldSpec]`.

### `FieldSpec`, `FieldKind`, `Slot`

`FieldSpec { name, kind, slot }` describes one operand. `FieldKind` is what it is (`Reg`, `Target`,
`Const`, … `Shape`, … `IntOp`, `FloatConv`, `Kind`, `Prim`, `ErrKind`) with `is_modifier` and
`bits`. `Slot` is where it sits (`A`,
`B`, `C`, `D`, `W`) with `shift` and `width`. This metadata is enough to write an assembler, a
generator of test instructions, or a VM dispatch table.

```rust
use bytecode_lang::{FieldKind, Opcode, Slot};

let f = Opcode::Call.fields();
assert_eq!(f.iter().map(|f| f.name).collect::<Vec<_>>(), ["dst", "func", "argc"]);
assert_eq!((f[1].kind, f[1].slot), (FieldKind::Func, Slot::W));
```

### `InstError`

`UnknownOpcode(u8)`, `InvalidOperand { opcode, field }`, `NonCanonical(Opcode)`;
`#[non_exhaustive]`, `Display`, `Error`.

## Index types

`Reg(u16)` `r3` · `Target(u32)` `@3` · `ConstId(u32)` `k3` · `FuncId(u32)` `f3` · `ImportId(u32)`
`imp3` · `GlobalId(u32)` `g3` · `TypeId(u32)` `t3` · `StrId(u32)` `s3` · `NameRef(u16)` `n3` ·
`TypeRef(u16)` `ty3` · `TableId(u32)` `jt3` · `ShapeId(u16)` `cs3` · `FieldIdx(u16)` `#3` ·
`UpvalIdx(u16)` `u3`.
Public newtypes with `index() -> usize`, `Copy`, `Ord`, `Hash`, `Default`, and the `Display`
shown.

## Decoding

### `Limits`

`#[non_exhaustive]` struct with public fields; start from `Limits::default()` and assign.

| Field | Default | Bounds |
|---|---|---|
| `max_bytes` | 256 MiB | input size |
| `max_strings` | 4,194,304 | string-table entries |
| `max_types` | 1,048,576 | type-table entries |
| `max_consts` | 4,194,304 | constants |
| `max_const_depth` | 64 | aggregate constant nesting (a scalar is depth 1) |
| `max_functions` | 1,048,576 | functions |
| `max_insts` | 16,777,216 | instructions per function |
| `max_total_insts` | 33,554,432 | instructions per module |
| `max_items` | 16,777,216 | every other list (imports, globals, exports, parameters, methods, constant elements, byte-string constant lengths, jump-table entries, handlers, line rows, locals) |

Registers, captures, name refs, type refs, call shapes, and struct fields are also capped at
65,536 by the format (`Limit::PerFunction`), and parameter lists and call shapes at 255 entries
(`Limit::Arity`). Independently of these, no list is reserved before the remaining
input is long enough to hold it.

### `Limit`

Which budget was hit: `Bytes`, `Strings`, `Types`, `Consts`, `ConstDepth`, `Functions`, `Insts`,
`TotalInsts`, `Items`, `PerFunction`, `Arity`. `#[non_exhaustive]`, `Display`.

### `DecodeError`, `DecodeErrorKind`

`DecodeError` has `offset() -> usize` (where in the input) and `kind() -> &DecodeErrorKind`;
`Display` is `at byte N: reason`.

| `DecodeErrorKind` | Meaning |
|---|---|
| `UnexpectedEnd` | the input ends before a value it promises (including counts the remaining bytes cannot hold) |
| `BadMagic` | not `LSB\0` |
| `UnsupportedVersion(v)` | not format version 2 |
| `ReservedFlags(v)` | non-zero header flags |
| `SectionOutOfOrder { expected, found }` | sections must be ids 1–10 in order |
| `SectionLength(id)` | a section's contents do not fill its length exactly |
| `TrailingBytes` | bytes after section 10 |
| `LimitExceeded(Limit)` | a budget |
| `InvalidUtf8` | a string-table entry |
| `InvalidTag { what, tag }` | a tag byte naming nothing (value type, type, constant, boolean, option, export, hook, callee, parameter list flags, parameter kind, parameter flags, call shape argument) |
| `InvalidChar(v)` | a `char` constant that is not a scalar value |
| `ConstForwardRef { index, child }` | an aggregate constant refers to itself or a later one |
| `HookOrder` | hook bindings not strictly increasing |
| `DebugCount { expected, found }` | the debug section describes a different number of functions |
| `Inst(InstError)` | a malformed instruction |

## `ErrorKind`

Runtime error kinds with stable codes, shared by every execution tier (OPS §6 and LSB §6):
`ArithOverflow` (E0001), `DivByZero` (E0002), `ShiftOutOfRange` (E0003), `InvalidConversion`
(E0004), `InvalidChar` (E0005), `NegativeExponent` (E0006), `TypeError` (E0100), `NullReference`
(E0101), `IndexOutOfBounds` (E0102), `KeyNotFound` (E0103), `UndefinedProperty` (E0104),
`StackOverflow` (E0105), `OutOfMemory` (E0106), `OutOfFuel` (E0107), `InvalidStrIndex` (E0108),
`Unreachable` (E0109), `InvalidCoroState` (E0110), `CannotSuspend` (E0111), `NoScheduler` (E0112),
`CloseIgnored` (E0113), `ArgumentError` (E0114), `NoMatch` (E0200, HIR's: raised by code
generators with `raise`). OPS owns E0001–E0099, LSB E0100–E0199, HIR E0200–E0299. Methods: `ALL`,
`code`, `from_code`, `name`, `is_catchable` (false for `OutOfMemory`, `OutOfFuel`, `Unreachable`).
`#[non_exhaustive]`. Every code fits one byte, the modifier of `raise`.

```rust
use bytecode_lang::ErrorKind;

assert_eq!(ErrorKind::IndexOutOfBounds.to_string(), "E0102 IndexOutOfBounds");
assert_eq!(ErrorKind::from_code(2), Some(ErrorKind::DivByZero));
```

## Instruction table

Every instruction of format version 2, generated from `Opcode::fields`. Modifiers print as mnemonic
suffixes; operands print in the order shown. Semantics and typing rules: `specs/LSB.md` §5. A test
checks that this table lists every opcode.

| Opcode | Mnemonic | Modifier | Operands |
|---|---|---|---|
| 0x00 | `nop` | — | — |
| 0x01 | `mov` | — | dst reg, src reg |
| 0x02 | `load_const` | — | dst reg, k const |
| 0x03 | `dload_const` | — | dst reg, k const |
| 0x04 | `load_int` | ty:IntTy | dst reg, val i32 |
| 0x05 | `dload_int` | — | dst reg, val i32 |
| 0x06 | `load_bool` | — | dst reg, val bool |
| 0x07 | `load_nil` | — | dst reg |
| 0x08 | `load_import` | — | dst reg, import import |
| 0x09 | `get_global` | — | dst reg, global global |
| 0x0A | `set_global` | — | global global, src reg |
| 0x10 | `iadd` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x11 | `isub` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x12 | `imul` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x13 | `idiv` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x14 | `irem` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x15 | `ifloor_div` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x16 | `ifloor_mod` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x17 | `iand` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x18 | `ior` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x19 | `ixor` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x1A | `ishl` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x1B | `ishr` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x1C | `imin` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x1D | `imax` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x1E | `ineg` | op:IntOp | dst reg, src reg |
| 0x1F | `ibit_not` | op:IntOp | dst reg, src reg |
| 0x20 | `iabs` | op:IntOp | dst reg, src reg |
| 0x21 | `ieq` | ty:IntTy | dst reg, lhs reg, rhs reg |
| 0x22 | `ine` | ty:IntTy | dst reg, lhs reg, rhs reg |
| 0x23 | `ilt` | ty:IntTy | dst reg, lhs reg, rhs reg |
| 0x24 | `ile` | ty:IntTy | dst reg, lhs reg, rhs reg |
| 0x25 | `igt` | ty:IntTy | dst reg, lhs reg, rhs reg |
| 0x26 | `ige` | ty:IntTy | dst reg, lhs reg, rhs reg |
| 0x27 | `ipow` | op:IntOp | dst reg, lhs reg, rhs reg |
| 0x30 | `fadd` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x31 | `fsub` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x32 | `fmul` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x33 | `fdiv` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x34 | `frem` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x35 | `fieee_rem` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x36 | `fmin` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x37 | `fmax` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x38 | `ffma` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x39 | `fneg` | ty:FloatTy | dst reg, src reg |
| 0x3A | `fabs` | ty:FloatTy | dst reg, src reg |
| 0x3B | `fsqrt` | ty:FloatTy | dst reg, src reg |
| 0x3C | `ffloor` | ty:FloatTy | dst reg, src reg |
| 0x3D | `fceil` | ty:FloatTy | dst reg, src reg |
| 0x3E | `ftrunc` | ty:FloatTy | dst reg, src reg |
| 0x3F | `fround` | ty:FloatTy | dst reg, src reg |
| 0x40 | `fround_even` | ty:FloatTy | dst reg, src reg |
| 0x41 | `feq` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x42 | `fne` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x43 | `flt` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x44 | `fle` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x45 | `fgt` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x46 | `fge` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x47 | `ftotal_cmp` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x48 | `fpow` | ty:FloatTy | dst reg, lhs reg, rhs reg |
| 0x50 | `bnot` | — | dst reg, src reg |
| 0x51 | `band` | — | dst reg, lhs reg, rhs reg |
| 0x52 | `bor` | — | dst reg, lhs reg, rhs reg |
| 0x53 | `bxor` | — | dst reg, lhs reg, rhs reg |
| 0x54 | `ceq` | — | dst reg, lhs reg, rhs reg |
| 0x55 | `cne` | — | dst reg, lhs reg, rhs reg |
| 0x56 | `clt` | — | dst reg, lhs reg, rhs reg |
| 0x57 | `cle` | — | dst reg, lhs reg, rhs reg |
| 0x58 | `cgt` | — | dst reg, lhs reg, rhs reg |
| 0x59 | `cge` | — | dst reg, lhs reg, rhs reg |
| 0x5A | `ref_eq` | — | dst reg, lhs reg, rhs reg |
| 0x60 | `int_cast` | conv:IntConv | dst reg, src reg |
| 0x61 | `zext` | pair:IntPair | dst reg, src reg |
| 0x62 | `sext` | pair:IntPair | dst reg, src reg |
| 0x63 | `trunc` | pair:IntPair | dst reg, src reg |
| 0x64 | `int_to_f32` | ty:IntTy | dst reg, src reg |
| 0x65 | `int_to_f64` | ty:IntTy | dst reg, src reg |
| 0x66 | `f32_to_int` | conv:FloatConv | dst reg, src reg |
| 0x67 | `f64_to_int` | conv:FloatConv | dst reg, src reg |
| 0x68 | `f32_to_f64` | — | dst reg, src reg |
| 0x69 | `f64_to_f32` | — | dst reg, src reg |
| 0x6A | `float_to_bits` | ty:IntTy | dst reg, src reg |
| 0x6B | `bits_to_float` | ty:IntTy | dst reg, src reg |
| 0x6C | `char_from_u32` | — | dst reg, src reg |
| 0x6D | `char_to_u32` | — | dst reg, src reg |
| 0x6E | `bool_to_int` | ty:IntTy | dst reg, src reg |
| 0x70 | `dadd` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x71 | `dsub` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x72 | `dmul` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x73 | `ddiv` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x74 | `drem` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x75 | `dfloor_div` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x76 | `dfloor_mod` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x77 | `dand` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x78 | `dor` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x79 | `dxor` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x7A | `dshl` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x7B | `dshr` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x7C | `dneg` | pol:Policy | dst reg, src reg |
| 0x7D | `dbit_not` | pol:Policy | dst reg, src reg |
| 0x7E | `deq` | — | dst reg, lhs reg, rhs reg |
| 0x7F | `dne` | — | dst reg, lhs reg, rhs reg |
| 0x80 | `dlt` | — | dst reg, lhs reg, rhs reg |
| 0x81 | `dle` | — | dst reg, lhs reg, rhs reg |
| 0x82 | `dgt` | — | dst reg, lhs reg, rhs reg |
| 0x83 | `dge` | — | dst reg, lhs reg, rhs reg |
| 0x84 | `dtruthy` | — | dst reg, src reg |
| 0x85 | `dconcat` | — | dst reg, lhs reg, rhs reg |
| 0x86 | `to_dyn` | from:Prim | dst reg, src reg |
| 0x87 | `from_dyn` | to:Prim | dst reg, src reg |
| 0x88 | `type_of` | — | dst reg, src reg |
| 0x89 | `is_kind` | kind:Kind | dst reg, src reg |
| 0x8A | `is_type` | — | dst reg, src reg, ty type |
| 0x8B | `cast` | — | dst reg, src reg, ty type |
| 0x8C | `dget_index` | — | dst reg, obj reg, key reg |
| 0x8D | `dset_index` | — | obj reg, key reg, src reg |
| 0x8E | `get_prop` | — | dst reg, obj reg, name name |
| 0x8F | `set_prop` | — | obj reg, name name, src reg |
| 0x90 | `has_prop` | — | dst reg, obj reg, name name |
| 0x91 | `dcall` | — | dst reg, callee reg, argc u8 |
| 0x92 | `diter_new` | — | dst reg, src reg |
| 0x93 | `dlen` | — | dst reg, src reg |
| 0x94 | `dnot` | — | dst reg, src reg |
| 0x95 | `dpow` | pol:Policy | dst reg, lhs reg, rhs reg |
| 0x96 | `dabs` | pol:Policy | dst reg, src reg |
| 0x97 | `dsep_index` | — | dst reg, obj reg, key reg |
| 0x98 | `dsep_prop` | — | dst reg, obj reg, name name |
| 0x99 | `dcall_shape` | — | dst reg, callee reg, shape shape |
| 0x9A | `dparam_ref` | — | dst reg, callee reg, pos reg |
| 0x9B | `dparam_ref_named` | — | dst reg, callee reg, name name |
| 0x9C | `err_payload` | — | dst reg, src reg |
| 0xA0 | `jmp` | — | target target |
| 0xA1 | `jmp_if` | — | cond reg, target target |
| 0xA2 | `jmp_if_not` | — | cond reg, target target |
| 0xA3 | `switch` | ty:IntTy | src reg, table table |
| 0xA4 | `call` | — | dst reg, func func, argc u8 |
| 0xA5 | `call_indirect` | — | dst reg, callee reg, argc u8 |
| 0xA6 | `call_import` | — | dst reg, import import, argc u8 |
| 0xA7 | `tail_call` | — | func func, args reg, argc u8 |
| 0xA8 | `tail_call_indirect` | — | callee reg, args reg, argc u8 |
| 0xA9 | `ret` | — | src reg |
| 0xAA | `ret_void` | — | — |
| 0xAB | `throw` | — | src reg |
| 0xAC | `err_code` | — | dst reg, src reg |
| 0xAD | `safepoint` | — | — |
| 0xAE | `unreachable` | — | — |
| 0xAF | `raise` | kind:ErrorKind | src reg |
| 0xB0 | `make_closure` | — | dst reg, func func |
| 0xB1 | `get_upval` | — | dst reg, idx upval |
| 0xB2 | `new_cell` | — | dst reg, src reg, ty type |
| 0xB3 | `cell_get` | — | dst reg, cell reg |
| 0xB4 | `cell_set` | — | cell reg, src reg |
| 0xB5 | `new_ref` | — | dst reg, src reg |
| 0xB6 | `dref_index` | — | dst reg, obj reg, key reg |
| 0xB7 | `dref_prop` | — | dst reg, obj reg, name name |
| 0xB8 | `dbind_index` | — | obj reg, key reg, src reg |
| 0xB9 | `dbind_prop` | — | obj reg, name name, src reg |
| 0xBA | `dunref_index` | — | obj reg, key reg |
| 0xBB | `dunref_prop` | — | obj reg, name name |
| 0xC0 | `new_struct` | — | dst reg, ty type |
| 0xC1 | `get_field` | — | dst reg, obj reg, field field |
| 0xC2 | `set_field` | — | obj reg, field field, src reg |
| 0xC3 | `new_array` | — | dst reg, len reg, ty type |
| 0xC4 | `array_len` | — | dst reg, arr reg |
| 0xC5 | `array_get` | — | dst reg, arr reg, idx reg |
| 0xC6 | `array_set` | — | arr reg, idx reg, src reg |
| 0xC7 | `array_push` | — | arr reg, src reg |
| 0xC8 | `array_pop` | — | dst reg, arr reg |
| 0xC9 | `new_map` | — | dst reg, ty type |
| 0xCA | `map_len` | — | dst reg, map reg |
| 0xCB | `map_get` | — | dst reg, map reg, key reg |
| 0xCC | `map_find` | — | dst reg, map reg, key reg |
| 0xCD | `map_has` | — | dst reg, map reg, key reg |
| 0xCE | `map_set` | — | map reg, key reg, src reg |
| 0xCF | `map_del` | — | map reg, key reg |
| 0xD0 | `map_push` | — | map reg, src reg |
| 0xD1 | `iter_new` | — | dst reg, src reg |
| 0xD2 | `iter_next` | — | has reg, iter reg, val reg |
| 0xD3 | `iter_key` | — | dst reg, iter reg |
| 0xD4 | `dup` | — | dst reg, src reg |
| 0xE0 | `str_len` | — | dst reg, s reg |
| 0xE1 | `str_concat` | — | dst reg, lhs reg, rhs reg |
| 0xE2 | `str_concat_n` | — | dst reg, first reg, count u8 |
| 0xE3 | `str_eq` | — | dst reg, lhs reg, rhs reg |
| 0xE4 | `str_cmp` | — | dst reg, lhs reg, rhs reg |
| 0xE5 | `str_slice` | — | dst reg, s reg, range reg, utf8 bool |
| 0xE6 | `str_byte` | — | dst reg, s reg, idx reg |
| 0xF0 | `coro_new` | — | dst reg, func func, argc u8 |
| 0xF1 | `coro_new_indirect` | — | dst reg, callee reg, argc u8 |
| 0xF2 | `yield` | — | dst reg, src reg |
| 0xF3 | `await` | — | dst reg, src reg |
| 0xF4 | `resume` | — | dst reg, coro reg, src reg |
| 0xF5 | `resume_throw` | — | dst reg, coro reg, src reg |
| 0xF6 | `coro_status` | — | dst reg, coro reg |
| 0xF7 | `coro_current` | — | dst reg |
| 0xF8 | `spawn` | — | dst reg, callee reg, argc u8 |
| 0xF9 | `yield_kv` | — | dst reg, key reg, src reg |
| 0xFA | `coro_close` | — | dst reg, coro reg, src reg |
| 0xFB | `coro_key` | — | dst reg, coro reg |
| 0xFC | `coro_result` | — | dst reg, coro reg |

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | on | Reserved; the crate is `no_std` + `alloc` either way and needs nothing from `std`. Errors implement `core::error::Error`. |

## Stability

Pre-1.0. The surface above may change in 0.x minors; changes are listed in `CHANGELOG.md`. The
encoded format carries its own version (`FORMAT_VERSION`), bumped on every layout or semantic
change, so no module is silently misread across versions. `Inst` and `Opcode` are intentionally
exhaustive; error enums and `Limits` are `#[non_exhaustive]`. 1.0 follows the verifier (0.5),
hardening (0.9), and a VM running Mox programs from LSB.
