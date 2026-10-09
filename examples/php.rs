//! PHP semantics in LSB format 2: a function with a dynamic-call signature
//! (a by-reference parameter, a default, a variadic), a dynamic call with a
//! spread and a named argument, a nested array write that separates only
//! what may be shared, and a reference into a map slot. Prints the listing
//! and how one dynamic call binds.
//!
//! ```text
//! cargo run --example php
//! ```

use bytecode_lang::{
    ArgItem, ArgKind, ErrorKind, Inst, ModuleBuilder, Param, ParamKind, ParamList, Prim, Reg,
    ValType, decode, disassemble, encode,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let d = ValType::Dyn;
    let mut m = ModuleBuilder::new();
    m.set_name("php");
    let (a, k, v, tag) = (m.string("a"), m.string("k"), m.string("v"), m.string("tag"));

    // function append(&$a, $k, $v = null, ...$tags) {
    //     $a[$k][] = $v;            // nested write: dsep_index, then the push
    //     $r = &$a[$k];             // a reference into the map slot
    //     match (true) {}           // no arm matches: NoMatch (E0200)
    // }
    let list = ParamList::new(vec![
        Param::normal(a).by_ref(),
        Param::normal(k),
        Param::normal(v).with_default(),
        Param::new(ParamKind::RestMap, None),
    ]);
    // The signature: one parameter per entry, plus the presence mask.
    let mut f = m.function("append", &[d, d, d, d, ValType::I64], &[]);
    f.set_params(list.clone());
    let (arr, inner, r) = (f.reg(d), f.reg(d), f.reg(d));
    f.emit(Inst::CellGet {
        dst: arr,
        cell: f.param(0),
    });
    f.emit(Inst::DSepIndex {
        dst: inner,
        obj: arr,
        key: f.param(1),
    });
    // (The push into `inner` and PHP's auto-vivification of a missing
    // `$a[$k]` are the code generator's ordinary dynamic stores.)
    f.emit(Inst::DRefIndex {
        dst: r,
        obj: arr,
        key: f.param(1),
    });
    f.emit(Inst::Raise {
        src: f.param(1),
        kind: ErrorKind::NoMatch,
    });
    let append = m.add_function(f)?;

    // function caller($f, $args) { return $f($args[0], ...$args, tag: true); }
    let mut g = m.function("caller", &[d, d], &[d]);
    let (yes, first, byref) = (g.reg(ValType::Bool), g.reg(d), g.reg(ValType::Bool));
    let pos = g.reg(ValType::I64);
    let window = g.regs(&[d, d, d, d]);
    // PHP decides per argument whether to send a reference: ask the callee.
    g.emit(Inst::LoadInt {
        dst: pos,
        val: 0,
        ty: bytecode_lang::IntTy::I64,
    });
    g.emit(Inst::DParamRef {
        dst: byref,
        callee: g.param(0),
        pos,
    });
    g.emit(Inst::DLoadInt { dst: first, val: 0 });
    let (by_value, sent) = (g.label(), g.label());
    g.jmp_if_not(byref, by_value);
    g.emit(Inst::DRefIndex {
        dst: Reg(window.0 + 1),
        obj: g.param(1),
        key: first,
    });
    g.jmp(sent);
    g.bind(by_value);
    g.emit(Inst::DGetIndex {
        dst: Reg(window.0 + 1),
        obj: g.param(1),
        key: first,
    });
    g.bind(sent);
    g.mov(Reg(window.0 + 2), g.param(1));
    g.emit(Inst::LoadBool {
        dst: yes,
        val: true,
    });
    g.emit(Inst::ToDyn {
        dst: Reg(window.0 + 3),
        src: yes,
        from: Prim::Bool,
    });
    g.dcall_shape(
        window,
        g.param(0),
        &[ArgKind::Positional, ArgKind::Spread, ArgKind::Named(tag)],
    );
    g.ret(window);
    m.add_function(g)?;

    let module = m.finish()?;
    let bytes = encode(&module);
    let back = decode(&bytes)?;
    assert_eq!(back, module);
    print!("{}", disassemble(&back));

    // How `append(&$x, 'k', tag: true)` binds: `$v` keeps its default, the
    // unknown named argument lands in the variadic map under "tag".
    let items = [
        ArgItem::Positional,
        ArgItem::Positional,
        ArgItem::Named(b"tag"),
    ];
    let bound = list.bind(&back, &items)?;
    println!(
        "\n; append binding: {:?}, presence mask {:#06b}",
        bound.slots(),
        bound.presence()
    );
    let params = back
        .function(append)
        .and_then(|f| f.params())
        .map(|p| p.to_string());
    println!("; append params: {}", params.unwrap_or_default());
    println!("; {} bytes", bytes.len());
    Ok(())
}
