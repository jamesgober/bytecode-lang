//! The builders: every error path, deduplication, and the tables they fill.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use bytecode_lang::{
    BuildError, Callee, Const, ConstId, ExportItem, FuncId, Hook, Inst, IntTy, JumpTable, LineRow,
    LocalVar, ModuleBuilder, Reg, StrId, StructDef, Target, TypeDef, TypeId, ValType,
};

#[test]
fn unbound_rebound_and_foreign_labels_are_errors() {
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    let l = f.label();
    f.jmp(l);
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::UnboundLabel {
            func: FuncId(0),
            label: 0
        })
    );

    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    let l = f.label();
    f.bind(l);
    f.emit(Inst::Nop {});
    f.bind(l);
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::LabelRebound {
            func: FuncId(0),
            label: 0
        })
    );

    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    let mut g = m.function("g", &[], &[]);
    let theirs = g.label();
    f.jmp(theirs);
    f.ret_void();
    g.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::ForeignLabel { func: FuncId(0) })
    );
    assert!(m.add_function(g).is_ok());
}

#[test]
fn a_label_bound_after_the_last_instruction_is_not_a_branch_target() {
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    let end = f.label();
    f.jmp(end);
    f.bind(end);
    assert_eq!(
        m.add_function(f),
        Err(BuildError::TargetOutOfRange {
            func: FuncId(0),
            target: 1
        })
    );

    // ...but it is a valid range end.
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    let err = f.reg(ValType::Dyn);
    let (start, end) = (f.label(), f.label());
    f.bind(start);
    f.ret_void();
    f.bind(end);
    f.try_region(start, end, start, err);
    let id = m.add_function(f).unwrap();
    let module = m.finish().unwrap();
    assert_eq!(module.function(id).unwrap().handlers()[0].end, 1);
}

#[test]
fn raw_targets_are_range_checked() {
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::Bool], &[]);
    f.emit(Inst::JmpIfNot {
        cond: Reg(0),
        target: Target(1),
    });
    f.ret_void();
    assert!(m.add_function(f).is_ok());

    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::Bool], &[]);
    f.emit(Inst::JmpIfNot {
        cond: Reg(0),
        target: Target(2),
    });
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::TargetOutOfRange {
            func: FuncId(0),
            target: 2
        })
    );
}

#[test]
fn inverted_ranges_are_errors() {
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    let err = f.reg(ValType::Dyn);
    let (a, b) = (f.label(), f.label());
    f.bind(a);
    f.emit(Inst::Nop {});
    f.bind(b);
    f.ret_void();
    f.try_region(b, a, a, err);
    assert_eq!(
        m.add_function(f),
        Err(BuildError::InvalidRange { func: FuncId(0) })
    );

    let mut m = ModuleBuilder::new();
    let name = m.string("x");
    let mut f = m.function("f", &[], &[]);
    let (a, b) = (f.label(), f.label());
    f.bind(a);
    f.emit(Inst::Nop {});
    f.bind(b);
    f.ret_void();
    f.local(Reg(0), name, b, a);
    assert_eq!(
        m.add_function(f),
        Err(BuildError::InvalidRange { func: FuncId(0) })
    );
}

#[test]
fn per_function_tables_are_bounded() {
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    let last = f.regs(&vec![ValType::I64; 65_536]);
    assert_eq!(last, Reg(0));
    assert_eq!(f.reg(ValType::I64), Reg(u16::MAX)); // the 65,537th
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::TooMany {
            func: FuncId(0),
            what: "registers"
        })
    );

    let mut m = ModuleBuilder::new();
    let f = m.function("f", &vec![ValType::I64; 256], &[]);
    assert_eq!(
        m.add_function(f),
        Err(BuildError::TooMany {
            func: FuncId(0),
            what: "parameters"
        })
    );

    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    for _ in 0..=65_536 {
        let _ = f.capture(ValType::Dyn);
    }
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::TooMany {
            func: FuncId(0),
            what: "captures"
        })
    );

    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    for i in 0..=65_536u32 {
        let _ = f.name_ref(StrId(i));
    }
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::TooMany {
            func: FuncId(0),
            what: "name refs"
        })
    );

    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    for i in 0..=65_536u32 {
        let _ = f.type_ref(TypeId(i));
    }
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::TooMany {
            func: FuncId(0),
            what: "type refs"
        })
    );
}

