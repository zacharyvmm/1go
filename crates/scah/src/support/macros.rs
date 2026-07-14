#[macro_export]
macro_rules! mut_prt_unchecked {
    ($e:expr) => {{
        #[inline(always)]
        fn cast<T>(r: &T) -> *mut T {
            r as *const T as *mut T
        }
        cast($e)
    }};
}

#[cfg(any(debug_assertions, test))]
#[macro_export]
macro_rules! scah_trace {
    ($store:expr, $event:expr) => {{
        let event = $event;
        #[cfg(feature = "otel")]
        {
            $crate::otel::emit_trace_event(&event);
        }
        $store.trace_event(event);
    }};
}

#[cfg(not(any(debug_assertions, test)))]
#[macro_export]
macro_rules! scah_trace {
    ($store:expr, $event:expr) => {{
        let _ = &$store;
    }};
}

/// Case-insensitive tag-name classification with a lowercase fast path.
///
/// Most real-world HTML uses lowercase tag names. This macro exploits that:
/// it first tries an exact `match` against the provided lowercase tag
/// literals (a cheap pointer+length compare). Only when an uppercase ASCII
/// character is detected in `$name` does it fall back to a linear
/// `eq_ignore_ascii_case` scan of the same literals.
///
/// # Arguments
///
/// - `$name` — a `&str` tag name to classify.
/// - `$($tag:literal)|+` — one or more lowercase string literals to match
///   against (case-insensitively).
///
/// # Examples
///
/// ```ignore
/// ascii_ci_tag_match!(name, "div", "p", "a")
/// ascii_ci_tag_match!(name, "br", "hr", "img", "input")
/// ```
///
/// Strategy chosen from Criterion microbenchmarks in `benches/`:
/// the exact-lowercase-match fast path outperforms a pure
/// `eq_ignore_ascii_case` scan for typical lowercase HTML by ~3-8×,
/// while the uppercase fallback has identical cost to the original scan.
#[macro_export]
macro_rules! ascii_ci_tag_match {
    ($name:expr, $($tag:literal),+ $(,)?) => {{
        match $name {
            $($tag)|+ => true,
            _ => {
                $name.as_bytes().iter().any(|b| b.is_ascii_uppercase())
                    && [$($tag),+]
                        .iter()
                        .any(|tag| $name.eq_ignore_ascii_case(tag))
            }
        }
    }};
}
