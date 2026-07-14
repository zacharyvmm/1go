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
/// # Correctness constraint
///
/// **All `$tag` literals must be lowercase ASCII.** The macro uses an exact
/// lowercase fast path before falling back to `eq_ignore_ascii_case()` only
/// when the input name contains uppercase ASCII. Passing an uppercase literal
/// such as `"DIV"` will silently break case-insensitive matching on that
/// arm — the exact-match fast path will never hit for a lowercase input,
/// and the uppercase fallback will not correct it because the literal itself
/// is uppercase.
///
/// This is safe for all current internal call sites because the HTML
/// specification defines tag names in lowercase.
///
/// Strategy chosen from comparative Criterion microbenchmarks in
/// `benches/tag_classification/`: the exact-lowercase-match fast path
/// outperforms a pure `eq_ignore_ascii_case` scan for typical lowercase
/// HTML, while the uppercase fallback has identical cost to the original
/// scan. The length-bucketed strategy was also measured; it trades a one-time
/// bucket construction cost for faster misses on long candidate lists but
/// showed no advantage for the short lists used in practice (≤25 tags).
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
