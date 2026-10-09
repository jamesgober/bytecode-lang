//! Dynamic-call signatures: parameter lists, call shapes, and the binding
//! rule every tier applies to them (`specs/LSB.md` §5.15).
//!
//! A typed call (`call`, `call_indirect`, `call_import`) passes exactly the
//! callee's parameters, arranged at compile time. A *dynamic* call
//! (`dcall`, `dcall_shape`) only learns its callee at run time, so the
//! callee carries a [`ParamList`] (names, kinds, by-reference flags,
//! defaults) and the call site carries a [`CallShape`] (which arguments are
//! positional, named, or spread). Binding one to the other is written once,
//! in [`ParamList::bind`], so the VM, the T0 evaluator, and a native tier
//! cannot disagree on PHP's or Python's argument rules: each tier either
//! calls it or differential-tests its own fast path against it.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::fmt;

use crate::ids::StrId;
use crate::module::Module;
use crate::types::ValType;

code_enum! {
    /// How a parameter of a [`ParamList`] receives arguments.
    ///
    /// The kinds appear in this order in a list: positional-only, normal,
    /// at most one rest ([`Rest`](ParamKind::Rest) or
    /// [`RestMap`](ParamKind::RestMap)), named-only, at most one
    /// [`RestNamed`](ParamKind::RestNamed). This is HIR's `Param` kind set
    /// (`specs/HIR.md` §10) without the receiver, which is an ordinary
    /// positional parameter at this level.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ParamKind;
    ///
    /// assert_eq!(ParamKind::Normal.code(), 1);
    /// assert_eq!(ParamKind::RestNamed.to_string(), "rest_named");
    /// assert!(ParamKind::RestMap.is_rest());
    /// assert!(ParamKind::NamedOnly.takes_names());
    /// assert!(!ParamKind::PositionalOnly.takes_names());
    /// ```
    ParamKind {
        /// Bound by position only (Python's parameters before `/`).
        PositionalOnly = 0 => "positional_only",
        /// Bound by position or by name (every PHP parameter).
        Normal = 1 => "normal",
        /// Bound by name only (Python's parameters after `*`).
        NamedOnly = 2 => "named_only",
        /// Collects the extra positional arguments into a new `dyn` array
        /// (Python `*args`).
        Rest = 3 => "rest",
        /// Collects the extra positional arguments into a new `dyn` map under
        /// the keys `0, 1, ...`, and, when the list has no
        /// [`RestNamed`](ParamKind::RestNamed), the unknown named arguments
        /// under their names (PHP `...$args` since 8.1).
        RestMap = 4 => "rest_map",
        /// Collects the unknown named arguments into a new `dyn` map from name
        /// to value (Python `**kwargs`).
        RestNamed = 5 => "rest_named",
    }
}

impl ParamKind {
    /// Whether the parameter collects arguments into a new container.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ParamKind;
    ///
    /// assert!(ParamKind::Rest.is_rest());
    /// assert!(!ParamKind::Normal.is_rest());
    /// ```
    #[must_use]
    pub const fn is_rest(self) -> bool {
        matches!(
            self,
            ParamKind::Rest | ParamKind::RestMap | ParamKind::RestNamed
        )
    }

    /// Whether a named argument can bind to the parameter by its name.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ParamKind;
    ///
    /// assert!(ParamKind::Normal.takes_names());
    /// assert!(!ParamKind::Rest.takes_names());
    /// ```
    #[must_use]
    pub const fn takes_names(self) -> bool {
        matches!(self, ParamKind::Normal | ParamKind::NamedOnly)
    }

    /// Whether a positional argument can bind to the parameter.
    const fn takes_positions(self) -> bool {
        matches!(self, ParamKind::PositionalOnly | ParamKind::Normal)
    }

    /// The kind's position in the required order; rest kinds share one rank.
    const fn rank(self) -> u8 {
        match self {
            ParamKind::PositionalOnly => 0,
            ParamKind::Normal => 1,
            ParamKind::Rest | ParamKind::RestMap => 2,
            ParamKind::NamedOnly => 3,
            ParamKind::RestNamed => 4,
        }
    }
}

/// One parameter of a [`ParamList`].
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Param, ParamKind, StrId};
///
/// let p = Param::normal(StrId(3)).by_ref();
/// assert_eq!((p.kind, p.name, p.by_ref, p.default), (ParamKind::Normal, Some(StrId(3)), true, false));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Param {
    /// The parameter's name in the callee's module. Required for
    /// [`Normal`](ParamKind::Normal) and [`NamedOnly`](ParamKind::NamedOnly)
    /// parameters; optional (diagnostics only) for the others.
    pub name: Option<StrId>,
    /// How the parameter receives arguments.
    pub kind: ParamKind,
    /// Whether the parameter takes its argument **by reference** (PHP
    /// `&$x`): it receives a reference (kind `reference`) aliasing the
    /// caller's place instead of the value. For a rest parameter, every
    /// collected argument is taken by reference.
    pub by_ref: bool,
    /// Whether the parameter has a default, which the callee computes
    /// itself when the presence mask says no argument arrived. Never set on
    /// a rest parameter.
    pub default: bool,
}

