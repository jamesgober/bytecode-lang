# bytecode-lang - Roadmap

> Path from scaffold to a stable 1.0. Hard parts are front-loaded; each phase has hard exit criteria.
> Master plan: ../_lexersketch/ROADMAP.md and ../_lexersketch/NEW-LIBS.md
> Normative spec: ../_lexersketch/specs/LSB.md (format version 2 since v0.3.0).
>
> **Anti-deferral rule:** no listed hard task moves to a later phase unless this file records the move and the reason.

## v0.1.0 - Scaffold (DONE)
Compiles, CI green, structure correct, no domain logic.
- [x] Manifest, README, CHANGELOG, REPS, dual license, CI, deny, clippy, rustfmt, DIRECTIVES, ROADMAP.

## v0.2.0 - Foundation (DONE)
- [x] Instruction set (frames, calls, closures, heap objects, control flow, arithmetic per the ops policy), module structure, encoding/decoding, disassembler.
- [x] Property tests: round trip, decoder never panics.

Delivered:
- `specs/LSB.md`: design rationale, value model, module structure, the complete instruction set
  with semantics and typing rules, runtime error kinds, the byte-level encoding, decoding budgets,
  and the verifier rules planned for v0.5.0.
- `Inst` (182 instructions, eight bytes encoded and decoded) and its metadata (`Opcode`,
  `FieldSpec`, `FieldKind`, `Slot`), all generated from one table. Every integer instruction
  carries an `IntOp` (type + full OPS policy set).
- The module model (`Module`, `Function`, `TypeDef`, `Const`, imports, globals, exports, hooks,
  meta, line tables, locals) with read-only accessors.
- `encode` (canonical, deterministic, exact-size), `decode`/`decode_with` (versioned, budgeted by
  `Limits`, total on arbitrary bytes, allocation bounded by the input), `disassemble`
  (deterministic, labels and comments, safe on unverified modules).
