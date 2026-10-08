<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br>
    <b>bytecode-lang</b>
    <br>
    <sub><sup>LEXERSKETCH BYTECODE FORMAT</sup></sub>
</h1>

<div align="center">
    <a href="https://crates.io/crates/bytecode-lang"><img alt="Crates.io" src="https://img.shields.io/crates/v/bytecode-lang"></a>
    <a href="https://crates.io/crates/bytecode-lang"><img alt="Downloads" src="https://img.shields.io/crates/d/bytecode-lang?color=%230099ff"></a>
    <a href="https://docs.rs/bytecode-lang"><img alt="docs.rs" src="https://img.shields.io/docsrs/bytecode-lang"></a>
    <a href="https://github.com/jamesgober/bytecode-lang/actions"><img alt="CI" src="https://github.com/jamesgober/bytecode-lang/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://github.com/rust-lang/rfcs/blob/master/text/2495-min-rust-version.md"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.85%2B-blue"></a>
</div>

<br>

<div align="left">
    <p>
        <strong>bytecode-lang</strong> defines LSB, the one bytecode format of the LexerSketch toolchain: a register-machine instruction set for static and dynamic languages alike, the module that holds it, a versioned binary encoding, a decoder that treats every byte as hostile, a deterministic disassembler, and a builder that resolves branch labels. Code generators emit LSB, the VM executes it, program images store it, and the debugger reads its line tables.
    </p>
    <p>
        Instructions are eight bytes, encoded and decoded alike, so a function body is a flat array the interpreter walks without parsing. Registers are typed, with <code>dyn</code> as one of the types, so the same format carries an unboxed <code>i64</code> loop for a systems language and PHP-style arrays, properties, and loose comparisons for a dynamic one. Every integer instruction carries its overflow, division, and shift policy, so every execution tier computes the same result or raises the same error.
    </p>
    <br>
    <hr>
    <p>
        <strong>MSRV is 1.85+</strong> (Rust 2024 edition). <code>no_std</code>-compatible (needs only <code>alloc</code>), <code>#![forbid(unsafe_code)]</code>, no dependencies.
    </p>
    <blockquote>
        <strong>Status: pre-1.0 (0.2.0, the foundation).</strong> The instruction set, module model, encoding, decoder, disassembler, and builder are complete. The verifier lands in 0.5.0; until then a decoded module is well-formed but not checked for meaning. See <a href="./dev/ROADMAP.md"><code>dev/ROADMAP.md</code></a> and <a href="./CHANGELOG.md"><code>CHANGELOG.md</code></a>.
    </blockquote>
</div>

<hr>
<br>

## The model

