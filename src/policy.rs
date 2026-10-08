//! Operation policies (`specs/OPS.md` §2) and the packed modifier bytes that
//! carry them on each instruction.
//!
//! OPS fixes a policy per op instance at lowering time, so that every tier
//! sees the same one. LSB therefore stores the policy *in the instruction*:
//! an `iadd` that wraps and an `iadd` that raises on overflow are different
//! instructions, and no tier ever consults a language-wide setting at run
//! time. Each packed type fits the single modifier byte of an eight-byte
//! instruction and rejects bit patterns that name no policy, so a decoded
//! instruction always carries a meaningful one.

use core::fmt;

code_enum! {
    /// An integer type: the width and signedness an integer instruction
    /// works at (OPS §1).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::IntTy;
    ///
    /// assert_eq!(IntTy::I64.bits(), 64);
    /// assert!(IntTy::I8.is_signed());
    /// assert!(!IntTy::U32.is_signed());
    /// assert_eq!(IntTy::from_code(3), Some(IntTy::I64));
    /// assert_eq!(IntTy::U16.to_string(), "u16");
    /// ```
    IntTy {
        /// Signed 8-bit.
        I8 = 0 => "i8",
        /// Signed 16-bit.
        I16 = 1 => "i16",
        /// Signed 32-bit.
        I32 = 2 => "i32",
        /// Signed 64-bit.
        I64 = 3 => "i64",
        /// Unsigned 8-bit.
        U8 = 4 => "u8",
        /// Unsigned 16-bit.
        U16 = 5 => "u16",
        /// Unsigned 32-bit.
        U32 = 6 => "u32",
        /// Unsigned 64-bit.
        U64 = 7 => "u64",
    }
}

impl IntTy {
    /// The width in bits: 8, 16, 32, or 64.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::IntTy;
    ///
    /// assert_eq!(IntTy::U16.bits(), 16);
    /// ```
    #[must_use]
    pub const fn bits(self) -> u32 {
        match self {
            IntTy::I8 | IntTy::U8 => 8,
            IntTy::I16 | IntTy::U16 => 16,
            IntTy::I32 | IntTy::U32 => 32,
            IntTy::I64 | IntTy::U64 => 64,
        }
    }

    /// Whether the type is two's-complement signed.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::IntTy;
    ///
    /// assert!(IntTy::I32.is_signed());
    /// assert!(!IntTy::U8.is_signed());
    /// ```
    #[must_use]
    pub const fn is_signed(self) -> bool {
        (self as u8) < 4
    }

    /// The type named by the low three bits of `bits`. Total: every 3-bit
    /// value names a type.
    pub(crate) const fn from_low3(bits: u8) -> IntTy {
        match bits & 7 {
            0 => IntTy::I8,
            1 => IntTy::I16,
            2 => IntTy::I32,
            3 => IntTy::I64,
            4 => IntTy::U8,
            5 => IntTy::U16,
            6 => IntTy::U32,
            _ => IntTy::U64,
        }
    }
}

code_enum! {
    /// A floating-point type (IEEE 754 binary32 or binary64).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::FloatTy;
    ///
    /// assert_eq!(FloatTy::F64.to_string(), "f64");
    /// assert_eq!(FloatTy::from_code(0), Some(FloatTy::F32));
    /// ```
    FloatTy {
        /// IEEE 754 binary32.
        F32 = 0 => "f32",
        /// IEEE 754 binary64.
        F64 = 1 => "f64",
    }
}

code_enum! {
    /// The `overflow` policy: what integer add, sub, mul, neg, abs, signed
    /// `MIN / -1`, and `int_cast` do when the exact result does not fit.
    ///
    /// [`Promote`](Overflow::Promote) exists for dynamic-value operations only
    /// (OPS §2, added for Mox/PHP): it is valid only where the result
    /// register is `dyn`. [`ModuleBuilder`](crate::ModuleBuilder) refuses it
    /// on a statically typed destination it can see, and the verifier
    /// (v0.5) refuses it everywhere else.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Overflow;
    ///
    /// assert_eq!(Overflow::default(), Overflow::Error);
    /// assert_eq!(Overflow::Wrap.to_string(), "wrap");
    /// assert_eq!(Overflow::from_code(3), Some(Overflow::Promote));
    /// ```
    #[derive(Default)]
    Overflow {
        /// Raise `ArithOverflow` (E0001) through the language's error model.
        #[default]
        Error = 0 => "error",
        /// Two's-complement wrap-around; never an error.
        Wrap = 1 => "wrap",
        /// Abort the program with `ArithOverflow` (E0001); not catchable.
        Trap = 2 => "trap",
        /// The result is the `f64` nearest the exact mathematical result
        /// (ties to even), as PHP does for `PHP_INT_MAX + 1`. Dynamic
        /// results only.
        Promote = 3 => "promote",
    }
}