#[test]
fn a_move_cycle_through_an_undeclared_register_is_an_error() {
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    f.parallel_move(&[(Reg(7), Reg(8)), (Reg(8), Reg(7))]);
    f.ret_void();
    assert!(matches!(
        m.add_function(f),
        Err(BuildError::UnknownRegister { .. })
    ));
}

#[test]
fn the_h01_back_edge_permutation_is_correct() {
    // `jump H(b, a)` from a loop whose header takes (a, b): the parameters
    // swap. Sequential moves (codegen-lang 1.x) lost a value here.
    let mut m = ModuleBuilder::new();
    let mut f = m.function("loop", &[ValType::I64, ValType::I64, ValType::I64], &[]);
    let (a, b, c) = (f.param(0), f.param(1), f.param(2));
    // A three-way rotation plus an independent copy.
    f.parallel_move(&[(a, b), (b, c), (c, a)]);
    f.ret_void();
    let id = m.add_function(f).unwrap();
    let module = m.finish().unwrap();
    let code = module.function(id).unwrap().code();
    let mut regs = [10u64, 20, 30, 0];
    for inst in code {
        if let Inst::Mov { dst, src } = *inst {
            regs[dst.index()] = regs[src.index()];
        }
    }
    assert_eq!(&regs[..3], &[20, 30, 10]);
    assert_eq!(code.len(), 5); // three moves, one to save the cycle, ret_void
}

#[test]
fn foreign_and_undefined_functions_are_errors() {
    let mut a = ModuleBuilder::new();
    let mut b = ModuleBuilder::new();
    let _taken = b.function("other", &[], &[]);
    let mut f = b.function("f", &[], &[]);
    f.ret_void();
    // `a` has no slot 1.
    assert_eq!(
        a.add_function(f),
        Err(BuildError::ForeignFunction(FuncId(1)))
    );

    let mut m = ModuleBuilder::new();
    let _never_added = m.function("f", &[], &[]);
    assert_eq!(
        m.finish().unwrap_err(),
        BuildError::UndefinedFunction(FuncId(0))
    );
}

#[test]
fn types_must_be_defined_once() {
    let mut m = ModuleBuilder::new();
    let _t = m.reserve_type();
    assert_eq!(
        m.finish().unwrap_err(),
        BuildError::UndefinedType(TypeId(0))
    );

    let mut m = ModuleBuilder::new();
    let t = m.reserve_type();
    m.define_type(t, TypeDef::Cell(ValType::Dyn));
    m.define_type(t, TypeDef::Cell(ValType::Dyn));
    assert_eq!(m.finish().unwrap_err(), BuildError::UndefinedType(t));

    let mut m = ModuleBuilder::new();
    m.define_type(TypeId(3), TypeDef::Cell(ValType::Dyn));
    assert_eq!(
        m.finish().unwrap_err(),
        BuildError::UndefinedType(TypeId(3))
    );
}

#[test]
fn constants_may_only_refer_backwards() {
    let mut m = ModuleBuilder::new();
    let _bad = m.constant(Const::Array(vec![ConstId(0)]));
    assert_eq!(
        m.finish().unwrap_err(),
        BuildError::ConstForwardRef(ConstId(0))
    );
}

#[test]
fn strings_structural_types_and_constants_are_deduplicated() {
    let mut m = ModuleBuilder::new();
    assert_eq!(m.string("x"), m.string("x"));
    let sig = m.func_type(&[ValType::Dyn], &[]);
    assert_eq!(
        m.add_type(TypeDef::Func(bytecode_lang::FuncType {
            params: vec![ValType::Dyn],
            results: vec![]
        })),
        sig
    );
    let s1 = m.add_type(TypeDef::Struct(StructDef::default()));
    let s2 = m.add_type(TypeDef::Struct(StructDef::default()));
    assert_ne!(s1, s2);
    // Floats deduplicate by bit pattern: 0.0 and -0.0 stay distinct.
    let zero = m.constant(Const::f64(0.0));
    let neg_zero = m.constant(Const::f64(-0.0));
    assert_ne!(zero, neg_zero);
    assert_eq!(m.constant(Const::f64(0.0)), zero);
    let module = m.finish().unwrap();
    assert_eq!(module.string_count(), 1);
    assert_eq!(module.types().len(), 3);
    assert_eq!(module.consts().len(), 2);
}

