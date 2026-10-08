# bytecode-lang - Roadmap

> Path from scaffold to a stable 1.0. Hard parts are front-loaded; each phase has hard exit criteria.
> Master plan: ../_lexersketch/ROADMAP.md and ../_lexersketch/NEW-LIBS.md
> Normative spec: ../_lexersketch/specs/LSB.md (format version 1).
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

## v0.5.0 - Implementation
- [ ] Verifier per `specs/LSB.md` §8 (module, function shape, control flow including the
  every-cycle-has-a-safepoint rule, types, constants): total, sound, linear, precise errors.
- [ ] Property tests: verifier accepts every builder-made module that follows the typing rules,
  rejects each single-rule violation with the right error, and is total on decoded arbitrary input.
- [ ] Benchmarks for the verifier at 1M instructions.
- [ ] Verifier rules added at the 0.2.0 review: V-M8 (`spawn` needs the `spawn` hook), V-T8
  (`promote` only with a `dyn` destination, for destinations the builder could not see), V-T9
  (coroutine instruction typing).

## v0.9.0 - Hardening
- [ ] Fuzzing the decoder and verifier (cargo-fuzz outside the crate's deps, or long proptest runs);
  audit with bvm-lang 2.0 and codegen-lang 2.0 as consumers.
- [ ] Settle the open questions in `specs/LSB.md` §10 with the owner (promote policy, variadic
  dynamic calls, fused compare-branch, non-null refs).

## v1.0.0 - Stable
- [ ] Frozen after the VM runs Mox programs from it (D18).