code_enum! {
    /// The `div_zero` policy: what integer div, rem, floor_div, and
    /// floor_mod do when the divisor is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::DivZero;
    ///
    /// assert_eq!(DivZero::default(), DivZero::Error);
    /// ```
    #[derive(Default)]
    DivZero {
        /// Raise `DivByZero` (E0002) through the language's error model.
        #[default]
        Error = 0 => "error",
        /// Abort the program with `DivByZero` (E0002); not catchable.
        Trap = 1 => "trap",
    }
}

code_enum! {
    /// The `shift` policy: what shl and shr do when the amount is at least
    /// the bit width, or negative for a signed amount.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Shift;
    ///
    /// assert_eq!(Shift::default(), Shift::Error);
    /// ```
    #[derive(Default)]
    Shift {
        /// Raise `ShiftOutOfRange` (E0003) through the language's error model.
        #[default]
        Error = 0 => "error",
        /// Use the amount modulo the width (`n & (width - 1)`); never an error.
        Mask = 1 => "mask",
    }
}

code_enum! {
    /// The `float_to_int` policy: what float-to-integer conversion does with
    /// NaN or an out-of-range value.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::FloatToInt;
    ///
    /// assert_eq!(FloatToInt::default(), FloatToInt::Error);
    /// ```
    #[derive(Default)]
    FloatToInt {
        /// Raise `InvalidConversion` (E0004) through the language's error model.
        #[default]
        Error = 0 => "error",
        /// Clamp to the type's MIN/MAX; NaN becomes 0.
        Saturate = 1 => "saturate",
    }
}

/// The complete OPS policy set, packed into five bits.
///
/// Layout: bits 0–1 `overflow`, bit 2 `div_zero`, bit 3 `shift`, bit 4
/// `float_to_int`; bits 5–7 are zero. An instruction carries the whole set
/// even when only part of it can apply (an `iadd` never divides), so a code
/// generator stamps one policy per language and type onto every instruction
/// and the semantics pick the part they need. The default is all-`error`,
/// the OPS default.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{Overflow, Policy, Shift};
///
/// let p = Policy::new().with_overflow(Overflow::Wrap).with_shift(Shift::Mask);
/// assert_eq!(p.overflow(), Overflow::Wrap);
/// assert_eq!(p.shift(), Shift::Mask);
/// assert_eq!(p.to_string(), "wrap.mask");
/// assert_eq!(Policy::from_bits(p.bits()), Some(p));
/// assert_eq!(Policy::from_bits(0b11).map(|p| p.overflow()), Some(Overflow::Promote));
/// assert_eq!(Policy::from_bits(0b10_0000), None); // bit 5 is reserved
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Policy(u8);

impl Policy {
    /// The OPS defaults: every policy is `error`.
    pub const DEFAULT: Policy = Policy(0);

