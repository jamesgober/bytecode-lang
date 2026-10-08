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

[Unreleased]: https://github.com/jamesgober/bytecode-lang/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/jamesgober/bytecode-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/bytecode-lang/releases/tag/v0.1.0
