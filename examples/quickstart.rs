//! The Tier-1 path: build a module, encode it, decode it, disassemble it.
//!
//! ```text
//! cargo run --example quickstart
//! ```

use bytecode_lang::{
    Const, ExportItem, Inst, IntOp, IntTy, ModuleBuilder, ValType, decode, disassemble, encode,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut m = ModuleBuilder::new();
    m.set_name("quickstart");

    // fn fib(n: i64) -> i64, iteratively.
    let mut f = m.function("fib", &[ValType::I64], &[ValType::I64]);
    let n = f.param(0);
    let (a, b, i, one, more) = (
        f.reg(ValType::I64),
        f.reg(ValType::I64),
        f.reg(ValType::I64),
        f.reg(ValType::I64),
        f.reg(ValType::Bool),
    );
    let add = IntOp::new(IntTy::I64);
    f.emit(Inst::LoadInt {
        dst: a,
        val: 0,
        ty: IntTy::I64,
    });
    f.emit(Inst::LoadInt {
        dst: b,
        val: 1,
        ty: IntTy::I64,
    });
    f.emit(Inst::LoadInt {
        dst: i,
        val: 0,
        ty: IntTy::I64,
    });
    f.emit(Inst::LoadInt {
        dst: one,
        val: 1,
        ty: IntTy::I64,
    });
    let (head, done) = (f.label(), f.label());
    f.bind(head);
    f.emit(Inst::ILt {
        dst: more,
        lhs: i,
        rhs: n,
        ty: IntTy::I64,
    });
    f.jmp_if_not(more, done);
    // (a, b) = (b, a + b): compute a + b into a, then swap a and b.
    f.emit(Inst::IAdd {
        dst: a,
        lhs: a,
        rhs: b,
        op: add,
    });
    f.parallel_move(&[(a, b), (b, a)]);
    f.emit(Inst::IAdd {
        dst: i,
        lhs: i,
        rhs: one,
        op: add,
    });
    f.emit(Inst::Safepoint {});
    f.jmp(head);
    f.bind(done);
    f.ret(a);
    let fib = m.add_function(f)?;
    m.export("fib", ExportItem::Func(fib));

    // A constant-pool entry, so the listing shows the pool too.
    let _answer = m.constant(Const::Int(42));

    let module = m.finish()?;
    let bytes = encode(&module);
    let back = decode(&bytes)?;
    assert_eq!(back, module);

    println!("{} bytes", bytes.len());
    print!("{}", disassemble(&back));
    Ok(())
}
