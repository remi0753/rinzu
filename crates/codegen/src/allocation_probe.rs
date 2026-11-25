//! Private test-build allocation injection; no runtime/SDK environment switch.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[derive(Clone, Copy, Default)]
struct State {
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        allow(dead_code)
    )]
    configured: bool,
    used: bool,
    active: bool,
    remaining: usize,
    allocations: usize,
}
thread_local! {static STATE: Cell<State> = const {Cell::new(State {configured:false, used:false, active:false, remaining:0, allocations:0})};}

pub fn configure(budget: usize) {
    STATE.with(|state| {
        state.set(State {
            configured: true,
            remaining: budget,
            ..Default::default()
        })
    });
}
pub fn allocations() -> Option<usize> {
    STATE.with(|state| {
        let state = state.get();
        (state.used && !state.active).then_some(state.allocations)
    })
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) struct Preparation;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn preparation() -> Preparation {
    STATE.with(|state| {
        let mut value = state.get();
        if value.configured && !value.used {
            value.used = true;
            value.active = true;
            state.set(value);
        }
    });
    Preparation
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl Drop for Preparation {
    fn drop(&mut self) {
        STATE.with(|state| {
            let mut value = state.get();
            value.active = false;
            state.set(value);
        });
    }
}
fn permitted() -> bool {
    STATE
        .try_with(|state| {
            let mut value = state.get();
            if !value.active {
                return true;
            }
            if value.remaining == 0 {
                return false;
            }
            value.remaining -= 1;
            value.allocations += 1;
            state.set(value);
            true
        })
        .unwrap_or(true)
}
/// Underlying allocator for the test executable's ordinary fatal wrapper.
pub struct Allocator;
// SAFETY: successful layouts, pointers and ownership pass unchanged to System.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !permitted() {
            return std::ptr::null_mut();
        }
        // SAFETY: exact GlobalAlloc input, with no bookkeeping allocation.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !permitted() {
            return std::ptr::null_mut();
        }
        // SAFETY: exact GlobalAlloc input and zeroing contract.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if !permitted() {
            return std::ptr::null_mut();
        }
        // SAFETY: live pointer and allocation layout supplied by the caller.
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: preserve the exact allocator's live pointer/layout contract.
        unsafe { System.dealloc(pointer, layout) }
    }
}