    /// The OPS defaults: every policy is `error`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Overflow, Policy};
    ///
    /// assert_eq!(Policy::new().overflow(), Overflow::Error);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Policy(0)
    }

    /// The `overflow` policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Overflow, Policy};
    ///
    /// assert_eq!(Policy::new().with_overflow(Overflow::Trap).overflow(), Overflow::Trap);
    /// ```
    #[must_use]
    pub const fn overflow(self) -> Overflow {
        match self.0 & 3 {
            0 => Overflow::Error,
            1 => Overflow::Wrap,
            2 => Overflow::Trap,
            _ => Overflow::Promote,
        }
    }

    /// The `div_zero` policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{DivZero, Policy};
    ///
    /// assert_eq!(Policy::new().with_div_zero(DivZero::Trap).div_zero(), DivZero::Trap);
    /// ```
    #[must_use]
    pub const fn div_zero(self) -> DivZero {
        if self.0 & 4 == 0 {
            DivZero::Error
        } else {
            DivZero::Trap
        }
    }

    /// The `shift` policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Policy, Shift};
    ///
    /// assert_eq!(Policy::new().with_shift(Shift::Mask).shift(), Shift::Mask);
    /// ```
    #[must_use]
    pub const fn shift(self) -> Shift {
        if self.0 & 8 == 0 {
            Shift::Error
        } else {
            Shift::Mask
        }
    }

    /// The `float_to_int` policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{FloatToInt, Policy};
    ///
    /// let p = Policy::new().with_float_to_int(FloatToInt::Saturate);
    /// assert_eq!(p.float_to_int(), FloatToInt::Saturate);
    /// ```
    #[must_use]
    pub const fn float_to_int(self) -> FloatToInt {
        if self.0 & 16 == 0 {
            FloatToInt::Error
        } else {
            FloatToInt::Saturate
        }
    }

    /// This policy with `overflow` replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Overflow, Policy};
    ///
    /// assert_eq!(Policy::new().with_overflow(Overflow::Wrap).to_string(), "wrap");
    /// ```
    #[must_use]
    pub const fn with_overflow(self, overflow: Overflow) -> Self {
        Policy((self.0 & !3) | overflow as u8)
    }

    /// This policy with `div_zero` replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{DivZero, Policy};
    ///
    /// assert_eq!(Policy::new().with_div_zero(DivZero::Trap).to_string(), "divtrap");
    /// ```
    #[must_use]
    pub const fn with_div_zero(self, div_zero: DivZero) -> Self {
        Policy((self.0 & !4) | ((div_zero as u8) << 2))
    }

    /// This policy with `shift` replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Policy, Shift};
    ///
    /// assert_eq!(Policy::new().with_shift(Shift::Mask).to_string(), "mask");
    /// ```
    #[must_use]
    pub const fn with_shift(self, shift: Shift) -> Self {
        Policy((self.0 & !8) | ((shift as u8) << 3))
    }

    /// This policy with `float_to_int` replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{FloatToInt, Policy};
    ///
    /// let p = Policy::new().with_float_to_int(FloatToInt::Saturate);
    /// assert_eq!(p.to_string(), "sat");
    /// ```
    #[must_use]
    pub const fn with_float_to_int(self, float_to_int: FloatToInt) -> Self {
        Policy((self.0 & !16) | ((float_to_int as u8) << 4))
    }

    /// The packed five-bit form.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Policy;
    ///
    /// assert_eq!(Policy::new().bits(), 0);
    /// ```
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Unpacks a policy, or `None` if a reserved bit (5–7) is set. Every
    /// 5-bit value names a policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::Policy;
    ///
    /// assert!(Policy::from_bits(0b1_1111).is_some());
    /// assert!(Policy::from_bits(0b10_0000).is_none());
    /// ```
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !0x1f != 0 {
            None
        } else {
            Some(Policy(bits))
        }
    }

    /// Whether every policy is the OPS default (`error`).
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{Overflow, Policy};
    ///
    /// assert!(Policy::new().is_default());
    /// assert!(!Policy::new().with_overflow(Overflow::Wrap).is_default());
    /// ```
    #[must_use]
    pub const fn is_default(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Debug for Policy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Policy")
            .field("overflow", &self.overflow())
            .field("div_zero", &self.div_zero())
            .field("shift", &self.shift())
            .field("float_to_int", &self.float_to_int())
            .finish()
    }
}

/// Prints only the policies that differ from the default, joined by `.`:
/// `wrap` / `trap` / `promote` (overflow), `divtrap`, `mask`, `sat`. The default policy
/// prints as the empty string.
impl fmt::Display for Policy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sep = "";
        let mut token = |f: &mut fmt::Formatter<'_>, text: &str| -> fmt::Result {
            f.write_str(sep)?;
            sep = ".";
            f.write_str(text)
        };
        match self.overflow() {
            Overflow::Error => {}
            Overflow::Wrap => token(f, "wrap")?,
            Overflow::Trap => token(f, "trap")?,
            Overflow::Promote => token(f, "promote")?,
        }
        if self.div_zero() == DivZero::Trap {
            token(f, "divtrap")?;
        }
        if self.shift() == Shift::Mask {
            token(f, "mask")?;
        }
        if self.float_to_int() == FloatToInt::Saturate {
            token(f, "sat")?;
        }
        Ok(())
    }
}

