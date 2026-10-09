//! The dynamic-call binding rule (`ParamList::bind`, `specs/LSB.md` §5.15)
//! against an independently written reference, and the by-reference
//! queries (`dparam_ref`, `dparam_ref_named`) against the binding.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use bytecode_lang::{
    ArgItem, BindError, Bound, Module, ModuleBuilder, Param, ParamKind, ParamList, StrId,
};
use proptest::prelude::*;

/// A module whose strings `s0..s5` are the names `p0..p5`, which
/// `common::param_list(_, 0)` gives the parameters.
fn names_module() -> Module {
    let mut m = ModuleBuilder::new();
    for i in 0..6 {
        let _ = m.string(&format!("p{i}"));
    }
    m.finish().unwrap()
}

/// An argument item drawn from: positional, a parameter's name, or an
/// unknown name.
#[derive(Clone, Debug)]
enum Item {
    Pos,
    Name(String),
}

fn item() -> impl Strategy<Value = Item> {
    prop_oneof![
        3 => Just(Item::Pos),
        3 => (0u8..6).prop_map(|i| Item::Name(format!("p{i}"))),
        1 => prop::sample::select(vec!["zz", "q"]).prop_map(|s| Item::Name(s.to_string())),
    ]
}

fn as_items(items: &[Item]) -> Vec<ArgItem<'_>> {
    items
        .iter()
        .map(|i| match i {
            Item::Pos => ArgItem::Positional,
            Item::Name(n) => ArgItem::Named(n.as_bytes()),
        })
        .collect()
}