impl Param {
    /// A parameter of kind `kind` named `name`, by value, without default.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Param, ParamKind};
    ///
    /// assert_eq!(Param::new(ParamKind::Rest, None).kind, ParamKind::Rest);
    /// ```
    #[must_use]
    pub const fn new(kind: ParamKind, name: Option<StrId>) -> Self {
        Param {
            name,
            kind,
            by_ref: false,
            default: false,
        }
    }

    /// A [`Normal`](ParamKind::Normal) parameter named `name`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Param, ParamKind, StrId};
    ///
    /// assert_eq!(Param::normal(StrId(0)).kind, ParamKind::Normal);
    /// ```
    #[must_use]
    pub const fn normal(name: StrId) -> Self {
        Param::new(ParamKind::Normal, Some(name))
    }

    /// This parameter taking its argument by reference.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Param, StrId};
    ///
    /// assert!(Param::normal(StrId(0)).by_ref().by_ref);
    /// ```
    #[must_use]
    pub const fn by_ref(mut self) -> Self {
        self.by_ref = true;
        self
    }

    /// This parameter with a default.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Param, StrId};
    ///
    /// assert!(Param::normal(StrId(0)).with_default().default);
    /// ```
    #[must_use]
    pub const fn with_default(mut self) -> Self {
        self.default = true;
        self
    }
}

/// Why a [`ParamList`] is malformed, or does not fit the signature it is
/// attached to.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Param, ParamError, ParamKind, ParamList};
///
/// let list = ParamList::new(vec![Param::new(ParamKind::Normal, None)]);
/// assert_eq!(list.validate(), Err(ParamError::Unnamed { index: 0 }));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ParamError {
    /// More than 255 parameters, or more than 64 when a parameter has a
    /// default (the presence mask is 64 bits).
    TooMany,
    /// A parameter's kind is out of the required order, or a second rest
    /// parameter of one rank appears.
    OutOfOrder {
        /// The parameter.
        index: u16,
    },
    /// A normal or named-only parameter has no name.
    Unnamed {
        /// The parameter.
        index: u16,
    },
    /// Two parameters have the same name.
    DuplicateName {
        /// The second of them.
        index: u16,
    },
    /// A rest parameter is marked as having a default.
    RestDefault {
        /// The parameter.
        index: u16,
    },
    /// The list does not fit the function's signature: the signature must
    /// have one parameter per entry, plus a trailing `i64` presence mask
    /// when an entry has a default; rest parameters are `dyn`; by-reference
    /// parameters are `dyn` or a `ref` (to a `cell dyn`).
    Signature {
        /// The first signature parameter that does not fit (the parameter
        /// count, if the counts differ).
        index: u16,
    },
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParamError::TooMany => {
                f.write_str("too many parameters (255 at most, 64 when a parameter has a default)")
            }
            ParamError::OutOfOrder { index } => {
                write!(f, "parameter {index} is out of the required kind order")
            }
            ParamError::Unnamed { index } => {
                write!(f, "parameter {index} can be named but has no name")
            }
            ParamError::DuplicateName { index } => {
                write!(f, "parameter {index} repeats an earlier name")
            }
            ParamError::RestDefault { index } => {
                write!(f, "rest parameter {index} cannot have a default")
            }
            ParamError::Signature { index } => {
                write!(
                    f,
                    "the signature does not fit the parameter list at {index}"
                )
            }
        }
    }
}

impl core::error::Error for ParamError {}

/// The dynamic-call signature of a function or an import: what lets a
/// `dcall` or `dcall_shape` pass named, spread, extra, missing, and
/// by-reference arguments (`specs/LSB.md` §5.15).
///
/// The list describes the signature's parameters in order. When any
/// parameter has a default, the signature has **one more** parameter, an
/// `i64` *presence mask*: bit `i` is set when parameter `i` received an
/// argument, so the callee computes the defaults of the others itself
/// (PHP's and Python's defaults are expressions). A callee without a list
/// binds as if it had one positional-only parameter per signature
/// parameter: exact arity, positional arguments only.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Param, ParamKind, ParamList, StrId};
///
/// // PHP: function f($a, &$b = null, ...$rest)
/// let list = ParamList::new(vec![
///     Param::normal(StrId(0)),
///     Param::normal(StrId(1)).by_ref().with_default(),
///     Param::new(ParamKind::RestMap, Some(StrId(2))),
/// ]);
/// assert!(list.validate().is_ok());
/// assert!(list.has_defaults());
/// assert_eq!(list.signature_len(), 4); // three parameters and the presence mask
/// assert!(list.positional_by_ref(1));
/// assert!(!list.positional_by_ref(7)); // a rest argument, by value
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct ParamList {
    /// The parameters, in signature order.
    pub params: Vec<Param>,
    /// Extra positional arguments with no rest parameter to take them are
    /// dropped instead of raising `ArgumentError` (PHP user functions,
    /// which see them only through `func_get_args`).
    pub ignore_extra: bool,
}

/// The most entries a parameter list or a call shape may have (call windows
/// and `argc` are 8-bit).
pub(crate) const MAX_ARITY: usize = 255;

/// The most parameters a list with defaults may have: the width of the
/// presence mask.
const MAX_WITH_DEFAULTS: usize = 64;

