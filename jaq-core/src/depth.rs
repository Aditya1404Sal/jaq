//! A limit on how deep evaluation may go on the stack.
//!
//! jaq evaluates recursively. An embedder whose stack overflow cannot be recovered from (such as
//! a WebAssembly component, where it traps the whole instance) sets a floor on the stack:
//! evaluation that recurses below it fails with an exception that `try` does not catch
//! ([`crate::Exn::is_too_deep`]) instead of overflowing.
use core::sync::atomic::{AtomicUsize, Ordering};

static FLOOR: AtomicUsize = AtomicUsize::new(0);

/// Set the lowest stack address evaluation may reach (the stack grows down); `0` lifts the limit.
pub fn set_floor(floor: usize) {
    FLOOR.store(floor, Ordering::Relaxed);
}

/// The address of the current stack frame, as [`set_floor`] takes it.
#[inline(never)]
pub fn here() -> usize {
    let marker = 0_u8;
    core::ptr::addr_of!(marker) as usize
}

/// Whether evaluation has gone below the floor.
pub(crate) fn exhausted() -> bool {
    let floor = FLOOR.load(Ordering::Relaxed);
    floor != 0 && here() < floor
}
