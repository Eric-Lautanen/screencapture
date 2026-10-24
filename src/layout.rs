//! Compile-time layout pins for the `repr(C)` records handed to Win32,
//! X11, and CoreGraphics/Quartz FFI. Each pinned struct carries a `const
//! _` assertion block that fires whenever its module compiles (no
//! cfg(test); every target proves it). A record whose ABI drifts — a
//! reordered field, a wrong scalar width, a lost trailing pointer —
//! breaks the build at the offending offset instead of corrupting a call
//! far away from the edit site.
//!
//! Call site:
//! ```
//! pin_layout!(Msg, 48; hwnd = 0, message = 8, w_param = 16, l_param = 24,
//!     time = 32, point = 36);
//! ```
//! The size literal is in bytes; field entries are struct-field /
//! offset-in-bytes pairs (a field's expected byte offset in this
//! record). Both use `debug_assert`-free `assert!`, so release builds
//! carry the check too.

/// Pin a `repr(C)` struct's byte size and per-field byte offsets.
#[macro_export]
macro_rules! pin_layout {
    ($ty:ty, $size:literal; $( $field:ident = $offset:literal ),* $(,)?) => {
        const _: () = {
            assert!(
                ::core::mem::size_of::<$ty>() == $size,
                ::core::stringify!($ty)
            );
            $( assert!(
                ::core::mem::offset_of!($ty, $field) == $offset,
                ::core::stringify!($field)
            ); )*
        };
    };
}