/// An integer type plus the complete policy set: the modifier byte of every
/// integer instruction that computes an integer.
///
/// Layout: bits 0–2 the [`IntTy`] code, bits 3–7 the [`Policy`].
///
/// # Examples
///
/// ```
/// use bytecode_lang::{IntOp, IntTy, Overflow, Policy};
///
/// let op = IntOp::new(IntTy::I64).with_policy(Policy::new().with_overflow(Overflow::Wrap));
/// assert_eq!(op.ty(), IntTy::I64);
/// assert_eq!(op.policy().overflow(), Overflow::Wrap);
/// assert_eq!(op.to_string(), "i64.wrap");
/// assert_eq!(IntOp::from_bits(op.bits()), Some(op));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IntOp(u8);

impl IntOp {
    /// An operation at `ty` with the default (all-`error`) policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntOp, IntTy};
    ///
    /// assert_eq!(IntOp::new(IntTy::U8).to_string(), "u8");
    /// ```
    #[must_use]
    pub const fn new(ty: IntTy) -> Self {
        IntOp(ty as u8)
    }

    /// This operation with its policy replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntOp, IntTy, Policy, Shift};
    ///
    /// let op = IntOp::new(IntTy::I32).with_policy(Policy::new().with_shift(Shift::Mask));
    /// assert_eq!(op.to_string(), "i32.mask");
    /// ```
    #[must_use]
    pub const fn with_policy(self, policy: Policy) -> Self {
        IntOp((self.0 & 7) | (policy.bits() << 3))
    }

    /// The integer type.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntOp, IntTy};
    ///
    /// assert_eq!(IntOp::new(IntTy::U64).ty(), IntTy::U64);
    /// ```
    #[must_use]
    pub const fn ty(self) -> IntTy {
        IntTy::from_low3(self.0)
    }

    /// The policy set.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntOp, IntTy, Policy};
    ///
    /// assert_eq!(IntOp::new(IntTy::I8).policy(), Policy::new());
    /// ```
    #[must_use]
    pub const fn policy(self) -> Policy {
        Policy(self.0 >> 3)
    }

    /// The packed byte.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntOp, IntTy};
    ///
    /// assert_eq!(IntOp::new(IntTy::I64).bits(), 3);
    /// ```
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Unpacks the byte. Every byte is an integer type plus a policy, so this
    /// returns `Some` for all 256 values in format version 1; the `Option`
    /// leaves room for reserved bits in a later version.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntOp, IntTy, Overflow};
    ///
    /// let op = IntOp::from_bits(0b0001_1011).unwrap(); // i64, overflow = promote
    /// assert_eq!((op.ty(), op.policy().overflow()), (IntTy::I64, Overflow::Promote));
    /// ```
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match Policy::from_bits(bits >> 3) {
            Some(_) => Some(IntOp(bits)),
            None => None,
        }
    }
}

impl fmt::Debug for IntOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntOp")
            .field("ty", &self.ty())
            .field("policy", &self.policy())
            .finish()
    }
}

/// `i64`, followed by `.` and the non-default policies, if any.
impl fmt::Display for IntOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.ty().name())?;
        if !self.policy().is_default() {
            write!(f, ".{}", self.policy())?;
        }
        Ok(())
    }
}

/// The modifier byte of [`Inst::IntCast`](crate::Inst::IntCast): source
/// type, destination type, and the `overflow` policy for values that do not
/// fit.
///
/// Layout: bits 0–2 source [`IntTy`], bits 3–5 destination [`IntTy`], bits
/// 6–7 [`Overflow`].
///
/// # Examples
///
/// ```
/// use bytecode_lang::{IntConv, IntTy, Overflow};
///
/// let c = IntConv::new(IntTy::I64, IntTy::U8, Overflow::Wrap);
/// assert_eq!((c.from(), c.to(), c.overflow()), (IntTy::I64, IntTy::U8, Overflow::Wrap));
/// assert_eq!(c.to_string(), "i64.u8.wrap");
/// assert_eq!(IntConv::from_bits(c.bits()), Some(c));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IntConv(u8);