impl ParamList {
    /// A list of these parameters that raises on extra arguments.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Param, ParamList, StrId};
    ///
    /// let list = ParamList::new(vec![Param::normal(StrId(0))]);
    /// assert!(!list.ignore_extra);
    /// ```
    #[must_use]
    pub fn new(params: Vec<Param>) -> Self {
        ParamList {
            params,
            ignore_extra: false,
        }
    }

    /// This list, dropping extra positional arguments (PHP user functions).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::ParamList;
    ///
    /// assert!(ParamList::new(vec![]).ignoring_extra().ignore_extra);
    /// ```
    #[must_use]
    pub fn ignoring_extra(mut self) -> Self {
        self.ignore_extra = true;
        self
    }

    /// Whether any parameter has a default, so the signature ends with the
    /// presence mask.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Param, ParamList, StrId};
    ///
    /// assert!(!ParamList::new(vec![Param::normal(StrId(0))]).has_defaults());
    /// ```
    #[must_use]
    pub fn has_defaults(&self) -> bool {
        self.params.iter().any(|p| p.default)
    }

    /// The number of signature parameters the list describes: one per
    /// entry, plus the presence mask when a parameter has a default.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Param, ParamList, StrId};
    ///
    /// let list = ParamList::new(vec![Param::normal(StrId(0)).with_default()]);
    /// assert_eq!(list.signature_len(), 2);
    /// ```
    #[must_use]
    pub fn signature_len(&self) -> usize {
        self.params.len() + usize::from(self.has_defaults())
    }