#[test]
fn functions_may_call_each_other_before_they_are_added() {
    let mut m = ModuleBuilder::new();
    let mut even = m.function("even", &[ValType::I64], &[ValType::Bool]);
    let mut odd = m.function("odd", &[ValType::I64], &[ValType::Bool]);
    let (even_id, odd_id) = (even.id(), odd.id());
    for (f, other) in [(&mut even, odd_id), (&mut odd, even_id)] {
        let window = f.regs(&[ValType::Bool, ValType::I64]);
        f.mov(Reg(window.0 + 1), f.param(0));
        f.emit(Inst::Call {
            dst: window,
            func: other,
            argc: 1,
        });
        f.ret(window);
    }
    // Added in reverse order: slots were fixed at declaration.
    assert_eq!(m.add_function(odd), Ok(odd_id));
    assert_eq!(m.add_function(even), Ok(even_id));
    let module = m.finish().unwrap();
    assert_eq!(
        module.function(even_id).unwrap().code()[1],
        Inst::Call {
            dst: Reg(1),
            func: odd_id,
            argc: 1
        }
    );
}

#[test]
fn tables_handlers_lines_and_locals_are_resolved() {
    let mut m = ModuleBuilder::new();
    let file = m.string("main.mox");
    let x = m.string("x");
    let mut f = m.function("f", &[ValType::I64], &[]);
    let err = f.reg(ValType::Dyn);
    let (case0, case1, other, start, end, handler) = (
        f.label(),
        f.label(),
        f.label(),
        f.label(),
        f.label(),
        f.label(),
    );
    f.set_location(file, 1, 1);
    f.bind(start);
    f.switch(IntTy::I64, f.param(0), &[case0, case1], other);
    f.set_location(file, 2, 5);
    f.bind(case0);
    f.emit(Inst::Nop {});
    f.bind(case1);
    f.emit(Inst::Nop {});
    f.bind(end);
    f.bind(other);
    f.ret_void();
    f.bind(handler);
    f.ret_void();
    f.try_region(start, end, handler, err);
    f.local(f.param(0), x, start, end);
    let id = m.add_function(f).unwrap();
    let module = m.finish().unwrap();
    let func = module.function(id).unwrap();
    assert_eq!(
        func.tables(),
        &[JumpTable {
            targets: vec![Target(1), Target(2)],
            default: Target(3)
        }]
    );
    assert_eq!(func.handlers()[0].start, 0);
    assert_eq!(func.handlers()[0].end, 3);
    assert_eq!(func.handlers()[0].target, Target(4));
    assert_eq!(
        func.lines(),
        &[
            LineRow {
                pc: 0,
                file,
                line: 1,
                column: 1
            },
            LineRow {
                pc: 1,
                file,
                line: 2,
                column: 5
            },
        ]
    );
    assert_eq!(
        func.locals(),
        &[LocalVar {
            reg: Reg(0),
            name: x,
            start: 0,
            end: 3
        }]
    );
}

#[test]
fn module_level_tables_are_kept() {
    let mut m = ModuleBuilder::new();
    let sig = m.func_type(&[ValType::Dyn, ValType::Dyn], &[ValType::Dyn]);
    let imp = m.import("mox.rt", "concat", sig);
    let g = m.global("count", ValType::I64, true, None);
    let mut f = m.function("main", &[], &[]);
    f.ret_void();
    let main = m.add_function(f).unwrap();
    m.export("main", ExportItem::Func(main));
    m.export("count", ExportItem::Global(g));
    m.hook(Hook::Concat, Callee::Func(main));
    m.hook(Hook::Concat, Callee::Import(imp)); // replaces
    m.hook(Hook::Add, Callee::Import(imp));
    m.set_name("app");
    m.set_start(main);
    let module = m.finish().unwrap();
    assert_eq!(module.hook(Hook::Concat), Some(Callee::Import(imp)));
    let hooks: Vec<Hook> = module.hooks().iter().map(|b| b.hook).collect();
    assert_eq!(hooks, [Hook::Add, Hook::Concat]); // sorted by code
    assert_eq!(module.exports().len(), 2);
    assert_eq!(module.start(), Some(main));
    assert_eq!(module.import(imp).unwrap().sig, sig);
    assert_eq!(module.global(g).unwrap().ty, ValType::I64);
}