impl IntConv {
    /// A conversion from `from` to `to` under `overflow`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntConv, IntTy, Overflow};
    ///
    /// let c = IntConv::new(IntTy::U32, IntTy::I32, Overflow::Error);
    /// assert_eq!(c.to_string(), "u32.i32");
    /// ```
    #[must_use]
    pub const fn new(from: IntTy, to: IntTy, overflow: Overflow) -> Self {
        IntConv((from as u8) | ((to as u8) << 3) | ((overflow as u8) << 6))
    }

    /// The source type.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntConv, IntTy, Overflow};
    ///
    /// assert_eq!(IntConv::new(IntTy::I8, IntTy::I16, Overflow::Error).from(), IntTy::I8);
    /// ```
    #[must_use]
    pub const fn from(self) -> IntTy {
        IntTy::from_low3(self.0)
    }

    /// The destination type.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntConv, IntTy, Overflow};
    ///
    /// assert_eq!(IntConv::new(IntTy::I8, IntTy::I16, Overflow::Error).to(), IntTy::I16);
    /// ```
    #[must_use]
    pub const fn to(self) -> IntTy {
        IntTy::from_low3(self.0 >> 3)
    }

    /// The overflow policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntConv, IntTy, Overflow};
    ///
    /// let c = IntConv::new(IntTy::I64, IntTy::I8, Overflow::Trap);
    /// assert_eq!(c.overflow(), Overflow::Trap);
    /// ```
    #[must_use]
    pub const fn overflow(self) -> Overflow {
        match self.0 >> 6 {
            0 => Overflow::Error,
            1 => Overflow::Wrap,
            2 => Overflow::Trap,
            _ => Overflow::Promote,
        }
    }

    /// The packed byte.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntConv, IntTy, Overflow};
    ///
    /// assert_eq!(IntConv::new(IntTy::I8, IntTy::I8, Overflow::Error).bits(), 0);
    /// ```
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Unpacks the byte. Every byte names a conversion, so this returns
    /// `Some` for all 256 values in format version 1. (`promote` decodes, but
    /// `int_cast` always writes a statically typed register, so the builder
    /// and the verifier refuse it.)
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntConv, Overflow};
    ///
    /// assert_eq!(IntConv::from_bits(0b1100_0000).map(|c| c.overflow()), Some(Overflow::Promote));
    /// ```
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        Some(IntConv(bits))
    }
}

impl fmt::Debug for IntConv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntConv")
            .field("from", &self.from())
            .field("to", &self.to())
            .field("overflow", &self.overflow())
            .finish()
    }
}

/// `from.to`, then `.wrap` or `.trap` when the policy is not `error`.
impl fmt::Display for IntConv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.from(), self.to())?;
        match self.overflow() {
            Overflow::Error => Ok(()),
            other => write!(f, ".{other}"),
        }
    }
}

/// The modifier byte of the bit-level conversions
/// ([`Zext`](crate::Inst::Zext), [`Sext`](crate::Inst::Sext),
/// [`Trunc`](crate::Inst::Trunc)), which never raise an error and so carry
/// no policy.
///
/// Layout: bits 0–2 source [`IntTy`], bits 3–5 destination [`IntTy`], bits
/// 6–7 zero.
///
/// # Examples
///
/// ```
/// use bytecode_lang::{IntPair, IntTy};
///
/// let p = IntPair::new(IntTy::I8, IntTy::I64);
/// assert_eq!((p.from(), p.to()), (IntTy::I8, IntTy::I64));
/// assert_eq!(p.to_string(), "i8.i64");
/// assert_eq!(IntPair::from_bits(0b0100_0000), None);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IntPair(u8);

impl IntPair {
    /// The pair `from` → `to`.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntPair, IntTy};
    ///
    /// assert_eq!(IntPair::new(IntTy::U8, IntTy::U32).to(), IntTy::U32);
    /// ```
    #[must_use]
    pub const fn new(from: IntTy, to: IntTy) -> Self {
        IntPair((from as u8) | ((to as u8) << 3))
    }