    /// Checks the list's own rules: at most 255 parameters (64 with
    /// defaults); kinds in order (positional-only, normal, one rest,
    /// named-only, one named rest); normal and named-only parameters named;
    /// names unique; no default on a rest parameter.
    ///
    /// # Errors
    ///
    /// The first rule broken, as a [`ParamError`].
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Param, ParamError, ParamKind, ParamList, StrId};
    ///
    /// let rest_first = ParamList::new(vec![
    ///     Param::new(ParamKind::Rest, None),
    ///     Param::normal(StrId(0)),
    /// ]);
    /// assert_eq!(rest_first.validate(), Err(ParamError::OutOfOrder { index: 1 }));
    /// ```
    pub fn validate(&self) -> Result<(), ParamError> {
        let limit = if self.has_defaults() {
            MAX_WITH_DEFAULTS
        } else {
            MAX_ARITY
        };
        if self.params.len() > limit {
            return Err(ParamError::TooMany);
        }
        let mut last_rank = 0u8;
        let mut names = BTreeSet::new();
        for (i, p) in self.params.iter().enumerate() {
            // `i` < 255, checked above.
            let index = u16::try_from(i).unwrap_or(u16::MAX);
            let rank = p.kind.rank();
            // Ranks never decrease, and the two rest ranks hold one each.
            let repeated_rest = i > 0 && rank == last_rank && (rank == 2 || rank == 4);
            if rank < last_rank || repeated_rest {
                return Err(ParamError::OutOfOrder { index });
            }
            last_rank = rank;
            if p.kind.takes_names() && p.name.is_none() {
                return Err(ParamError::Unnamed { index });
            }
            if let Some(name) = p.name {
                if !names.insert(name) {
                    return Err(ParamError::DuplicateName { index });
                }
            }
            if p.kind.is_rest() && p.default {
                return Err(ParamError::RestDefault { index });
            }
        }
        Ok(())
    }

    /// Checks that the list fits a signature with parameter types
    /// `sig_params`: one parameter per entry plus a trailing `i64` presence
    /// mask when an entry has a default; rest parameters `dyn`; by-reference
    /// parameters `dyn` or `ref` (the verifier checks that the `ref` is a
    /// `cell dyn`).
    ///
    /// # Errors
    ///
    /// [`ParamError::Signature`] naming the first parameter that does not
    /// fit.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Param, ParamError, ParamKind, ParamList, StrId, ValType};
    ///
    /// let list = ParamList::new(vec![Param::new(ParamKind::Rest, None)]);
    /// assert!(list.fits(&[ValType::Dyn]).is_ok());
    /// assert_eq!(list.fits(&[ValType::I64]), Err(ParamError::Signature { index: 0 }));
    /// ```
    pub fn fits(&self, sig_params: &[ValType]) -> Result<(), ParamError> {
        let n = self.params.len();
        if sig_params.len() != self.signature_len() {
            let index = u16::try_from(n.min(sig_params.len())).unwrap_or(u16::MAX);
            return Err(ParamError::Signature { index });
        }
        for (i, (p, &ty)) in self.params.iter().zip(sig_params).enumerate() {
            let ok = if p.kind.is_rest() && !p.by_ref {
                ty == ValType::Dyn
            } else if p.by_ref {
                // A by-reference rest still collects into a `dyn` container.
                ty == ValType::Dyn || (!p.kind.is_rest() && matches!(ty, ValType::Ref(_)))
            } else {
                true
            };
            if !ok {
                let index = u16::try_from(i).unwrap_or(u16::MAX);
                return Err(ParamError::Signature { index });
            }
        }
        if self.has_defaults() && sig_params.get(n) != Some(&ValType::I64) {
            let index = u16::try_from(n).unwrap_or(u16::MAX);
            return Err(ParamError::Signature { index });
        }
        Ok(())
    }

    /// The rest parameter collecting extra positional arguments, if any.
    fn positional_rest(&self) -> Option<(usize, &Param)> {
        self.params
            .iter()
            .enumerate()
            .find(|(_, p)| matches!(p.kind, ParamKind::Rest | ParamKind::RestMap))
    }

    /// The parameter collecting unknown named arguments, if any: the named
    /// rest, or else a [`RestMap`](ParamKind::RestMap).
    fn named_rest(&self) -> Option<(usize, &Param)> {
        self.params
            .iter()
            .enumerate()
            .find(|(_, p)| p.kind == ParamKind::RestNamed)
            .or_else(|| {
                self.params
                    .iter()
                    .enumerate()
                    .find(|(_, p)| p.kind == ParamKind::RestMap)
            })
    }

    /// Whether a positional argument at 0-based position `pos` would be
    /// taken by reference: the semantics of `dparam_ref` (PHP decides
    /// before evaluating each argument of a dynamic call whether to send a
    /// reference). Positions past the positional parameters belong to the
    /// rest parameter, if any; with none, `false`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Param, ParamKind, ParamList, StrId};
    ///
    /// let list = ParamList::new(vec![
    ///     Param::normal(StrId(0)),
    ///     Param::new(ParamKind::RestMap, None).by_ref(),
    /// ]);
    /// assert!(!list.positional_by_ref(0));
    /// assert!(list.positional_by_ref(5));
    /// ```
    #[must_use]
    pub fn positional_by_ref(&self, pos: u64) -> bool {
        let mut seen = 0u64;
        for p in &self.params {
            if p.kind.takes_positions() {
                if seen == pos {
                    return p.by_ref;
                }
                seen += 1;
            }
        }
        self.positional_rest().is_some_and(|(_, p)| p.by_ref)
    }

    /// Whether a named argument `name` would be taken by reference: the
    /// semantics of `dparam_ref_named`. Parameter names are strings of
    /// `module`, the callee's module.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, Param, ParamList};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let x = m.string("x");
    /// let module = m.finish().unwrap();
    /// let list = ParamList::new(vec![Param::normal(x).by_ref()]);
    /// assert!(list.named_by_ref(&module, b"x"));
    /// assert!(!list.named_by_ref(&module, b"y"));
    /// ```
    #[must_use]
    pub fn named_by_ref(&self, module: &Module, name: &[u8]) -> bool {
        match self.find_named(module, name) {
            Some(i) => self.params.get(i).is_some_and(|p| p.by_ref),
            None => self.named_rest().is_some_and(|(_, p)| p.by_ref),
        }
    }

    /// The normal or named-only parameter called `name`.
    fn find_named(&self, module: &Module, name: &[u8]) -> Option<usize> {
        self.params.iter().position(|p| {
            p.kind.takes_names()
                && p.name
                    .and_then(|s| module.string(s))
                    .is_some_and(|s| s.as_bytes() == name)
        })
    }

    /// Binds a dynamic call's arguments to this list: the rule of
    /// `specs/LSB.md` §5.15, written once so every tier applies it alike.
    ///
    /// `items` are the call's arguments in order, after spreads were
    /// expanded (a spread array contributes positional items; a spread
    /// map's integer keys positional items and its string keys named ones).
    /// Parameter names are strings of `module`, the callee's module.
    ///
    /// 1. Positional items fill the positional-only and normal parameters
    ///    in order; extra ones go to the rest parameter, else are dropped
    ///    under [`ignore_extra`](ParamList::ignore_extra), else
    ///    [`BindError::TooMany`]. A positional item after a named one is
    ///    [`BindError::PositionalAfterNamed`].
    /// 2. A named item fills the normal or named-only parameter of that
    ///    name ([`BindError::Duplicate`] if it is already filled); with
    ///    none, it goes to the named rest, else to a
    ///    [`RestMap`](ParamKind::RestMap) (a repeated name there is
    ///    `Duplicate`), else [`BindError::UnknownName`].
    /// 3. A parameter left empty takes its default (its presence bit is
    ///    clear), or is [`BindError::Missing`]; a rest parameter is an empty
    ///    collection.
    ///
    /// The first violation in item order is reported, then the first missing
    /// parameter. Runs in `O(items × params)` with `params ≤ 255`, plus
    /// `O(n log n)` for duplicate names in a rest collection.
    ///
    /// # Errors
    ///
    /// A [`BindError`]; a VM raises `ArgumentError` (E0114) for every one.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ArgItem, Bound, ModuleBuilder, Param, ParamKind, ParamList};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let (a, b) = (m.string("a"), m.string("b"));
    /// let module = m.finish().unwrap();
    /// // function f($a, $b = 2, ...$rest)
    /// let list = ParamList::new(vec![
    ///     Param::normal(a),
    ///     Param::normal(b).with_default(),
    ///     Param::new(ParamKind::RestMap, None),
    /// ]);
    /// // f(1, 2, 3, x: 4)
    /// let items = [ArgItem::Positional, ArgItem::Positional, ArgItem::Positional, ArgItem::Named(b"x")];
    /// let bound = list.bind(&module, &items).unwrap();
    /// assert_eq!(bound.slots(), &[Bound::Arg(0), Bound::Arg(1), Bound::Collected(vec![2, 3])]);
    /// assert_eq!(bound.presence(), 0b111);
    /// // f(b: 5, a: 6)
    /// let bound = list.bind(&module, &[ArgItem::Named(b"b"), ArgItem::Named(b"a")]).unwrap();
    /// assert_eq!(bound.slots(), &[Bound::Arg(1), Bound::Arg(0), Bound::Collected(vec![])]);
    /// ```
    pub fn bind(&self, module: &Module, items: &[ArgItem<'_>]) -> Result<Binding, BindError> {
        let mut slots: Vec<Bound> = self
            .params
            .iter()
            .map(|p| {
                if p.kind.is_rest() {
                    Bound::Collected(Vec::new())
                } else {
                    Bound::Default
                }
            })
            .collect();
        let rest = self.positional_rest().map(|(i, _)| i);
        let named_rest = self.named_rest().map(|(i, _)| i);
        // The next parameter that may take a positional argument. It only
        // moves forward, so finding all of them is O(params) in total.
        let mut cursor = 0usize;
        let mut seen_named = false;
        let mut rest_names: BTreeSet<&[u8]> = BTreeSet::new();
        for (item, arg) in items.iter().enumerate() {
            match *arg {
                ArgItem::Positional => {
                    if seen_named {
                        return Err(BindError::PositionalAfterNamed { item });
                    }
                    while self
                        .params
                        .get(cursor)
                        .is_some_and(|p| !p.kind.takes_positions())
                    {
                        cursor += 1;
                    }
                    if let Some(slot) = slots.get_mut(cursor) {
                        cursor += 1;
                        *slot = Bound::Arg(item);
                    } else if let Some(Bound::Collected(list)) = rest.and_then(|r| slots.get_mut(r))
                    {
                        list.push(item);
                    } else if !self.ignore_extra {
                        return Err(BindError::TooMany { item });
                    }
                }
                ArgItem::Named(name) => {
                    seen_named = true;
                    if let Some(param) = self.find_named(module, name) {
                        match slots.get_mut(param) {
                            Some(slot @ Bound::Default) => *slot = Bound::Arg(item),
                            _ => return Err(BindError::Duplicate { item }),
                        }
                    } else if let Some(Bound::Collected(list)) =
                        named_rest.and_then(|r| slots.get_mut(r))
                    {
                        if !rest_names.insert(name) {
                            return Err(BindError::Duplicate { item });
                        }
                        list.push(item);
                    } else {
                        return Err(BindError::UnknownName { item });
                    }
                }
            }
        }
        let mut presence = 0u64;
        for (i, (slot, p)) in slots.iter().zip(&self.params).enumerate() {
            let present = match slot {
                Bound::Arg(_) => true,
                Bound::Collected(list) => !list.is_empty(),
                Bound::Default if p.default => false,
                Bound::Default => return Err(BindError::Missing { param: i }),
            };
            if present && i < 64 {
                presence |= 1 << i;
            }
        }
        Ok(Binding { slots, presence })
    }
}

