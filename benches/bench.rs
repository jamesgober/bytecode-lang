//! Criterion benchmarks: encoding, decoding, building, and disassembling
//! modules of one million instructions, and binding dynamic-call arguments
//! (`ParamList::bind`) for 100,000 calls.
//!
//! ```text
//! cargo bench --bench bench
//! ```

use std::hint::black_box;

use bytecode_lang::{
    ArgItem, Const, ConstId, FuncId, FunctionBuilder, Inst, IntOp, IntTy, Module, ModuleBuilder,
    Param, ParamKind, ParamList, Policy, Reg, StrId, ValType, decode, disassemble, encode,
};
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};

const FUNCTIONS: usize = 1_000;
const PER_FUNCTION: usize = 1_000;

/// A loop body typical of generated code, repeated to `PER_FUNCTION`
/// instructions: constants, typed and dynamic arithmetic, a property read,
/// a compare and branch, a call window, a safepoint.
fn body(f: &mut FunctionBuilder, callee: FuncId, k: ConstId) {
    let regs = f.regs(&[ValType::I64; 4]);
    let dyns = f.regs(&[ValType::Dyn; 2]);
    let cond = f.reg(ValType::Bool);
    let (a, b, c, arg) = (regs, Reg(regs.0 + 1), Reg(regs.0 + 2), Reg(regs.0 + 3));
    let (x, y) = (dyns, Reg(dyns.0 + 1));
    let i64op = IntOp::new(IntTy::I64);
    let name = f.name_ref(StrId(0));
    let top = f.label();
    f.bind(top);
    let mut n = 0;
    while n + 12 < PER_FUNCTION {
        f.emit(Inst::LoadConst { dst: a, k });
        f.emit(Inst::LoadInt {
            dst: b,
            val: 7,
            ty: IntTy::I64,
        });
        f.emit(Inst::IAdd {
            dst: c,
            lhs: a,
            rhs: b,
            op: i64op,
        });
        f.emit(Inst::IMul {
            dst: c,
            lhs: c,
            rhs: b,
            op: i64op,
        });
        f.emit(Inst::DAdd {
            dst: x,
            lhs: x,
            rhs: y,
            pol: Policy::new(),
        });
        f.emit(Inst::GetProp {
            dst: y,
            obj: x,
            name,
        });
        f.emit(Inst::ILt {
            dst: cond,
            lhs: c,
            rhs: a,
            ty: IntTy::I64,
        });
        f.jmp_if(cond, top);
        f.mov(arg, c);
        f.emit(Inst::Call {
            dst: c,
            func: callee,
            argc: 1,
        });
        f.emit(Inst::Safepoint {});
        f.emit(Inst::Nop {});
        n += 12;
    }
    while n + 1 < PER_FUNCTION {
        f.emit(Inst::Nop {});
        n += 1;
    }
    f.ret_void();
}

fn module(functions: usize) -> Module {
    let mut m = ModuleBuilder::new();
    let _field = m.string("field");
    let k = m.constant(Const::Int(1 << 40));
    let builders: Vec<_> = (0..functions)
        .map(|i| m.function(&format!("f{i}"), &[], &[]))
        .collect();
    let first = builders[0].id();
    for mut f in builders {
        body(&mut f, first, k);
        m.add_function(f).unwrap_or_else(|e| panic!("{e}"));
    }
    m.finish().unwrap_or_else(|e| panic!("{e}"))
}

fn codec(c: &mut Criterion) {
    let module = module(FUNCTIONS);
    let insts: usize = module.functions().iter().map(|f| f.code().len()).sum();
    assert_eq!(insts, FUNCTIONS * PER_FUNCTION);
    let bytes = encode(&module);

    let mut g = c.benchmark_group("1m_insts");
    g.sample_size(20);
    g.throughput(Throughput::Elements(insts as u64));
    g.bench_function("encode", |b| {
        b.iter(|| black_box(encode(black_box(&module))))
    });
    g.bench_function("decode", |b| {
        b.iter(|| black_box(decode(black_box(&bytes))))
    });
    g.bench_function("build", |b| b.iter(|| black_box(self::module(FUNCTIONS))));
    g.finish();

    let mut g = c.benchmark_group("1m_insts_bytes");
    g.sample_size(20);
    g.throughput(Throughput::Bytes(bytes.len() as u64));
    g.bench_function("encode", |b| {
        b.iter(|| black_box(encode(black_box(&module))))
    });
    g.bench_function("decode", |b| {
        b.iter(|| black_box(decode(black_box(&bytes))))
    });
    g.finish();

    let words: Vec<[u8; 8]> = module
        .functions()
        .iter()
        .flat_map(|f| f.code().iter().map(Inst::to_bytes))
        .collect();
    let mut g = c.benchmark_group("inst");
    g.sample_size(20);
    g.throughput(Throughput::Elements(words.len() as u64));
    g.bench_function("from_bytes_1m", |b| {
        b.iter_batched_ref(
            || Vec::with_capacity(words.len()),
            |out: &mut Vec<Inst>| {
                for w in &words {
                    if let Ok(i) = Inst::from_bytes(*w) {
                        out.push(i);
                    }
                }
            },
            BatchSize::LargeInput,
        )
    });
    g.finish();

    let small = self::module(100);
    let mut g = c.benchmark_group("100k_insts");
    g.sample_size(20);
    g.throughput(Throughput::Elements((100 * PER_FUNCTION) as u64));
    g.bench_function("disassemble", |b| {
        b.iter(|| black_box(disassemble(black_box(&small))))
    });
    g.finish();
}

/// Binding 100,000 dynamic calls to PHP's `function f($a, $b = 1, $c = 2,
/// ...$rest)`, cycling through four call shapes: `f(1, 2)`, `f(1, c: 3)`,
/// `f(1, 2, 3, 4, 5)` (two into the rest map), and `f(c: 3, a: 1, b: 2)`.
fn binding(c: &mut Criterion) {
    let mut m = ModuleBuilder::new();
    let (a, b, cc) = (m.string("a"), m.string("b"), m.string("c"));
    let module = m.finish().unwrap_or_else(|e| panic!("{e}"));
    let list = ParamList::new(vec![
        Param::normal(a),
        Param::normal(b).with_default(),
        Param::normal(cc).with_default(),
        Param::new(ParamKind::RestMap, None),
    ]);
    let p = ArgItem::Positional;
    let calls: [&[ArgItem<'_>]; 4] = [
        &[p, p],
        &[p, ArgItem::Named(b"c")],
        &[p, p, p, p, p],
        &[
            ArgItem::Named(b"c"),
            ArgItem::Named(b"a"),
            ArgItem::Named(b"b"),
        ],
    ];
    const CALLS: usize = 100_000;
    let mut g = c.benchmark_group("bind");
    g.sample_size(20);
    g.throughput(Throughput::Elements(CALLS as u64));
    g.bench_function("php_100k_calls", |bench| {
        bench.iter(|| {
            let mut present = 0u64;
            for i in 0..CALLS {
                let items = calls[i % calls.len()];
                if let Ok(bound) = list.bind(black_box(&module), black_box(items)) {
                    present ^= bound.presence();
                }
            }
            black_box(present)
        })
    });
    g.finish();
}

criterion_group!(benches, codec, binding);
criterion_main!(benches);
