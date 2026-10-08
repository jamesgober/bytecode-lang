# bytecode-lang - Roadmap

> Path from scaffold to a stable 1.0. Hard parts are front-loaded; each phase has hard exit criteria.
> Master plan: ../_lexersketch/ROADMAP.md and ../_lexersketch/NEW-LIBS.md
>
> **Anti-deferral rule:** no listed hard task moves to a later phase unless this file records the move and the reason.

## v0.1.0 - Scaffold (DONE)
Compiles, CI green, structure correct, no domain logic.
- [x] Manifest, README, CHANGELOG, REPS, dual license, CI, deny, clippy, rustfmt, DIRECTIVES, ROADMAP.

## v0.2.0 - Foundation
- [ ] Instruction set (frames, calls, closures, heap objects, control flow, arithmetic per the ops policy), module structure, encoding/decoding, disassembler.
- [ ] Property tests: round trip, decoder never panics.

## v0.5.0 - Implementation
- [ ] Verifier (types, bounds, jump targets, stack/frame shape); debug line tables; benchmarks.

## v0.9.0 - Hardening
- [ ] Fuzzing the decoder and verifier; audit with bvm-lang 2.0 and codegen-lang 2.0 as consumers.

## v1.0.0 - Stable
- [ ] Frozen after the VM runs Mox programs from it (D18).