/// One argument of a dynamic call, as [`ParamList::bind`] sees it.
///
/// # Examples
///
/// ```
/// use bytecode_lang::ArgItem;
///
/// let items = [ArgItem::Positional, ArgItem::Named(b"x")];
/// assert_eq!(items.len(), 2);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ArgItem<'a> {
    /// A positional argument.
    Positional,
    /// A named argument, its name as bytes (from a call shape's string or a
    /// spread map's string key).
    Named(&'a [u8]),
}

/// What one parameter received from [`ParamList::bind`].
///
/// # Examples
///
/// ```
/// use bytecode_lang::Bound;
///
/// assert_ne!(Bound::Arg(0), Bound::Default);
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Bound {
    /// The argument at this item index.
    Arg(usize),
    /// Nothing: the parameter has a default, which the callee computes
    /// (its presence bit is clear).
    Default,
    /// A rest parameter's items, in order. A [`RestMap`](ParamKind::RestMap)
    /// stores a positional item under the next integer key and a named one
    /// under its name.
    Collected(Vec<usize>),
}

/// The result of [`ParamList::bind`]: what each parameter receives, and the
/// presence mask.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{ArgItem, Bound, ModuleBuilder, Param, ParamKind, ParamList};
///
/// let module = ModuleBuilder::new().finish().unwrap();
/// let list = ParamList::new(vec![Param::new(ParamKind::PositionalOnly, None)]);
/// let b = list.bind(&module, &[ArgItem::Positional]).unwrap();
/// assert_eq!(b.slots(), &[Bound::Arg(0)]);
/// assert_eq!(b.presence(), 1);
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Binding {
    slots: Vec<Bound>,
    presence: u64,
}

impl Binding {
    /// What each parameter receives, in parameter order.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, ParamList};
    ///
    /// let module = ModuleBuilder::new().finish().unwrap();
    /// assert!(ParamList::new(vec![]).bind(&module, &[]).unwrap().slots().is_empty());
    /// ```
    #[must_use]
    pub fn slots(&self) -> &[Bound] {
        &self.slots
    }