#[test]
fn build_errors_describe_the_problem() {
    assert_eq!(
        BuildError::UnboundLabel {
            func: FuncId(2),
            label: 4
        }
        .to_string(),
        "label 4 of f2 is used but never bound"
    );
    assert_eq!(
        BuildError::TargetOutOfRange {
            func: FuncId(0),
            target: 9
        }
        .to_string(),
        "branch target @9 is past the end of f0"
    );
    assert_eq!(
        BuildError::TooLarge { section: 6 }.to_string(),
        "section 6 would exceed 4 GiB"
    );
}

#[test]
fn promote_requires_a_dyn_destination() {
    use bytecode_lang::{IntConv, IntOp, Overflow, Policy};
    let promote = Policy::new().with_overflow(Overflow::Promote);

    // On a dynamic add: fine.
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::Dyn, ValType::Dyn], &[ValType::Dyn]);
    let out = f.reg(ValType::Dyn);
    f.emit(Inst::DAdd {
        dst: out,
        lhs: f.param(0),
        rhs: f.param(1),
        pol: promote,
    });
    f.ret(out);
    assert!(m.add_function(f).is_ok());

    // A dynamic multiply writing a static register: refused, with the pc.
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::Dyn, ValType::Dyn], &[]);
    let out = f.reg(ValType::I64);
    f.emit(Inst::Nop {});
    f.emit(Inst::DMul {
        dst: out,
        lhs: f.param(0),
        rhs: f.param(1),
        pol: promote,
    });
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::PromoteNotDynamic {
            func: FuncId(0),
            pc: 1
        })
    );

    // Typed integer instructions always write static registers.
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::I64], &[]);
    let x = f.param(0);
    let op = IntOp::new(IntTy::I64).with_policy(promote);
    f.emit(Inst::IAdd {
        dst: x,
        lhs: x,
        rhs: x,
        op,
    });
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::PromoteNotDynamic {
            func: FuncId(0),
            pc: 0
        })
    );

    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::I64], &[]);
    let small = f.reg(ValType::I8);
    let conv = IntConv::new(IntTy::I64, IntTy::I8, Overflow::Promote);
    f.emit(Inst::IntCast {
        dst: small,
        src: f.param(0),
        conv,
    });
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::PromoteNotDynamic {
            func: FuncId(0),
            pc: 0
        })
    );

    // An undeclared destination is the verifier's concern, not the builder's.
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    f.emit(Inst::DAdd {
        dst: Reg(40),
        lhs: Reg(41),
        rhs: Reg(42),
        pol: promote,
    });
    f.ret_void();
    assert!(m.add_function(f).is_ok());
    assert_eq!(
        BuildError::PromoteNotDynamic {
            func: FuncId(1),
            pc: 3
        }
        .to_string(),
        "instruction 3 of f1 uses overflow = promote but its destination is not dyn"
    );
}

/// Runs the `mov`s of `code` on a register file of `n` slots holding
/// `100 + index`, and returns it.
fn run_moves(code: &[Inst], n: usize) -> Vec<u64> {
    let mut regs: Vec<u64> = (0..n as u64).map(|i| 100 + i).collect();
    for inst in code {
        if let Inst::Mov { dst, src } = *inst {
            regs[dst.index()] = regs[src.index()];
        }
    }
    regs
}

#[test]
fn parallel_move_never_uses_a_named_register_as_its_temporary() {
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::I64, ValType::I64], &[]);
    let (a, b) = (f.param(0), f.param(1));
    // The first swap declares the cached i64 temporary: r2.
    f.parallel_move(&[(a, b), (b, a)]);
    let first_len = f.pc() as usize;
    // The second swap names r2 itself. Reusing r2 as the temporary would
    // clobber it; the builder must pick another register.
    let t = Reg(2);
    f.parallel_move(&[(a, t), (t, a)]);
    f.ret_void();
    let id = m.add_function(f).unwrap();
    let module = m.finish().unwrap();
    let func = module.function(id).unwrap();
    assert_eq!(func.regs().len(), 4, "a second temporary, r3, was declared");
    let second = &func.code()[first_len..func.code().len() - 1];
    // r0 and r2 swapped; r1 untouched by the second set.
    let regs = run_moves(second, 4);
    assert_eq!((regs[0], regs[1], regs[2]), (102, 101, 100));
    assert!(second.contains(&Inst::Mov {
        dst: Reg(3),
        src: Reg(0)
    }));
}

