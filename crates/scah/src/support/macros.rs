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