    /// The presence mask: bit `i` is set when parameter `i` received an
    /// argument (for a rest parameter, at least one).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ModuleBuilder, Param, ParamList, StrId};
    ///
    /// let mut m = ModuleBuilder::new();
    /// let x = m.string("x");
    /// let module = m.finish().unwrap();
    /// let list = ParamList::new(vec![Param::normal(x).with_default()]);
    /// assert_eq!(list.bind(&module, &[]).unwrap().presence(), 0);
    /// ```
    #[must_use]
    pub fn presence(&self) -> u64 {
        self.presence
    }
}

/// Why a dynamic call's arguments do not bind ([`ParamList::bind`]). A VM
/// raises `ArgumentError` (E0114) for every variant; the variant is the
/// detail a diagnostic can show.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{ArgItem, BindError, ModuleBuilder, ParamList};
///
/// let module = ModuleBuilder::new().finish().unwrap();
/// let none = ParamList::new(vec![]);
/// assert_eq!(none.bind(&module, &[ArgItem::Positional]), Err(BindError::TooMany { item: 0 }));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum BindError {
    /// A positional argument with no parameter to take it.
    TooMany {
        /// The argument's item index.
        item: usize,
    },
    /// A positional argument after a named one (PHP's "cannot use
    /// positional argument after named argument").
    PositionalAfterNamed {
        /// The argument's item index.
        item: usize,
    },
    /// A named argument no parameter takes.
    UnknownName {
        /// The argument's item index.
        item: usize,
    },
    /// A named argument for a parameter already filled, or a name repeated
    /// in a rest collection.
    Duplicate {
        /// The argument's item index.
        item: usize,
    },
    /// A parameter without default received nothing.
    Missing {
        /// The parameter.
        param: usize,
    },
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::TooMany { item } => write!(f, "argument {item} has no parameter"),
            BindError::PositionalAfterNamed { item } => {
                write!(f, "positional argument {item} follows a named argument")
            }
            BindError::UnknownName { item } => {
                write!(f, "named argument {item} matches no parameter")
            }
            BindError::Duplicate { item } => {
                write!(f, "argument {item} names a parameter already given")
            }
            BindError::Missing { param } => write!(f, "parameter {param} received no argument"),
        }
    }
}

impl core::error::Error for BindError {}

/// One argument of a [`CallShape`].
///
/// # Examples
///
/// ```
/// use bytecode_lang::{ArgKind, StrId};
///
/// assert_eq!(ArgKind::Named(StrId(4)).to_string(), "s4:");
/// assert_eq!(ArgKind::Spread.to_string(), "...");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ArgKind {
    /// A positional argument.
    Positional,
    /// A named argument (PHP `f(x: 1)`, Python `f(x=1)`); the name is a
    /// string of the calling module.
    Named(StrId),
    /// A spread (PHP `...$a`, Python `*a`): an array contributes positional
    /// arguments; a map contributes its integer-keyed values positionally
    /// and its string-keyed values as named arguments.
    Spread,
    /// A named spread (Python `**kw`): a map whose keys must all be strings.
    SpreadNamed,
}

impl ArgKind {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            ArgKind::Positional => 0,
            ArgKind::Named(_) => 1,
            ArgKind::Spread => 2,
            ArgKind::SpreadNamed => 3,
        }
    }

    /// Whether the argument can contribute positional arguments.
    const fn is_positional(self) -> bool {
        matches!(self, ArgKind::Positional | ArgKind::Spread)
    }
}

/// `_` (positional), `s4:` (named), `...` (spread), `**` (named spread).
impl fmt::Display for ArgKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArgKind::Positional => f.write_str("_"),
            ArgKind::Named(name) => write!(f, "{name}:"),
            ArgKind::Spread => f.write_str("..."),
            ArgKind::SpreadNamed => f.write_str("**"),
        }
    }
}

/// Why a [`CallShape`] is malformed.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{ArgKind, CallShape, ShapeError, StrId};
///
/// let shape = CallShape::new(vec![ArgKind::Named(StrId(0)), ArgKind::Positional]);
/// assert_eq!(shape.validate(), Err(ShapeError::PositionalAfterNamed { index: 1 }));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ShapeError {
    /// More than 255 arguments (a call window holds at most 255).
    TooMany,
    /// A positional argument or a spread follows a named argument or a
    /// named spread.
    PositionalAfterNamed {
        /// The argument.
        index: u16,
    },
    /// A name appears twice.
    DuplicateName {
        /// The second occurrence.
        index: u16,
    },
}

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShapeError::TooMany => f.write_str("more than 255 arguments"),
            ShapeError::PositionalAfterNamed { index } => {
                write!(f, "argument {index} is positional after a named argument")
            }
            ShapeError::DuplicateName { index } => {
                write!(f, "argument {index} repeats an earlier name")
            }
        }
    }
}

impl core::error::Error for ShapeError {}