    /// The source type.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntPair, IntTy};
    ///
    /// assert_eq!(IntPair::new(IntTy::U8, IntTy::U32).from(), IntTy::U8);
    /// ```
    #[must_use]
    pub const fn from(self) -> IntTy {
        IntTy::from_low3(self.0)
    }

    /// The destination type.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntPair, IntTy};
    ///
    /// assert_eq!(IntPair::new(IntTy::I64, IntTy::I16).to(), IntTy::I16);
    /// ```
    #[must_use]
    pub const fn to(self) -> IntTy {
        IntTy::from_low3(self.0 >> 3)
    }

    /// The packed byte.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::{IntPair, IntTy};
    ///
    /// assert_eq!(IntPair::new(IntTy::I16, IntTy::I16).bits(), 0b001_001);
    /// ```
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Unpacks the byte, or `None` if bit 6 or 7 is set.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytecode_lang::IntPair;
    ///
    /// assert!(IntPair::from_bits(0b0011_1111).is_some());
    /// assert!(IntPair::from_bits(0b1000_0000).is_none());
    /// ```
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits >> 6 != 0 {
            None
        } else {
            Some(IntPair(bits))
        }
    }
}

impl fmt::Debug for IntPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntPair")
            .field("from", &self.from())
            .field("to", &self.to())
            .finish()
    }
}

/// `from.to`.
impl fmt::Display for IntPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.from(), self.to())
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    #[test]
    fn policy_accepts_exactly_the_valid_bit_patterns() {
        let mut accepted = 0;
        for bits in 0..=u8::MAX {
            if let Some(p) = Policy::from_bits(bits) {
                accepted += 1;
                assert_eq!(p.bits(), bits);
                let rebuilt = Policy::new()
                    .with_overflow(p.overflow())
                    .with_div_zero(p.div_zero())
                    .with_shift(p.shift())
                    .with_float_to_int(p.float_to_int());
                assert_eq!(rebuilt, p);
            }
        }
        // 4 overflow x 2 div_zero x 2 shift x 2 float_to_int.
        assert_eq!(accepted, 32);
    }

    #[test]
    fn int_op_round_trips_every_valid_byte() {
        let mut accepted = 0;
        for bits in 0..=u8::MAX {
            if let Some(op) = IntOp::from_bits(bits) {
                accepted += 1;
                assert_eq!(IntOp::new(op.ty()).with_policy(op.policy()), op);
            }
        }
        assert_eq!(accepted, 8 * 32);
    }

    #[test]
    fn int_conv_and_pair_round_trip() {
        for bits in 0..=u8::MAX {
            if let Some(c) = IntConv::from_bits(bits) {
                assert_eq!(IntConv::new(c.from(), c.to(), c.overflow()), c);
            }
            if let Some(p) = IntPair::from_bits(bits) {
                assert_eq!(IntPair::new(p.from(), p.to()), p);
            }
        }
    }

    #[test]
    fn display_lists_only_non_default_policies() {
        assert_eq!(Policy::new().to_string(), "");
        let all = Policy::new()
            .with_overflow(Overflow::Trap)
            .with_div_zero(DivZero::Trap)
            .with_shift(Shift::Mask)
            .with_float_to_int(FloatToInt::Saturate);
        assert_eq!(all.to_string(), "trap.divtrap.mask.sat");
        let promote = Policy::new().with_overflow(Overflow::Promote);
        assert_eq!(promote.to_string(), "promote");
        assert_eq!(Policy::from_bits(promote.bits()), Some(promote));
        assert_eq!(
            IntOp::new(IntTy::U64).with_policy(all).to_string(),
            "u64.trap.divtrap.mask.sat"
        );
    }

    #[test]
    fn int_types_report_width_and_sign() {
        let widths: [u32; 8] = [8, 16, 32, 64, 8, 16, 32, 64];
        for (ty, bits) in IntTy::ALL.iter().zip(widths) {
            assert_eq!(ty.bits(), bits);
            assert_eq!(ty.is_signed(), ty.code() < 4);
            assert_eq!(IntTy::from_low3(ty.code()), *ty);
        }
    }
}