#[test]
fn parallel_move_skips_undeclared_registers_when_declaring_a_temporary() {
    // The move set names r2 and r3, which are not declared: a fresh
    // temporary would be numbered r2, then r3, so both are skipped and the
    // temporary is r4.
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::I64, ValType::I64], &[]);
    let (a, b) = (f.param(0), f.param(1));
    f.parallel_move(&[(a, b), (b, a), (Reg(2), Reg(3))]);
    f.ret_void();
    let id = m.add_function(f).unwrap();
    let module = m.finish().unwrap();
    let func = module.function(id).unwrap();
    assert_eq!(func.regs().len(), 5);
    let regs = run_moves(func.code(), 5);
    assert_eq!((regs[0], regs[1], regs[2]), (101, 100, 103));
}

#[test]
fn coroutine_instructions_build_and_round_trip() {
    use bytecode_lang::{Callee, CoroState, Hook, decode, disassemble, encode};
    let mut m = ModuleBuilder::new();
    let coro_ty = m.add_type(TypeDef::Coroutine);
    let sig = m.func_type(&[ValType::Dyn], &[ValType::Dyn]);
    let scheduler = m.import("ls.task", "spawn", sig);
    m.hook(Hook::Spawn, Callee::Import(scheduler));

    // A generator: yields its argument, then returns nil.
    let mut generator = m.function("gen", &[ValType::Dyn], &[ValType::Dyn]);
    let got = generator.reg(ValType::Dyn);
    generator.emit(Inst::Yield {
        dst: got,
        src: generator.param(0),
    });
    generator.emit(Inst::LoadNil { dst: got });
    generator.ret(got);
    let gen_id = generator.id();
    m.add_function(generator).unwrap();

    // The consumer: drives it, checks its state, awaits, and spawns a task.
    let mut f = m.function("main", &[ValType::Dyn], &[]);
    let window = f.regs(&[ValType::Ref(coro_ty), ValType::Dyn]);
    let value = f.reg(ValType::Dyn);
    let state = f.reg(ValType::U8);
    let flag = f.reg(ValType::Bool);
    let me = f.reg(ValType::Dyn);
    let task = f.regs(&[ValType::Dyn, ValType::Dyn]);
    f.mov(Reg(window.0 + 1), f.param(0));
    f.emit(Inst::CoroNew {
        dst: window,
        func: gen_id,
        argc: 1,
    });
    f.emit(Inst::Resume {
        dst: value,
        coro: window,
        src: value,
    });
    f.emit(Inst::CoroStatus {
        dst: state,
        coro: window,
    });
    f.emit(Inst::ResumeThrow {
        dst: value,
        coro: window,
        src: value,
    });
    f.emit(Inst::CoroCurrent { dst: me });
    f.emit(Inst::Await {
        dst: value,
        src: value,
    });
    f.emit(Inst::CoroNewIndirect {
        dst: me,
        callee: value,
        argc: 0,
    });
    f.emit(Inst::Spawn {
        dst: task,
        callee: value,
        argc: 1,
    });
    f.emit(Inst::DNot {
        dst: flag,
        src: value,
    });
    f.emit(Inst::YieldKv {
        dst: value,
        key: me,
        src: value,
    });
    f.emit(Inst::CoroClose {
        dst: value,
        coro: window,
        src: value,
    });
    f.emit(Inst::CoroKey {
        dst: me,
        coro: window,
    });
    f.emit(Inst::CoroResult {
        dst: value,
        coro: window,
    });
    f.ret_void();
    m.add_function(f).unwrap();
    let module = m.finish().unwrap();

    let bytes = encode(&module);
    assert_eq!(decode(&bytes).unwrap(), module);
    let text = disassemble(&module);
    for line in [
        "yield r1, r0",
        "coro_new r1, f0, 1",
        "resume r3, r1, r3",
        "coro_status r4, r1",
        "resume_throw r3, r1, r3",
        "coro_current r6",
        "await r3, r3",
        "coro_new_indirect r6, r3, 0",
        "spawn r7, r3, 1",
        "dnot r5, r3",
        "yield_kv r3, r6, r3",
        "coro_close r3, r1, r3",
        "coro_key r6, r1",
        "coro_result r3, r1",
        "t0 coroutine",
        "spawn = imp0",
    ] {
        assert!(text.contains(line), "missing `{line}` in\n{text}");
    }
    assert_eq!(CoroState::from_code(2), Some(CoroState::Yielded));
}

