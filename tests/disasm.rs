//! The disassembler: an exact golden listing, and robustness on unverified
//! modules.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use bytecode_lang::{
    ArgKind, Callee, Const, ErrorKind, ExportItem, Field, Hook, Inst, IntOp, IntTy, Module,
    ModuleBuilder, Overflow, Param, ParamKind, ParamList, Policy, Reg, StructDef, Target, TypeDef,
    ValType, decode, disassemble, encode,
};

/// A small Mox-flavoured module touching every section.
fn sample() -> Module {
    let mut m = ModuleBuilder::new();
    m.set_name("demo");
    let file = m.string("demo.mox");
    let hello = m.string("hello");
    let k_hello = m.constant(Const::Str(hello));
    let k_big = m.constant(Const::Int(1 << 40));
    let k_list = m.constant(Const::Array(vec![k_big, k_big]));
    let point_name = m.string("Point");
    let x = m.string("x");
    let point = m.add_type(TypeDef::Struct(StructDef {
        name: point_name,
        parent: None,
        fields: vec![Field {
            name: x,
            ty: ValType::Dyn,
        }],
        methods: vec![],
    }));
    let print_sig = m.func_type(&[ValType::Str], &[]);
    let print = m.import("ls.io", "print", print_sig);
    let counter = m.global("counter", ValType::I64, true, Some(k_big));

    // fn sum(n: i64) -> i64 { let t = 0; for i in 0..n { t += i } t }
    let mut f = m.function("sum", &[ValType::I64], &[ValType::I64]);
    let n = f.param(0);
    let (t, i, more) = (
        f.reg(ValType::I64),
        f.reg(ValType::I64),
        f.reg(ValType::Bool),
    );
    let wrap = IntOp::new(IntTy::I64).with_policy(Policy::new().with_overflow(Overflow::Wrap));
    f.set_location(file, 1, 1);
    f.emit(Inst::LoadInt {
        dst: t,
        val: 0,
        ty: IntTy::I64,
    });
    f.emit(Inst::LoadInt {
        dst: i,
        val: 0,
        ty: IntTy::I64,
    });
    let (head, done) = (f.label(), f.label());
    f.bind(head);
    f.set_location(file, 2, 3);
    f.emit(Inst::ILt {
        dst: more,
        lhs: i,
        rhs: n,
        ty: IntTy::I64,
    });
    f.jmp_if_not(more, done);
    f.emit(Inst::IAdd {
        dst: t,
        lhs: t,
        rhs: i,
        op: IntOp::new(IntTy::I64),
    });
    f.emit(Inst::Safepoint {});
    f.parallel_move(&[(t, i), (i, t)]);
    f.emit(Inst::IAdd {
        dst: i,
        lhs: i,
        rhs: i,
        op: wrap,
    });
    f.jmp(head);
    f.bind(done);
    f.local(t, x, head, done);
    f.ret(t);
    let sum = m.add_function(f).unwrap();

    // fn main() { print("hello"); counter; obj.x }
    let mut f = m.function("main", &[], &[]);
    let window = f.regs(&[ValType::Dyn, ValType::Str]);
    let obj = f.reg(ValType::Dyn);
    let g = f.reg(ValType::I64);
    let err = f.reg(ValType::Dyn);
    let x_ref = f.name_ref(x);
    let point_ref = f.type_ref(point);
    let (start, end, catch) = (f.label(), f.label(), f.label());
    f.bind(start);
    f.emit(Inst::LoadConst {
        dst: Reg(window.0 + 1),
        k: k_hello,
    });
    f.emit(Inst::CallImport {
        dst: window,
        import: print,
        argc: 1,
    });
    f.emit(Inst::GetGlobal {
        dst: g,
        global: counter,
    });
    f.emit(Inst::DLoadConst {
        dst: obj,
        k: k_list,
    });
    f.emit(Inst::NewStruct {
        dst: obj,
        ty: point_ref,
    });
    f.emit(Inst::GetProp {
        dst: obj,
        obj,
        name: x_ref,
    });
    f.bind(end);
    f.ret_void();
    f.bind(catch);
    f.emit(Inst::Throw { src: err });
    f.try_region(start, end, catch, err);
    let main = m.add_function(f).unwrap();

    // PHP: function push(&$a, $k, $v = null) { $a[$k][] ...; printf(...) }
    let (a, k, v, format) = (
        m.string("a"),
        m.string("k"),
        m.string("v"),
        m.string("format"),
    );
    let printf_sig = m.func_type(&[ValType::Dyn, ValType::Dyn], &[ValType::Dyn]);
    let printf = m.import_with_params(
        "php.std",
        "printf",
        printf_sig,
        ParamList::new(vec![
            Param::normal(format),
            Param::new(ParamKind::RestMap, None),
        ]),
    );
    let d = ValType::Dyn;
    let mut f = m.function("push", &[d, d, d, ValType::I64], &[]);
    f.set_params(
        ParamList::new(vec![
            Param::normal(a).by_ref(),
            Param::normal(k),
            Param::normal(v).with_default(),
        ])
        .ignoring_extra(),
    );
    let (arr, slot, r, p, byref) = (f.reg(d), f.reg(d), f.reg(d), f.reg(d), f.reg(ValType::Bool));
    let window = f.regs(&[d, d, d]);
    let pol = Policy::new().with_overflow(Overflow::Promote);
    f.emit(Inst::CellGet {
        dst: arr,
        cell: f.param(0),
    });
    f.emit(Inst::DSepIndex {
        dst: slot,
        obj: arr,
        key: f.param(1),
    });
    f.emit(Inst::DRefIndex {
        dst: r,
        obj: slot,
        key: f.param(2),
    });
    f.emit(Inst::DPow {
        dst: p,
        lhs: f.param(1),
        rhs: f.param(1),
        pol,
    });
    f.emit(Inst::LoadImport {
        dst: window,
        import: printf,
    });
    f.emit(Inst::DParamRef {
        dst: byref,
        callee: window,
        pos: f.param(3),
    });
    f.dcall_shape(window, window, &[ArgKind::Positional, ArgKind::Named(k)]);
    f.emit(Inst::Raise {
        src: f.param(1),
        kind: ErrorKind::NoMatch,
    });
    let push = m.add_function(f).unwrap();

    m.export("main", ExportItem::Func(main));
    m.export("push", ExportItem::Func(push));
    m.hook(Hook::Add, Callee::Func(sum));
    m.set_start(main);
    m.finish().unwrap()
}