/// The reference binder: the rule as the spec states it, written from
/// scratch with a queue of positional parameters, a name map, and a set of
/// collected names.
fn reference(
    list: &ParamList,
    module: &Module,
    items: &[Item],
) -> Result<(Vec<Bound>, u64), BindError> {
    let name_of = |p: &Param| p.name.and_then(|s| module.string(s)).map(str::to_string);
    let mut queue: VecDeque<usize> = list
        .params
        .iter()
        .enumerate()
        .filter(|(_, p)| matches!(p.kind, ParamKind::PositionalOnly | ParamKind::Normal))
        .map(|(i, _)| i)
        .collect();
    let by_name: BTreeMap<String, usize> = list
        .params
        .iter()
        .enumerate()
        .filter(|(_, p)| matches!(p.kind, ParamKind::Normal | ParamKind::NamedOnly))
        .filter_map(|(i, p)| name_of(p).map(|n| (n, i)))
        .collect();
    let rest = list
        .params
        .iter()
        .position(|p| matches!(p.kind, ParamKind::Rest | ParamKind::RestMap));
    let named_rest = list
        .params
        .iter()
        .position(|p| p.kind == ParamKind::RestNamed)
        .or_else(|| {
            list.params
                .iter()
                .position(|p| p.kind == ParamKind::RestMap)
        });
    let mut filled: BTreeMap<usize, usize> = BTreeMap::new();
    let mut collected: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut collected_names: BTreeSet<String> = BTreeSet::new();
    let mut named_seen = false;
    for (idx, it) in items.iter().enumerate() {
        match it {
            Item::Pos if named_seen => return Err(BindError::PositionalAfterNamed { item: idx }),
            Item::Pos => match queue.pop_front() {
                Some(p) => {
                    filled.insert(p, idx);
                }
                None => match rest {
                    Some(r) => collected.entry(r).or_default().push(idx),
                    None if list.ignore_extra => {}
                    None => return Err(BindError::TooMany { item: idx }),
                },
            },
            Item::Name(n) => {
                named_seen = true;
                if let Some(&p) = by_name.get(n) {
                    if filled.contains_key(&p) {
                        return Err(BindError::Duplicate { item: idx });
                    }
                    filled.insert(p, idx);
                    queue.retain(|&q| q != p);
                } else if let Some(r) = named_rest {
                    if !collected_names.insert(n.clone()) {
                        return Err(BindError::Duplicate { item: idx });
                    }
                    collected.entry(r).or_default().push(idx);
                } else {
                    return Err(BindError::UnknownName { item: idx });
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut presence = 0u64;
    for (i, p) in list.params.iter().enumerate() {
        if p.kind.is_rest() {
            let got = collected.remove(&i).unwrap_or_default();
            if !got.is_empty() {
                presence |= 1 << i;
            }
            out.push(Bound::Collected(got));
        } else if let Some(&item) = filled.get(&i) {
            presence |= 1 << i;
            out.push(Bound::Arg(item));
        } else if p.default {
            out.push(Bound::Default);
        } else {
            return Err(BindError::Missing { param: i });
        }
    }
    Ok((out, presence))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// `bind` agrees with the reference on every list and argument list,
    /// errors included.
    #[test]
    fn bind_matches_the_reference(
        spec in common::param_spec(),
        items in prop::collection::vec(item(), 0..10),
    ) {
        let module = names_module();
        let list = common::param_list(&spec, 0);
        prop_assert!(list.validate().is_ok());
        let got = list
            .bind(&module, &as_items(&items))
            .map(|b| (b.slots().to_vec(), b.presence()));
        prop_assert_eq!(got, reference(&list, &module, &items));
    }

    /// Successful bindings use every argument at most once, give every
    /// non-rest parameter one argument or its default, and set exactly the
    /// presence bits of the parameters that received something.
    #[test]
    fn successful_bindings_are_consistent(
        spec in common::param_spec(),
        items in prop::collection::vec(item(), 0..10),
    ) {
        let module = names_module();
        let list = common::param_list(&spec, 0);
        if let Ok(b) = list.bind(&module, &as_items(&items)) {
            let mut used = BTreeSet::new();
            for (i, (slot, p)) in b.slots().iter().zip(&list.params).enumerate() {
                let present = match slot {
                    Bound::Arg(item) => {
                        prop_assert!(!p.kind.is_rest());
                        prop_assert!(used.insert(*item));
                        true
                    }
                    Bound::Default => {
                        prop_assert!(p.default);
                        false
                    }
                    Bound::Collected(list) => {
                        prop_assert!(p.kind.is_rest());
                        for item in list {
                            prop_assert!(used.insert(*item));
                        }
                        !list.is_empty()
                    }
                };
                prop_assert_eq!((b.presence() >> i) & 1 == 1, present);
            }
            // Without `ignore_extra`, every argument lands somewhere.
            if !list.ignore_extra {
                prop_assert_eq!(used.len(), items.len());
            }
        }
    }

    /// Named arguments bind by name, not by order: reordering the named
    /// arguments of a call changes no parameter's argument.
    #[test]
    fn named_arguments_bind_independently_of_their_order(
        spec in common::param_spec(),
        positional in 0usize..4,
        names in prop::collection::btree_set(0u8..6, 0..6),
        seed in any::<u64>(),
    ) {
        let module = names_module();
        let list = common::param_list(&spec, 0);
        let mut items: Vec<Item> = vec![Item::Pos; positional];
        let named: Vec<String> = names.iter().map(|i| format!("p{i}")).collect();
        items.extend(named.iter().cloned().map(Item::Name));
        let mut shuffled = named.clone();
        // A deterministic rotation and reversal from the seed.
        let len = shuffled.len().max(1);
        shuffled.rotate_left(seed as usize % len);
        if seed & 1 == 1 {
            shuffled.reverse();
        }
        let mut other: Vec<Item> = vec![Item::Pos; positional];
        other.extend(shuffled.iter().cloned().map(Item::Name));
        let name_at = |items: &[Item], i: usize| match &items[i] {
            Item::Pos => format!("#{i}"),
            Item::Name(n) => n.clone(),
        };
        let a = list.bind(&module, &as_items(&items));
        let b = list.bind(&module, &as_items(&other));
        prop_assert_eq!(a.is_ok(), b.is_ok());
        if let (Ok(a), Ok(b)) = (a, b) {
            for (x, y) in a.slots().iter().zip(b.slots()) {
                match (x, y) {
                    (Bound::Arg(i), Bound::Arg(j)) => {
                        prop_assert_eq!(name_at(&items, *i), name_at(&other, *j));
                    }
                    (Bound::Collected(xs), Bound::Collected(ys)) => {
                        let xs: BTreeSet<String> = xs.iter().map(|&i| name_at(&items, i)).collect();
                        let ys: BTreeSet<String> = ys.iter().map(|&j| name_at(&other, j)).collect();
                        prop_assert_eq!(xs, ys);
                    }
                    (x, y) => prop_assert_eq!(x, y),
                }
            }
            prop_assert_eq!(a.presence(), b.presence());
        }
    }

    /// `positional_by_ref(p)` is the by-reference flag of whatever parameter
    /// a positional argument at `p` binds to (false when it is dropped).
    #[test]
    fn positional_by_ref_agrees_with_the_binding(
        spec in common::param_spec(),
        pos in 0usize..8,
    ) {
        let module = names_module();
        let mut list = common::param_list(&spec, 0);
        // Defaults everywhere, so only the positions decide the binding.
        for p in &mut list.params {
            p.default = !p.kind.is_rest();
        }
        list.ignore_extra = true;
        let items = vec![Item::Pos; pos + 1];
        let b = list.bind(&module, &as_items(&items)).unwrap();
        let receiver = b.slots().iter().position(|s| match s {
            Bound::Arg(i) => *i == pos,
            Bound::Collected(list) => list.contains(&pos),
            Bound::Default => false,
        });
        let expected = receiver.is_some_and(|i| list.params[i].by_ref);
        prop_assert_eq!(list.positional_by_ref(pos as u64), expected);
    }

    /// `named_by_ref(n)` is the by-reference flag of the parameter a named
    /// argument `n` binds to (false when nothing takes it).
    #[test]
    fn named_by_ref_agrees_with_the_binding(
        spec in common::param_spec(),
        name in prop_oneof![(0u8..6).prop_map(|i| format!("p{i}")), Just("zz".to_string())],
    ) {
        let module = names_module();
        let mut list = common::param_list(&spec, 0);
        for p in &mut list.params {
            p.default = !p.kind.is_rest();
        }
        let items = [ArgItem::Named(name.as_bytes())];
        let expected = match list.bind(&module, &items) {
            Ok(b) => b
                .slots()
                .iter()
                .position(|s| matches!(s, Bound::Arg(0)) || matches!(s, Bound::Collected(l) if l.contains(&0)))
                .is_some_and(|i| list.params[i].by_ref),
            Err(_) => false,
        };
        prop_assert_eq!(list.named_by_ref(&module, name.as_bytes()), expected);
    }
}

#[test]
fn php_variadics_collect_named_extras_under_their_names() {
    // function f($a, ...$rest); f(1, 2, x: 3) gives $rest = [0 => 2, 'x' => 3].
    let module = names_module();
    let list = ParamList::new(vec![
        Param::normal(StrId(0)),
        Param::new(ParamKind::RestMap, None),
    ]);
    let items = [
        ArgItem::Positional,
        ArgItem::Positional,
        ArgItem::Named(b"x"),
    ];
    let b = list.bind(&module, &items).unwrap();
    assert_eq!(b.slots(), &[Bound::Arg(0), Bound::Collected(vec![1, 2])]);
}

#[test]
fn python_keyword_rest_takes_unknown_names_and_star_args_take_extras() {
    // def f(a, /, b, *args, c, **kw)
    let module = names_module();
    let list = ParamList::new(vec![
        Param::new(ParamKind::PositionalOnly, Some(StrId(0))),
        Param::normal(StrId(1)),
        Param::new(ParamKind::Rest, None),
        Param::new(ParamKind::NamedOnly, Some(StrId(2))),
        Param::new(ParamKind::RestNamed, None),
    ]);
    // f(1, 2, 3, c=4, p0=5): `p0` is positional-only, so it lands in **kw.
    let items = [
        ArgItem::Positional,
        ArgItem::Positional,
        ArgItem::Positional,
        ArgItem::Named(b"p2"),
        ArgItem::Named(b"p0"),
    ];
    let b = list.bind(&module, &items).unwrap();
    assert_eq!(
        b.slots(),
        &[
            Bound::Arg(0),
            Bound::Arg(1),
            Bound::Collected(vec![2]),
            Bound::Arg(3),
            Bound::Collected(vec![4]),
        ]
    );
    // A named-only parameter without default must be given.
    assert_eq!(
        list.bind(&module, &[ArgItem::Positional, ArgItem::Positional]),
        Err(BindError::Missing { param: 3 })
    );
}

#[test]
fn binding_scales_linearly_with_many_spread_names() {
    // 200,000 named arguments from a spread into **kw: the duplicate check is
    // a set, not a scan, so this finishes quickly.
    let module = names_module();
    let list = ParamList::new(vec![Param::new(ParamKind::RestNamed, None)]);
    let names: Vec<String> = (0..200_000).map(|i| format!("k{i}")).collect();
    let items: Vec<ArgItem<'_>> = names.iter().map(|n| ArgItem::Named(n.as_bytes())).collect();
    let b = list.bind(&module, &items).unwrap();
    match &b.slots()[0] {
        Bound::Collected(list) => assert_eq!(list.len(), 200_000),
        other => panic!("{other:?}"),
    }
    let mut repeated = items.clone();
    repeated.push(ArgItem::Named(b"k7"));
    assert_eq!(
        list.bind(&module, &repeated),
        Err(BindError::Duplicate { item: 200_000 })
    );
}