/// The argument layout of a `dcall_shape` call site: one entry per window
/// register (`dst+1 ..= dst+len`), saying whether that argument is
/// positional, named, or spread. Shapes live in a per-function table and
/// are named by a [`ShapeId`](crate::ShapeId).
///
/// # Examples
///
/// ```
/// use bytecode_lang::{ArgKind, CallShape, StrId};
///
/// // f($a, ...$rest, flag: true)
/// let shape = CallShape::new(vec![ArgKind::Positional, ArgKind::Spread, ArgKind::Named(StrId(2))]);
/// assert!(shape.validate().is_ok());
/// assert_eq!(shape.to_string(), "(_, ..., s2:)");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct CallShape {
    /// The arguments, in window order.
    pub args: Vec<ArgKind>,
}

impl CallShape {
    /// A shape of these arguments.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ArgKind, CallShape};
    ///
    /// assert_eq!(CallShape::new(vec![ArgKind::Positional]).args.len(), 1);
    /// ```
    #[must_use]
    pub fn new(args: Vec<ArgKind>) -> Self {
        CallShape { args }
    }

    /// Checks the shape: at most 255 arguments, no positional argument or
    /// spread after a named argument or named spread, no repeated name.
    ///
    /// # Errors
    ///
    /// The first rule broken, as a [`ShapeError`].
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{ArgKind, CallShape, ShapeError, StrId};
    ///
    /// let twice = CallShape::new(vec![ArgKind::Named(StrId(1)), ArgKind::Named(StrId(1))]);
    /// assert_eq!(twice.validate(), Err(ShapeError::DuplicateName { index: 1 }));
    /// ```
    pub fn validate(&self) -> Result<(), ShapeError> {
        if self.args.len() > MAX_ARITY {
            return Err(ShapeError::TooMany);
        }
        let mut named = false;
        let mut names = BTreeSet::new();
        for (i, &arg) in self.args.iter().enumerate() {
            let index = u16::try_from(i).unwrap_or(u16::MAX);
            if arg.is_positional() {
                if named {
                    return Err(ShapeError::PositionalAfterNamed { index });
                }
            } else {
                named = true;
            }
            if let ArgKind::Named(name) = arg {
                if !names.insert(name) {
                    return Err(ShapeError::DuplicateName { index });
                }
            }
        }
        Ok(())
    }
}

/// `(_, s3:, ...)`.
impl fmt::Display for CallShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(")?;
        for (i, arg) in self.args.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{arg}")?;
        }
        f.write_str(")")
    }
}

