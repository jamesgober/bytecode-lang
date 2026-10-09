<h1 align="center">
    <img width="90px" height="auto" src="https://raw.githubusercontent.com/jamesgober/jamesgober/main/media/icons/hexagon-3.svg" alt="Triple Hexagon">
    <br><b>CHANGELOG</b>
</h1>
<p>
  All notable changes to <code>bytecode-lang</code> will be documented in this file. The format is based on <a href="https://keepachangelog.com/en/1.1.0/">Keep a Changelog</a>,
  and this project adheres to <a href="https://semver.org/spec/v2.0.0.html/">Semantic Versioning</a>.
</p>

---

## [Unreleased]

### Added

### Changed

### Fixed

### Security

---

## [0.3.0] - 2026-10-09

LSB **format version 2**: the format gaps bcgen-lang 0.2 and bvm-lang 2.0.0-alpha.2 found
(LexerSketch ISSUES P23, P26, P27 rule 10, P28). A breaking 0.x minor: format 1 files are refused,
and the API changes listed under **Breaking** need code changes in every consumer. The spec,
`specs/LSB.md`, is revised in the same change (format version 2, §5.15–5.17 new), with a note on
dynamic NaN added to `specs/OPS.md` §4.

### Breaking

- **`FORMAT_VERSION` is 2.** `decode` refuses version 1 (`UnsupportedVersion(1)`); regenerate
  stored modules. The listing header is `lsb 2`.
- **Layout.** The import record and the function record each gain an `opt<paramlist>` (after the
  signature); the function record gains a call-shape table after its jump tables. The minimum
  function record is 41 bytes (was 36), the minimum import record 13 (was 12). The empty module is
  still 130 bytes.
- **Policy bytes repacked for `shift = saturate`.** `Policy` is six bits: overflow (bits 0–1),
  div_zero (bit 2), shift (bits 3–4; code 3 reserved), float_to_int (bit 5); 48 valid values (was 5
  bits, all 32 valid). `IntOp` is type (bits 0–2), overflow (3–4), div_zero (5), shift (6–7); it no
  longer carries `float_to_int`: `IntOp::with_policy` drops it and `IntOp::policy` reports it as
  `Error`; bytes with shift code 3 are refused (192 valid, was all 256).
- **`Inst::F32ToInt` and `Inst::F64ToInt`** take `conv: FloatConv` (destination type and
  `float_to_int`) instead of `op: IntOp`.