#[test]
fn parameter_lists_are_checked_against_their_rules_and_signature() {
    use bytecode_lang::{Param, ParamError, ParamKind, ParamList};
    let d = ValType::Dyn;
    let build = |params: &[ValType], list: ParamList| {
        let mut m = ModuleBuilder::new();
        let mut f = m.function("f", params, &[]);
        f.set_params(list);
        f.ret_void();
        m.add_function(f)
    };
    let invalid = |error| {
        Err(BuildError::InvalidParams {
            owner: Callee::Func(FuncId(0)),
            error,
        })
    };
    // Rules of the list itself.
    let unnamed = ParamList::new(vec![Param::new(ParamKind::NamedOnly, None)]);
    assert_eq!(
        build(&[d], unnamed),
        invalid(ParamError::Unnamed { index: 0 })
    );
    let backwards = ParamList::new(vec![
        Param::new(ParamKind::RestNamed, None),
        Param::new(ParamKind::Rest, None),
    ]);
    assert_eq!(
        build(&[d, d], backwards),
        invalid(ParamError::OutOfOrder { index: 1 })
    );
    // Fit to the signature: count, presence mask, rest and by-ref types.
    let x = StrId(0);
    let one = ParamList::new(vec![Param::normal(x)]);
    assert_eq!(
        build(&[d, d], one.clone()),
        invalid(ParamError::Signature { index: 1 })
    );
    assert!(build(&[ValType::I32], one).is_ok()); // by value: any type converts
    let optional = ParamList::new(vec![Param::normal(x).with_default()]);
    assert_eq!(
        build(&[d], optional.clone()),
        invalid(ParamError::Signature { index: 1 })
    );
    assert!(build(&[d, ValType::I64], optional).is_ok());
    let rest = ParamList::new(vec![Param::new(ParamKind::Rest, None)]);
    assert_eq!(
        build(&[ValType::Str], rest),
        invalid(ParamError::Signature { index: 0 })
    );
    let by_ref = ParamList::new(vec![Param::normal(x).by_ref()]);
    assert_eq!(
        build(&[ValType::I64], by_ref.clone()),
        invalid(ParamError::Signature { index: 0 })
    );
    assert!(build(&[ValType::Ref(TypeId(0))], by_ref).is_ok());
    // A function without a list keeps exact positional arity.
    let mut m = ModuleBuilder::new();
    let mut f = m.function("plain", &[d], &[]);
    f.ret_void();
    let id = m.add_function(f).unwrap();
    assert!(m.finish().unwrap().function(id).unwrap().params().is_none());
}

#[test]
fn imports_take_parameter_lists_checked_against_their_signature() {
    use bytecode_lang::{Param, ParamError, ParamKind, ParamList};
    let mut m = ModuleBuilder::new();
    let format = m.string("format");
    let sig = m.func_type(&[ValType::Dyn, ValType::Dyn], &[ValType::Dyn]);
    let list = ParamList::new(vec![
        Param::normal(format),
        Param::new(ParamKind::RestMap, None),
    ]);
    let printf = m.import_with_params("php", "printf", sig, list.clone());
    let module = m.finish().unwrap();
    assert_eq!(module.import(printf).unwrap().params, Some(list.clone()));
    let bytes = bytecode_lang::encode(&module);
    assert_eq!(bytecode_lang::decode(&bytes).unwrap(), module);

    // Too few signature parameters for the list.
    let mut m = ModuleBuilder::new();
    let short = m.func_type(&[ValType::Dyn], &[]);
    let id = m.import_with_params("php", "printf", short, list.clone());
    assert_eq!(
        m.finish().unwrap_err(),
        BuildError::InvalidParams {
            owner: Callee::Import(id),
            error: ParamError::Signature { index: 1 }
        }
    );
    // A signature that is not a function type.
    let mut m = ModuleBuilder::new();
    let cell = m.add_type(TypeDef::Cell(ValType::Dyn));
    let id = m.import_with_params("php", "printf", cell, list);
    assert_eq!(
        m.finish().unwrap_err(),
        BuildError::InvalidParams {
            owner: Callee::Import(id),
            error: ParamError::Signature { index: 0 }
        }
    );
}