/// `params (s0, &s1 = ?, ...s2) ignore_extra`-style rendering for listings:
/// each parameter as its kind marker, `&` when by reference, its name, and
/// ` = ?` when it has a default.
impl fmt::Display for ParamList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(")?;
        for (i, p) in self.params.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(match p.kind {
                ParamKind::PositionalOnly => "pos ",
                ParamKind::Normal => "",
                ParamKind::NamedOnly => "named ",
                ParamKind::Rest => "...",
                ParamKind::RestMap => "...map ",
                ParamKind::RestNamed => "**",
            })?;
            if p.by_ref {
                f.write_str("&")?;
            }
            match p.name {
                Some(name) => write!(f, "{name}")?,
                None => f.write_str("_")?,
            }
            if p.default {
                f.write_str(" = ?")?;
            }
        }
        f.write_str(")")?;
        if self.ignore_extra {
            f.write_str(" ignore_extra")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;
    use crate::ModuleBuilder;

    fn module_with(names: &[&str]) -> (Module, Vec<StrId>) {
        let mut m = ModuleBuilder::new();
        let ids = names.iter().map(|n| m.string(n)).collect();
        (m.finish().unwrap_or_default(), ids)
    }

    #[test]
    fn kinds_must_come_in_order_with_one_rest_each() {
        let ok = ParamList::new(vec![
            Param::new(ParamKind::PositionalOnly, None),
            Param::normal(StrId(0)),
            Param::new(ParamKind::Rest, None),
            Param::new(ParamKind::NamedOnly, Some(StrId(1))),
            Param::new(ParamKind::RestNamed, None),
        ]);
        assert_eq!(ok.validate(), Ok(()));
        let two_rests = ParamList::new(vec![
            Param::new(ParamKind::Rest, None),
            Param::new(ParamKind::RestMap, None),
        ]);
        assert_eq!(
            two_rests.validate(),
            Err(ParamError::OutOfOrder { index: 1 })
        );
        let two_named_rests = ParamList::new(vec![
            Param::new(ParamKind::RestNamed, None),
            Param::new(ParamKind::RestNamed, None),
        ]);
        assert_eq!(
            two_named_rests.validate(),
            Err(ParamError::OutOfOrder { index: 1 })
        );
        let named_twice = ParamList::new(vec![Param::normal(StrId(0)), Param::normal(StrId(0))]);
        assert_eq!(
            named_twice.validate(),
            Err(ParamError::DuplicateName { index: 1 })
        );
        let rest_default = ParamList::new(vec![Param::new(ParamKind::Rest, None).with_default()]);
        assert_eq!(
            rest_default.validate(),
            Err(ParamError::RestDefault { index: 0 })
        );
    }

    #[test]
    fn the_presence_mask_caps_lists_with_defaults_at_64() {
        let names = |n: u32| (0..n).map(|i| Param::normal(StrId(i))).collect::<Vec<_>>();
        let mut many = names(65);
        assert_eq!(ParamList::new(many.clone()).validate(), Ok(()));
        if let Some(p) = many.first_mut() {
            p.default = true;
        }
        assert_eq!(ParamList::new(many).validate(), Err(ParamError::TooMany));
        assert_eq!(
            ParamList::new(names(256)).validate(),
            Err(ParamError::TooMany)
        );
    }

    #[test]
    fn fits_checks_counts_mask_and_reference_types() {
        let list = ParamList::new(vec![
            Param::normal(StrId(0)).by_ref().with_default(),
            Param::new(ParamKind::Rest, None),
        ]);
        let ok = [ValType::Dyn, ValType::Dyn, ValType::I64];
        assert_eq!(list.fits(&ok), Ok(()));
        let by_ref_typed = [ValType::I32, ValType::Dyn, ValType::I64];
        assert_eq!(
            list.fits(&by_ref_typed),
            Err(ParamError::Signature { index: 0 })
        );
        let no_mask = [ValType::Dyn, ValType::Dyn];
        assert_eq!(list.fits(&no_mask), Err(ParamError::Signature { index: 2 }));
        let wrong_mask = [ValType::Dyn, ValType::Dyn, ValType::I32];
        assert_eq!(
            list.fits(&wrong_mask),
            Err(ParamError::Signature { index: 2 })
        );
    }

    #[test]
    fn binding_follows_php_rules() {
        let (module, ids) = module_with(&["a", "b"]);
        let list = ParamList::new(vec![
            Param::normal(ids[0]),
            Param::normal(ids[1]).with_default(),
        ]);
        use ArgItem::{Named, Positional};
        assert_eq!(
            list.bind(&module, &[Positional, Positional, Positional]),
            Err(BindError::TooMany { item: 2 })
        );
        assert!(
            list.clone()
                .ignoring_extra()
                .bind(&module, &[Positional, Positional, Positional])
                .is_ok()
        );
        assert_eq!(
            list.bind(&module, &[Positional, Named(b"a")]),
            Err(BindError::Duplicate { item: 1 })
        );
        assert_eq!(
            list.bind(&module, &[Named(b"b"), Positional]),
            Err(BindError::PositionalAfterNamed { item: 1 })
        );
        assert_eq!(
            list.bind(&module, &[Named(b"c")]),
            Err(BindError::UnknownName { item: 0 })
        );
        assert_eq!(
            list.bind(&module, &[Named(b"b")]),
            Err(BindError::Missing { param: 0 })
        );
        let b = list
            .bind(&module, &[Positional])
            .unwrap_or_else(|_| Binding {
                slots: vec![],
                presence: u64::MAX,
            });
        assert_eq!(b.slots(), &[Bound::Arg(0), Bound::Default]);
        assert_eq!(b.presence(), 1);
    }

    #[test]
    fn positional_only_names_go_to_the_named_rest() {
        let (module, ids) = module_with(&["a"]);
        let list = ParamList::new(vec![
            Param::new(ParamKind::PositionalOnly, Some(ids[0])),
            Param::new(ParamKind::RestNamed, None),
        ]);
        use ArgItem::{Named, Positional};
        let b = list.bind(&module, &[Positional, Named(b"a")]);
        assert_eq!(
            b.map(|b| b.slots().to_vec()),
            Ok(vec![Bound::Arg(0), Bound::Collected(vec![1])])
        );
        assert_eq!(
            list.bind(&module, &[Positional, Named(b"k"), Named(b"k")]),
            Err(BindError::Duplicate { item: 2 })
        );
    }

    #[test]
    fn by_ref_queries_follow_the_binding() {
        let (module, ids) = module_with(&["a", "k"]);
        let list = ParamList::new(vec![
            Param::normal(ids[0]).by_ref(),
            Param::new(ParamKind::RestNamed, None).by_ref(),
        ]);
        assert!(list.positional_by_ref(0));
        assert!(!list.positional_by_ref(1));
        assert!(list.named_by_ref(&module, b"a"));
        assert!(list.named_by_ref(&module, b"k"));
    }

    #[test]
    fn shapes_reject_positionals_after_names() {
        let s = CallShape::new(vec![ArgKind::SpreadNamed, ArgKind::Spread]);
        assert_eq!(
            s.validate(),
            Err(ShapeError::PositionalAfterNamed { index: 1 })
        );
        let ok = CallShape::new(vec![
            ArgKind::Spread,
            ArgKind::Named(StrId(0)),
            ArgKind::SpreadNamed,
        ]);
        assert_eq!(ok.validate(), Ok(()));
        let long = CallShape::new(vec![ArgKind::Positional; 256]);
        assert_eq!(long.validate(), Err(ShapeError::TooMany));
    }

    #[test]
    fn lists_and_shapes_print_compactly() {
        let list = ParamList::new(vec![
            Param::new(ParamKind::PositionalOnly, None),
            Param::normal(StrId(1)).by_ref().with_default(),
            Param::new(ParamKind::RestMap, Some(StrId(2))),
        ])
        .ignoring_extra();
        assert_eq!(list.to_string(), "(pos _, &s1 = ?, ...map s2) ignore_extra");
        assert_eq!(
            CallShape::new(vec![ArgKind::Positional, ArgKind::SpreadNamed]).to_string(),
            "(_, **)"
        );
    }
}