- A **[`Module`](./docs/API.md#module)** is one compilation unit: a string table, a type table (function signatures, structs with single inheritance and methods, arrays, ordered maps, cells, iterators), a constant pool, imports (host functions), globals, functions, exports, dynamic-operation hooks, and per-function debug line tables and local names.
- A **function** is a frame of typed 64-bit registers plus a flat array of **[`Inst`](./docs/API.md#inst)**ructions: 182 of them, covering moves and constants, OPS integer and float arithmetic with their policies (PHP's int-to-float `promote` included), conversions, dynamic arithmetic with a numeric fast path, dynamic truthiness and logical not, calls (direct, indirect, host, tail, dynamic), closures and cells, structs, arrays, insertion-ordered hash maps (PHP arrays), iteration, byte strings, type tests and casts, exceptions, GC safepoints, and stackful coroutines for generators and async tasks (`coro_new`, `yield`, `yield_kv`, `await`, `resume`, `resume_throw`, `coro_close`, `spawn`).
- **[`encode`](./docs/API.md#encode)** and **[`decode`](./docs/API.md#decode)** convert between a module and its canonical bytes; **[`disassemble`](./docs/API.md#disassemble)** prints it.
- **[`ModuleBuilder`](./docs/API.md#modulebuilder)** builds modules: branches take labels, strings and constants are deduplicated, and simultaneous register moves are sequentialized correctly.

The normative semantics of every instruction, the encoding, and the verifier's rules are in the LexerSketch spec `specs/LSB.md`.

<br>

What it guarantees, and how each guarantee is checked:

| Guarantee | How it is held |
|---|---|
| Encoding round-trips exactly: `decode(encode(m)) == m`. | Property tests over arbitrary modules built from every opcode, every modifier value, and every table, debug information included. |
| Each module has exactly one encoding: `encode(decode(b)) == b` for every accepted `b`. | The same tests re-encode; fuzzed and mutated inputs that decode must re-encode byte for byte; the decoder rejects non-zero unused bytes and non-canonical tags. |
| The decoder never panics and never allocates more than the input justifies. | Thousands of arbitrary, framed, and mutated inputs per run; claims of four billion entries in tiny files are refused before any allocation; a 100,000-deep constant chain is refused in linear time. |
| Decoding honours its budgets. | Property tests decode random modules under random limits and check every count of every accepted result. |
| A built function has no unresolved or out-of-range branch. | Label resolution is compared with a reference resolver on random programs; raw targets are range-checked too. |
| Parallel moves keep every value. | Random move sets, cycles included and naming the builder's own temporaries, are executed and compared with parallel assignment, and the number of moves is exactly one per move plus one per cycle. |
| `overflow = promote` never lands in a statically typed register the builder can see. | Property test over every declared register type, for dynamic and typed instructions. |
| Encoding and disassembly are deterministic. | Equal modules give equal bytes and identical listings, before and after a round trip. |

<hr>
<br>

## Installation

```toml
[dependencies]
bytecode-lang = "0.2"
```

Without the standard library:

```toml
[dependencies]
bytecode-lang = { version = "0.2", default-features = false }
```

<hr>
<br>

## Quick start

Build a function, encode it, decode it, read it:

```rust
use bytecode_lang::{decode, disassemble, encode, Inst, IntOp, IntTy, ModuleBuilder, ValType};

let mut m = ModuleBuilder::new();
let mut f = m.function("add", &[ValType::I64, ValType::I64], &[ValType::I64]);
let (a, b) = (f.param(0), f.param(1));
let sum = f.reg(ValType::I64);
f.emit(Inst::IAdd { dst: sum, lhs: a, rhs: b, op: IntOp::new(IntTy::I64) });
f.ret(sum);
m.add_function(f).unwrap();
let module = m.finish().unwrap();

let bytes = encode(&module);
let back = decode(&bytes).unwrap();
assert_eq!(back, module);
assert!(disassemble(&back).contains("  0000 iadd.i64 r2, r0, r1\n  0001 ret r2"));
```

### Loops, labels, and policies

The overflow policy is part of the instruction: `iadd.i64` raises `ArithOverflow`, `iadd.i64.wrap` wraps, on every tier.

```rust
use bytecode_lang::{disassemble, Inst, IntOp, IntTy, ModuleBuilder, Overflow, Policy, ValType};

let wrap = IntOp::new(IntTy::I64).with_policy(Policy::new().with_overflow(Overflow::Wrap));
let mut m = ModuleBuilder::new();
let mut f = m.function("hash", &[ValType::I64], &[ValType::I64]);
let n = f.param(0);
let (h, i, one, more) = (f.reg(ValType::I64), f.reg(ValType::I64), f.reg(ValType::I64), f.reg(ValType::Bool));
f.emit(Inst::LoadInt { dst: h, val: 17, ty: IntTy::I64 });
f.emit(Inst::LoadInt { dst: i, val: 0, ty: IntTy::I64 });
f.emit(Inst::LoadInt { dst: one, val: 1, ty: IntTy::I64 });
let (top, done) = (f.label(), f.label());
f.bind(top);
f.emit(Inst::ILt { dst: more, lhs: i, rhs: n, ty: IntTy::I64 });
f.jmp_if_not(more, done);
f.emit(Inst::IMul { dst: h, lhs: h, rhs: h, op: wrap });
f.emit(Inst::IAdd { dst: i, lhs: i, rhs: one, op: IntOp::new(IntTy::I64) });
f.emit(Inst::Safepoint {}); // every loop passes a safepoint: GC and fuel
f.jmp(top);
f.bind(done);
f.ret(h);
m.add_function(f).unwrap();

let text = disassemble(&m.finish().unwrap());
assert!(text.contains("jmp_if_not r4, L1"));
assert!(text.contains("imul.i64.wrap r1, r1, r1"));
assert!(text.contains("L1:\n  0009 ret r1"));
```

### Dynamic code

`dyn` registers, property access by name, and hooks that let a language define its own slow path:

```rust
use bytecode_lang::{Callee, Hook, Inst, ModuleBuilder, Policy, ValType};

let mut m = ModuleBuilder::new();
// PHP-style loose `+` on strings and arrays lives in the language runtime.
let sig = m.func_type(&[ValType::Dyn, ValType::Dyn], &[ValType::Dyn]);
let loose_add = m.import("mox.rt", "add", sig);
m.hook(Hook::Add, Callee::Import(loose_add));

let total = m.string("total");
let mut f = m.function("bump", &[ValType::Dyn, ValType::Dyn], &[ValType::Dyn]);
let (obj, amount) = (f.param(0), f.param(1));
let (old, new) = (f.reg(ValType::Dyn), f.reg(ValType::Dyn));
let name = f.name_ref(total);
f.emit(Inst::GetProp { dst: old, obj, name });
f.emit(Inst::DAdd { dst: new, lhs: old, rhs: amount, pol: Policy::new() });
f.emit(Inst::SetProp { obj, name, src: new });
f.ret(new);
m.add_function(f).unwrap();
let module = m.finish().unwrap();
assert_eq!(module.hook(Hook::Add), Some(Callee::Import(loose_add)));
```

### Hostile input

```rust
use bytecode_lang::{decode_with, DecodeErrorKind, Limit, Limits};

// A header followed by a string section claiming four billion strings.
let mut bytes = b"LSB\0\x01\0\0\0\0\0\0\0".to_vec();
bytes.extend([1, 0, 0, 0, 4, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
let mut limits = Limits::default();
limits.max_strings = 1000;
let err = decode_with(&bytes, &limits).unwrap_err();
assert_eq!(err.kind(), &DecodeErrorKind::LimitExceeded(Limit::Strings));
assert_eq!(err.to_string(), "at byte 20: string count limit exceeded");
```

<hr>
<br>

## Examples

Runnable programs in [`examples/`](./examples):

| Example | What it shows |
|---|---|
| [`quickstart`](./examples/quickstart.rs) | An iterative Fibonacci built with labels and a parallel move, encoded, decoded, and disassembled. `cargo run --example quickstart` |
| [`inspect`](./examples/inspect.rs) | Decoding a file under explicit limits and reporting where a corrupt one goes wrong. `cargo run --example inspect -- module.lsb` |

<hr>
<br>

## Performance

Encoding writes eight bytes per instruction from a value already laid out as the word; decoding validates each word with one mask test and one jump-table dispatch, and reserves every list once, after proving the input can fill it. Section lengths are computed by running the same writer against a byte counter, so the output buffer is allocated exactly once.

Measured with the benchmarks in [`benches/`](./benches) on a module of 1,000 functions × 1,000 instructions (a realistic mix: constants, typed and dynamic arithmetic, a property read, a compare and branch, a call, a safepoint), x86_64, Rust stable, release profile:

| Benchmark | What it measures | Windows |
|---|---|---:|
| `1m_insts/encode` | Encode the module (8.0 MB). | ~2.4 ms · ~419 M inst/s · ~3.2 GiB/s |
| `1m_insts/decode` | Decode it, with every check (two measurements in one run). | ~4.1–4.8 ms · ~210–245 M inst/s · ~1.6–1.9 GiB/s |
| `inst/from_bytes_1m` | Decode one million words alone, output preallocated. | ~2.2 ms · ~457 M inst/s |
| `1m_insts/build` | Build the module with the builder, labels resolved, branch targets and `promote` checked. | ~3.8–4.5 ms · ~220–260 M inst/s |
| `100k_insts/disassemble` | Disassemble 100,000 instructions to text. | ~12.3 ms · ~8.1 M inst/s |

Decoding costs about twice `from_bytes` alone: the difference is allocating and first touching the 8 MB of decoded code. Ranges are across runs on a machine shared with other builds. The builder's `overflow = promote` check, fused into its branch-target pass, costs about 0.1–0.5 ms per million instructions (≈3–12%) in interleaved measurements. These are library-only numbers; nothing here measures a VM.

```bash
cargo bench --bench bench
```

Criterion writes per-benchmark reports to `target/criterion/`. Numbers vary by CPU; use the trend across runs, not a single absolute.

<hr>
<br>

## Design notes

- **A register machine whose decoded form is its executed form.** The `Inst` enum is eight bytes, the same as its encoding, so there is no second representation to keep in sync and no parse step between loading and running.
- **Typed registers, `dyn` included.** Declared register types make GC roots exact and let the verifier check every operand in one linear pass. Dynamic languages declare `dyn` registers and use the `d*` instructions; static languages use unboxed typed registers. Mixed code converts explicitly.
- **Constants are format values.** The pool holds exact `i64`, `u64`, IEEE bit patterns, chars, UTF-8 and byte strings, and arrays and maps of earlier constants; the runtime value layout is the VM's business at load time. That is why the crate does not depend on `value-lang`, whose `i32` integers and process-local symbols cannot represent or serialize these.
- **Fixed-width, canonical encoding.** No variable-length integers: every value has one encoding, decoding is branch-light, and a decoded file re-encodes byte for byte.
- **Structure here, meaning in the verifier.** The decoder checks the format; whether an index is in range or a register is used at its type is the verifier's (0.5). Every accessor that follows an index returns `Option`, and the disassembler prints bad references as `<invalid>`.
- **No recursion anywhere input reaches.** Aggregate constants refer only to earlier constants, so their depth is computed in one forward pass; nothing else in the format nests.

<hr>
<br>

## Testing

```bash
cargo test                       # unit + integration + property + doctests
cargo test --no-default-features # no_std + alloc
cargo clippy --all-targets --all-features -- -D warnings
cargo bench --bench bench
```

The property tests in [`tests/properties.rs`](./tests/properties.rs) generate instructions from the crate's own opcode metadata (so every opcode and modifier is covered without a hand-kept list), build arbitrary modules, and check the round trip, canonicity, determinism, decoder robustness against arbitrary, framed, and mutated bytes, budgets, label resolution against a reference resolver, and parallel moves against parallel assignment. [`tests/codec.rs`](./tests/codec.rs) hand-assembles inputs for every decode error, independently of the encoder. [`tests/disasm.rs`](./tests/disasm.rs) pins the listing format with a golden file. Every `rust` example in this README and in [`docs/API.md`](./docs/API.md) is compiled and run as a doctest.

<hr>
<br>

## Cross-platform support

- Linux (x86_64, aarch64)
- macOS (x86_64, Apple Silicon)
- Windows (x86_64)

The format is little-endian by definition and the crate uses no operating-system facilities, so the same module encodes to the same bytes on every platform.

<hr>
<br>

## Contributing

See [`REPS.md`](./REPS.md) for the engineering standards every change is held to, [`dev/DIRECTIVES.md`](./dev/DIRECTIVES.md) for the definition of done, and [`dev/ROADMAP.md`](./dev/ROADMAP.md) for what comes next. Before a PR: `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-features` must be clean.

<br>

<div id="license">
    <h2>License</h2>
    <p>Licensed under either of</p>
    <ul>
        <li><b>Apache License, Version 2.0</b> &mdash; <a href="./LICENSE-APACHE">LICENSE-APACHE</a></li>
        <li><b>MIT License</b> &mdash; <a href="./LICENSE-MIT">LICENSE-MIT</a></li>
    </ul>
    <p>at your option.</p>
</div>

<div align="center">
  <h2></h2>
  <sup>COPYRIGHT <small>&copy;</small> 2026 <strong>James Gober.</strong></sup>
</div>