- `ModuleBuilder`/`FunctionBuilder`: dedup, reserved types, early-declared functions, label
  resolution with a final range check on every branch, and `parallel_move` (the H01 class fixed at
  the format's builder).
- `ErrorKind`: the shared runtime error codes (OPS E0001-E0005, LSB E0100-E0112).
- Review changes before release: `overflow = promote` (OPS §2) in every policy byte, refused by the
  builder on statically typed destinations; coroutines, generators, and async designed in full
  (`specs/LSB.md` §5.13) and their nine instructions added to the ISA now, because the instruction
  set is a wall and adding them later would move the opcode space (including the HIR 0.3.0
  review's keyed yields, return values, close with `finally`, async generators, and precise unwind
  points with the canonical `finally` lowering, `specs/LSB.md` §4.3); `dlnot` and the truthiness
  table; the float floor-division algorithm verified against CPython 3.14 (296,457 cases,
  `dev/verify/float_floor.py`); the `parallel_move` temporary can no longer be a register the move
  set names.
- Tests: property tests for every DIRECTIVES §4 invariant v0.2 can check (round trip, canonicity,
  determinism, decoder robustness and budgets), label resolution and parallel moves against
  references, every decode and build error path, a golden listing; criterion benchmarks at 1M
  instructions.

Pulled forward (not deferred): the debug line table and local-variable table were planned for
v0.5.0; they are part of the module structure the spec defines, so they are in the v0.2.0 model and
encoding (and round-trip tests). v0.5.0 keeps only their verification rules (V-F4).

Dependency wiring (decided here, recorded per the anti-deferral rule):
- **value-lang: not wired.** Constants are format values, not runtime values. value-lang 1 holds
  integers as `i32` and symbols as process-local interner ids, so it cannot represent `i64`, `u64`,
  `f32`, `char`, or byte-string constants, and its symbols do not serialize. The format is also
  consumed by static-language tiers whose registers are unboxed. The NaN-boxed layout is applied by
  the VM at load time (the Value ABI wall); the spec states the one representation constraint LSB
  needs (heap references share one representation across `str`, `ref`, and `dyn`; nil is null).
- **No other first-party crate is used.** The crate has no dependencies.
- The `std` feature is kept for family consistency; it enables nothing (errors use
  `core::error::Error`).

Known gaps carried into v0.5.0 (by design of the phase split, not deferrals):
- Nothing checks meaning yet: index ranges, register types, frame shapes. A decoded module is
  well-formed, not verified.

## v0.3.0 - Format version 2: PHP semantics the VM cannot fake (DONE, prepared 2026-10-09)
Inserted before the verifier because bcgen-lang 0.2 and bvm-lang 2.0.0-alpha.2 found format gaps
that block Mox (LexerSketch ISSUES P23, P26, P27 rule 10, P28) and that the verifier must already
know about; a breaking 0.x minor.

Delivered:
- `specs/LSB.md` revised to format version 2: §5.15 dynamic calls (parameter lists, call shapes,
  the binding rule, run-time by-reference decisions, host functions as values), §5.16
  copy-on-write separation (the value discipline, the guarantee, a conforming two-bit
  implementation for tracing collectors), §5.17 references (transparent reference slots, copies
  share them, the one divergence from PHP and `dunref_*`), OPS v2 `pow`/`abs`/`shift = saturate`
  in every form, `raise`/`err_payload`, E0006/E0114/E0200, generator rule 10 changed to PHP's,
  dynamic NaN stated (§5.6, plus a note in `specs/OPS.md` §4), not/bit-not names aligned with HIR,
  §7.4 version history, verifier rules V-M9, V-F5, V-T10, and §10 questions 3, 7, 9 answered.
- 18 instructions (`ipow`, `fpow`, `dpow`, `dabs`, `dsep_index`, `dsep_prop`, `dcall_shape`,
  `dparam_ref`, `dparam_ref_named`, `err_payload`, `raise`, `new_ref`, `dref_index`, `dref_prop`,
  `dbind_index`, `dbind_prop`, `dunref_index`, `dunref_prop`; 200 in all), three renames
  (`ibit_not`, `dbit_not`, `dnot`), `Kind::Reference`, hooks 28–30, `ErrorKind` E0006/E0114/E0200.
- `Policy` repacked to six bits with `Shift::Saturate`; `IntOp` carries type, overflow, div_zero,
  and shift; `FloatConv` for the float-to-integer conversions.
- `ParamList`/`Param`/`ParamKind`/`ParamError` and `CallShape`/`ArgKind`/`ShapeError` in the
  module model, encoding, decoder (`Limit::Arity`), builder (`set_params`, `import_with_params`,
  `call_shape`, `dcall_shape`, `InvalidParams`, `InvalidShape`), and disassembler.
- `ParamList::bind` (`ArgItem`, `Binding`, `Bound`, `BindError`): the binding rule written once, so
  bvm-lang, the T0 evaluator, and a native tier bind alike; `positional_by_ref`/`named_by_ref` for
  `dparam_ref`/`dparam_ref_named`.
- Tests: `tests/binding.rs` (the binder against an independently written reference on random
  lists and arguments, errors included; order independence of named arguments; the by-reference
  queries against the binding; 200,000 spread names), `tests/format2.rs` (every new instruction as
  a word, through a module, and in the listing; renames; repacked bytes; `raise` accepting exactly
  the catchable kinds), codec tests for the new records and their errors, builder tests for every
  new error, exhaustive tests of every policy byte, parameter lists and call shapes in the
  arbitrary-module property generators, an extended golden listing. Benchmark
  `bind/php_100k_calls`; example `php`.

Dependency wiring:
- **Still no dependencies.** Nothing new is needed: the binder works on the module's own strings.
  value-lang stays unwired for the reasons under v0.2.0.

Moved or not done here (recorded per the anti-deferral rule):
- **Executing** the new instructions is bvm-lang's (alpha.3), and **emitting** them bcgen-lang's
  (0.3); this crate defines, encodes, decodes, disassembles, and builds them, and provides the
  binder. Nothing here runs PHP code, so the separation guarantee and reference semantics are
  specified and unit-checked at the format level only; their end-to-end check is bvm-lang's
  differential suite.
- **`ls_pow`** (OPS v2's shared float `pow` routine, "specified in `ops-vectors/`") does not exist
  yet; `fpow`/`dpow` reference it. It belongs to the OPS vectors work, not to the format.
- **The per-coroutine "close on drop" flag** is not added: every target language closes every
  dropped generator (LSB §10 question 9).

## v0.5.0 - Implementation
- [ ] Verifier per `specs/LSB.md` §8 (module, function shape, control flow including the
  every-cycle-has-a-safepoint rule, types, constants): total, sound, linear, precise errors.
- [ ] Property tests: verifier accepts every builder-made module that follows the typing rules,
  rejects each single-rule violation with the right error, and is total on decoded arbitrary input.
- [ ] Benchmarks for the verifier at 1M instructions.
- [ ] Verifier rules added at the 0.2.0 review: V-M8 (`spawn` needs the `spawn` hook), V-T8
  (`promote` only with a `dyn` destination, for destinations the builder could not see), V-T9
  (coroutine instruction typing).
- [ ] Verifier rules added with format 2 (v0.3.0): V-M9 (parameter lists obey §5.15 and fit their
  signatures, by-reference `ref` parameters are `cell dyn`), V-F5 (call shapes valid, `ShapeId`s
  and `dcall_shape` windows in range), V-CF2 (`raise` ends a block), V-T10 (typing of the format 2
  instructions; `cell_get`/`cell_set` on `dyn`).

## v0.9.0 - Hardening
- [ ] Fuzzing the decoder and verifier (cargo-fuzz outside the crate's deps, or long proptest runs);
  audit with bvm-lang 2.0 and codegen-lang 2.0 as consumers.
- [ ] Settle the open questions in `specs/LSB.md` §10 with the owner (fused compare-branch,
  non-null refs, checked arithmetic with a flag, typed `catch`; promote and variadic dynamic calls
  are settled).

## v1.0.0 - Stable
- [ ] Frozen after the VM runs Mox programs from it (D18).
