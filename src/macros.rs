//! Internal macros shared by the type modules.

/// Defines a fieldless `u8` code enum with its stable codes and disassembler
/// names, plus `ALL`, `from_code`, `code`, `name`, and `Display`.
///
/// Every small enum in the format (integer types, policies, dynamic kinds,
/// hooks) is a byte on the wire, so they all need the same four conversions.
/// Generating them from one list keeps the code, the name, and the decoder's
/// accepted set from drifting apart.
macro_rules! code_enum {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $code:literal => $text:literal ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        #[repr(u8)]
        pub enum $name {
            $( $(#[$vmeta])* $variant = $code ),*
        }

        impl $name {
            /// Every value, in code order.
            pub const ALL: &'static [$name] = &[ $( $name::$variant ),* ];

            /// The value with this stable code, or `None` if no value has it.
            #[must_use]
            pub const fn from_code(code: u8) -> Option<Self> {
                match code {
                    $( $code => Some($name::$variant), )*
                    _ => None,
                }
            }

            /// The stable code this value is encoded as.
            #[must_use]
            pub const fn code(self) -> u8 {
                self as u8
            }

            /// The lowercase name the disassembler prints.
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self {
                    $( $name::$variant => $text, )*
                }
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}

/// Defines a dense index newtype with `index()` and a prefixed `Display`.
macro_rules! index_type {
    (
        $(#[$meta:meta])*
        $name:ident($inner:ty) => $prefix:literal
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
        pub struct $name(pub $inner);

        impl $name {
            /// The index as a `usize`, ready to index the table it points into.
            #[must_use]
            #[inline]
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}