#[test]
fn call_shapes_are_deduplicated_checked_and_bounded() {
    use bytecode_lang::{ArgKind, ShapeError, ShapeId};
    let mut m = ModuleBuilder::new();
    let (x, y) = (m.string("x"), m.string("y"));
    let mut f = m.function("f", &[ValType::Dyn], &[]);
    let a = f.call_shape(&[ArgKind::Positional, ArgKind::Named(x)]);
    let b = f.call_shape(&[ArgKind::Spread]);
    assert_eq!((a, b), (ShapeId(0), ShapeId(1)));
    assert_eq!(f.call_shape(&[ArgKind::Positional, ArgKind::Named(x)]), a);
    let window = f.regs(&[ValType::Dyn, ValType::Dyn, ValType::Dyn]);
    let pc = f.dcall_shape(
        window,
        f.param(0),
        &[ArgKind::Named(y), ArgKind::SpreadNamed],
    );
    assert_eq!(pc, 0);
    f.ret_void();
    let id = m.add_function(f).unwrap();
    let module = m.finish().unwrap();
    let func = module.function(id).unwrap();
    assert_eq!(func.shapes().len(), 3);
    assert_eq!(
        func.code()[0],
        Inst::DCallShape {
            dst: window,
            callee: Reg(0),
            shape: ShapeId(2)
        }
    );

    // A malformed shape is reported when the function is added.
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[], &[]);
    let _bad = f.call_shape(&[ArgKind::SpreadNamed, ArgKind::Positional]);
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::InvalidShape {
            func: FuncId(0),
            error: ShapeError::PositionalAfterNamed { index: 1 }
        })
    );

    // 65,536 shapes fit a 16-bit id; the next one does not.
    let mut m = ModuleBuilder::new();
    let names: Vec<StrId> = (0..=65_536u32).map(|i| m.string(&i.to_string())).collect();
    let mut f = m.function("f", &[], &[]);
    for &name in &names {
        let _ = f.call_shape(&[ArgKind::Named(name)]);
    }
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::TooMany {
            func: FuncId(0),
            what: "call shapes"
        })
    );
}

#[test]
fn new_build_errors_describe_the_problem() {
    use bytecode_lang::{ParamError, ShapeError};
    assert_eq!(
        BuildError::InvalidParams {
            owner: Callee::Func(FuncId(3)),
            error: ParamError::Unnamed { index: 1 }
        }
        .to_string(),
        "the parameter list of f3 is invalid: parameter 1 can be named but has no name"
    );
    assert_eq!(
        BuildError::InvalidShape {
            func: FuncId(0),
            error: ShapeError::DuplicateName { index: 2 }
        }
        .to_string(),
        "a call shape of f0 is invalid: argument 2 repeats an earlier name"
    );
}

#[test]
fn promote_on_the_new_dynamic_arithmetic_needs_a_dyn_destination() {
    use bytecode_lang::{Overflow, Policy};
    let promote = Policy::new().with_overflow(Overflow::Promote);
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::Dyn], &[]);
    let typed = f.reg(ValType::I64);
    f.emit(Inst::DAbs {
        dst: typed,
        src: f.param(0),
        pol: promote,
    });
    f.ret_void();
    assert_eq!(
        m.add_function(f),
        Err(BuildError::PromoteNotDynamic {
            func: FuncId(0),
            pc: 0
        })
    );
    let mut m = ModuleBuilder::new();
    let mut f = m.function("f", &[ValType::Dyn], &[ValType::Dyn]);
    let out = f.reg(ValType::Dyn);
    f.emit(Inst::DPow {
        dst: out,
        lhs: f.param(0),
        rhs: f.param(0),
        pol: promote,
    });
    f.ret(out);
    assert!(m.add_function(f).is_ok());
}