- **Renamed instructions, same opcodes and semantics** (aligned with HIR's `not`/`bit_not`):
  `Inst::INot`/`inot` → `Inst::IBitNot`/`ibit_not` (0x1F); `Inst::DNot`/`dnot` (bitwise, 0x7D) →
  `Inst::DBitNot`/`dbit_not`; `Inst::DLNot`/`dlnot` (logical, 0x94) → `Inst::DNot`/`dnot`. Code
  that wrote `Inst::DNot { dst, src, pol }` no longer compiles (the new `DNot` has no `pol`), so
  the swap cannot pass silently.
- **`Import`** gains `params: Option<ParamList>` and is `Clone` but no longer `Copy`; struct
  literals need the new field.
- **`Kind`** gains `Reference` (14); **`Hook`** gains `Pow` (28), `Abs` (29), `CallShape` (30);
  **`FieldKind`** gains `Shape`, `FloatConv`, `ErrKind`. These enums are exhaustive, so `match`es on
  them need the new arms.
- **Semantics.** `dcall` binds through the callee's parameter list and reports an argument-count
  mismatch as `ArgumentError` (E0114) instead of `TypeError`. The generator automatic key (LSB
  §5.13 rule 10) follows PHP's generators: after only `yield -5 => x` the next automatic key is 0
  (format 1 said -4, the array rule, which maps keep). `dup` of a reference raises `TypeError`.
  `cell_get`/`cell_set` also accept a `dyn` holding a cell or reference.
- **`Opcode::ALL`** has 200 entries (was 182).

### Added

- **Dynamic calls with real signatures** (LSB §5.15): `ParamList` (`Param`, `ParamKind`:
  positional-only, normal, named-only, rest, rest map, named rest; by-reference and default flags;
  `ignore_extra`; a trailing `i64` presence mask when a parameter has a default) on functions
  (`FunctionBuilder::set_params`, `Function::params`) and imports
  (`ModuleBuilder::import_with_params`, `Import::params`), with `validate`, `fits`, and
  `ParamError`. `CallShape` (`ArgKind`: positional, named, spread, named spread; `ShapeError`) in a
  per-function shape table (`FunctionBuilder::call_shape`, `Function::shapes`, `ShapeId`).
  `Inst::DCallShape` (`dcall_shape`, 0x99) and `FunctionBuilder::dcall_shape`.
- **One binding rule:** `ParamList::bind` (`ArgItem`, `Binding`, `Bound`, `BindError`) implements
  LSB §5.15's argument binding so every tier binds alike; `ParamList::positional_by_ref` and
  `named_by_ref` are the semantics of the new `Inst::DParamRef` (`dparam_ref`, 0x9A) and
  `Inst::DParamRefNamed` (`dparam_ref_named`, 0x9B), with which a PHP code generator decides at run
  time whether to send a reference. Host functions are first-class values with any signature.
- **Copy-on-write separation** (LSB §5.16): `Inst::DSepIndex` (`dsep_index`, 0x97) and
  `Inst::DSepProp` (`dsep_prop`, 0x98) make a nested element safe to write in place, copying it
  only when another container may share it (PHP's `$a[$k][] = $v` without a copy per write).
- **References** (PHP `&`, LSB §5.17): `Kind::Reference`; `Inst::NewRef` (0xB5),
  `Inst::DRefIndex` (0xB6), `Inst::DRefProp` (0xB7), `Inst::DBindIndex` (0xB8),
  `Inst::DBindProp` (0xB9), `Inst::DUnrefIndex` (0xBA), `Inst::DUnrefProp` (0xBB); reference slots
  are transparent and shared by copies, as PHP's are.
- **OPS v2 operations:** `Inst::IPow` (`ipow`, 0x27), `Inst::FPow` (`fpow`, 0x48), `Inst::DPow`
  (`dpow`, 0x95), `Inst::DAbs` (`dabs`, 0x96, `abs` with `promote`), the `pow` and `abs` hooks,
  `Shift::Saturate` (printed `shsat`), `FloatConv`.
- **Errors:** `ErrorKind::NegativeExponent` (E0006), `ErrorKind::ArgumentError` (E0114),
  `ErrorKind::NoMatch` (E0200, HIR v3 §8.10); `Inst::Raise` (`raise`, 0xAF; raises any catchable
  kind with a payload, printed `raise.E0200`) and `Inst::ErrPayload` (`err_payload`, 0x9C).
- `Limit::Arity`: parameter lists and call shapes are capped at 255 entries by the format; call
  shapes at 65,536 per function.
- `BuildError::InvalidParams` and `BuildError::InvalidShape`.
- The disassembler prints parameter lists (`params (&s1, s2 = ?, ...map _) ignore_extra`), call
  shapes (`shape cs0 (_, s3:, ...)`), and resolves names in `dcall_shape` comments.
- Example `php`; benchmark `bind/php_100k_calls`; tests `tests/binding.rs` (the binder against an
  independent reference, order independence of named arguments, the by-reference queries) and
  `tests/format2.rs` (every new instruction, rename, and modifier change), plus parameter lists and
  call shapes in the arbitrary-module generators, codec, and builder tests.

### Changed

- Version 0.3.0. The spec's §10 questions 3 (variadic dynamic calls), 7 (suspended stacks: bvm-lang
  2.0.0-alpha.2 copies on suspension), and 9 (close on drop: implemented, no per-coroutine flag in
  format 2) are recorded as answered.

---

## [0.2.0] - 2026-10-08

The foundation: LSB's instruction set, module model, versioned encoding, budgeted decoder,
disassembler, and a builder that resolves branch labels. The normative specification is the
LexerSketch spec `specs/LSB.md`, written alongside this release. The verifier is 0.5.0.

### Added

- `Inst`: 182 instructions in eight-byte words (the decoded enum is also eight bytes): moves,
  constants, and globals; OPS integer arithmetic, each carrying an `IntOp` (type plus the complete
  overflow / div_zero / shift / float_to_int policy); float arithmetic; conversions; boolean, char,
  and reference comparisons; dynamic (`dyn`) arithmetic and comparison with a numeric fast path and
  per-module hooks; type tests and casts; dynamic indexing, properties, calls, iteration, and length;
  branches and jump tables; direct, indirect, host, tail, and dynamic calls with window operands;
  exceptions (`throw`, try regions, `err_code`); safepoints and `unreachable`; closures with
  by-value captures and cells; structs, arrays, insertion-ordered hash maps with PHP's next-integer
  key, typed and dynamic iterators, `dup`; byte strings; `dlnot`, the dynamic logical not
  (PHP `!`), beside `dnot`, the bitwise `~`.
- Coroutines, generators, and async: stackful asymmetric coroutines with `coro_new`,
  `coro_new_indirect`, `yield`, `yield_kv` (PHP keyed yields), `await`, `resume`,
  `resume_throw`, `coro_close` (runs pending `finally` blocks; Python `close()`), `coro_key`,
  `coro_result` (Python `StopIteration.value`, PHP `getReturn()`), `coro_status`,
  `coro_current`, and `spawn` (through the new `Hook::Spawn` into the host scheduler);
  `CoroState`, `Kind::Coroutine`, `TypeDef::Coroutine`, and the error kinds `InvalidCoroState`
  (E0110), `CannotSuspend` (E0111), `NoScheduler` (E0112), `CloseIgnored` (E0113). Defined, encoded, decoded, and
  disassembled; execution arrives with bvm-lang 2.0.
- `Overflow::Promote` (OPS §2, for PHP): the nearest `f64` when an integer result does not fit,
  valid only with a `dyn` destination. It is carried in every policy byte; `Inst::overflow`
  reports an instruction's overflow policy; the builder refuses `promote` on a destination
  declared with a static type (`BuildError::PromoteNotDynamic`).
- `Opcode`, `FieldSpec`, `FieldKind`, `Slot`: per-opcode operand metadata, generated from the same
  table as the instruction enum, its encoder, decoder, and renderer.
- `Module` and `Function`: string table, type table (`TypeDef`: function signatures, structs with
  single inheritance and methods, arrays, maps, cells, iterators), constant pool (`Const`: exact
  integers, IEEE bit patterns, chars, UTF-8 and byte strings, arrays and maps of earlier constants),
  imports, globals, functions (typed registers, captures, per-function name and type lists, jump
  tables, try regions, code), exports, hooks, module name and start function, and per-function line
  tables and local variables.
- `encode`: canonical, deterministic, fixed-width little-endian encoding with a versioned header
  (`MAGIC`, `FORMAT_VERSION` = 1) and ten length-prefixed sections; exact output size computed by
  running the writer against a counter.
- `decode` / `decode_with` and `Limits`: a total decoder that refuses unknown versions, checks
  every tag, UTF-8, `char`, canonical instruction words, section framing, the constant DAG and its
  depth, and every count against configurable budgets and against the bytes remaining, before any
  allocation. `DecodeError` reports the byte offset.
- `disassemble` (and `Display` for `Module` and `Inst`): a deterministic listing with labels,
  mnemonic modifiers (`iadd.i64.wrap`), and name and constant comments; out-of-range references
  print as `<invalid>`.
- `ModuleBuilder` and `FunctionBuilder`: deduplicated strings, structural types, and constants;
  reserved types for recursive structs; functions declared before they are defined; labels for
  branches, jump tables, try regions, and locals, resolved and range-checked so a built function
  never has an unresolved or out-of-range target; `parallel_move`, which sequentializes
  simultaneous moves with one temporary per cycle (the fix for family issue H01), and never uses
  a register the move set names as that temporary.
- `ErrorKind`: the OPS and LSB runtime error kinds with their stable codes.
- Property tests (round trip, canonicity, determinism, decoder robustness on arbitrary, framed, and
  mutated bytes, budgets, label resolution against a reference, parallel moves against parallel
  assignment), integration tests for every decode and build error, a golden disassembly, and
  criterion benchmarks at one million instructions.
- Examples: `quickstart`, `inspect`.

### Changed

- Version 0.2.0. The crate has no dependencies (constants are format values, not `value-lang`
  values; see `dev/ROADMAP.md`).

---

## [0.1.0] - 2026-10-08

Initial scaffold and repository bootstrap. No domain logic yet &mdash; this release establishes the structure, tooling, and quality gates the implementation will be built on.

### Added

- `Cargo.toml` with crate metadata, Rust 2024 edition, MSRV 1.85.
- Dual `Apache-2.0 OR MIT` license files.
- `README.md`, `CHANGELOG.md`, and a documentation skeleton.
- `REPS.md` compliance baseline.
- `.github/workflows/ci.yml` CI matrix; `deny.toml`, `clippy.toml`, `rustfmt.toml`.
- `dev/DIRECTIVES.md` and `dev/ROADMAP.md` (committed engineering standards + plan).

[Unreleased]: https://github.com/jamesgober/bytecode-lang/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/jamesgober/bytecode-lang/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jamesgober/bytecode-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/bytecode-lang/releases/tag/v0.1.0