const GOLDEN: &str = include_str!("golden/sample.lsb.txt");

#[test]
fn listing_matches_the_golden_file() {
    let text = disassemble(&sample());
    if std::env::var_os("LSB_BLESS").is_some() {
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/sample.lsb.txt"),
            &text,
        )
        .unwrap();
    }
    assert_eq!(text, GOLDEN);
}

#[test]
fn listing_survives_the_round_trip() {
    let m = sample();
    assert_eq!(disassemble(&decode(&encode(&m)).unwrap()), disassemble(&m));
    assert_eq!(m.to_string(), disassemble(&m));
}

#[test]
fn invalid_references_print_as_invalid() {
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    f.emit(Inst::Call {
        dst: Reg(9),
        func: bytecode_lang::FuncId(77),
        argc: 0,
    });
    f.emit(Inst::LoadConst {
        dst: Reg(0),
        k: bytecode_lang::ConstId(5),
    });
    f.emit(Inst::GetProp {
        dst: Reg(0),
        obj: Reg(1),
        name: bytecode_lang::NameRef(4),
    });
    f.emit(Inst::Jmp { target: Target(0) });
    let module = {
        m.add_function(f).unwrap();
        m.finish().unwrap()
    };
    let text = disassemble(&module);
    assert!(text.contains("call r9, f77, 0  ; f77 <invalid>"), "{text}");
    assert!(text.contains("load_const r0, k5  ; k5 <invalid>"), "{text}");
    assert!(
        text.contains("get_prop r0, r1, n4  ; n4 <invalid>"),
        "{text}"
    );
    assert!(text.contains("jmp L0"), "{text}");
}
