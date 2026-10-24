//! The allocator of the engine's WebAssembly modules.
//!
//! Rust's allocator on `wasm32-unknown-unknown` (dlmalloc) asks the host for more linear
//! memory exactly when its free space runs out: one 64 KiB page at a time for the many
//! small records of a document. Each `memory.grow` is a call into the host, which commits
//! the pages and replaces the memory's buffer, and costs about a millisecond whatever the
//! size of the step (measured in Node on Windows on a large memory: 1.1 to 1.7 ms for 1 page or 2048 pages
//! alike). Opening a large Pivot file made tens of thousands of them and spent most of its
//! time there.
//!
//! [`GrowAhead`] is the standard allocator, and whenever that allocator had to grow the
//! memory it grows it further in one step ([`grow_ahead_pages`]) and hands the extra pages
//! back to the allocator's free space. Allocation results are unchanged; the memory
//! reaches the same size in a few dozen steps instead of tens of thousands, and ends at
//! most one step larger than the document needs. A memory that cannot grow that far (its
//! maximum, D14) is grown by the largest step that fits, down to what the allocation asked.

/// WebAssembly pages are 64 KiB.
pub const PAGE_BYTES: usize = 64 * 1024;
/// The least memory added in one step: 16 MiB.
const MIN_AHEAD_PAGES: usize = 256;
/// The most memory added in one step: 128 MiB.
const MAX_AHEAD_PAGES: usize = 2048;

/// How many pages to add ahead of the allocator when a memory of `current` pages had to
/// grow: an eighth of the memory, between 16 and 128 MiB, so the number of steps stays
/// logarithmic while the unused tail stays small next to a large document.
pub fn grow_ahead_pages(current: usize) -> usize {
    (current / 8).clamp(MIN_AHEAD_PAGES, MAX_AHEAD_PAGES)
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::{PAGE_BYTES, grow_ahead_pages};
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::arch::wasm32::memory_size;

    /// The standard allocator that grows the linear memory ahead of its needs.
    ///
    /// Install it with `#[global_allocator] static ALLOCATOR: styk_alloc::GrowAhead = styk_alloc::GrowAhead;`.
    pub struct GrowAhead;

    impl GrowAhead {
        /// Called after an allocation: if the memory grew to satisfy it, claim the next step
        /// now. The block is allocated from the standard allocator (which grows the memory
        /// by exactly its size) and freed at once; dlmalloc keeps the pages as free space
        /// and cannot give them back to WebAssembly, so they serve the allocations to come.
        /// A step the memory cannot grow by is halved until one fits; below a page the
        /// memory is at its maximum and the allocator carries on as it was.
        #[inline]
        unsafe fn after(before: usize) {
            let now = memory_size::<0>();
            if now == before {
                return;
            }
            let mut pages = grow_ahead_pages(now);
            while pages > 0 {
                // SAFETY: a nonzero size with a valid power-of-two alignment; the block is freed with its own layout.
                unsafe {
                    let layout = Layout::from_size_align_unchecked(pages * PAGE_BYTES, 16);
                    let block = System.alloc(layout);
                    if !block.is_null() {
                        System.dealloc(block, layout);
                        return;
                    }
                }
                pages /= 2;
            }
        }
    }

    unsafe impl GlobalAlloc for GrowAhead {
        #[inline]
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let before = memory_size::<0>();
            let pointer = unsafe { System.alloc(layout) };
            unsafe { Self::after(before) };
            pointer
        }

        #[inline]
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let before = memory_size::<0>();
            let pointer = unsafe { System.alloc_zeroed(layout) };
            unsafe { Self::after(before) };
            pointer
        }

        #[inline]
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            unsafe { System.dealloc(pointer, layout) }
        }

        #[inline]
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let before = memory_size::<0>();
            let moved = unsafe { System.realloc(pointer, layout, new_size) };
            unsafe { Self::after(before) };
            moved
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::GrowAhead;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_scale_with_the_memory_between_the_bounds() {
        assert_eq!(grow_ahead_pages(0), MIN_AHEAD_PAGES);
        assert_eq!(grow_ahead_pages(1024), MIN_AHEAD_PAGES, "a 64 MiB memory still grows 16 MiB at a time");
        assert_eq!(grow_ahead_pages(8192), 1024, "a 512 MiB memory grows by 64 MiB");
        assert_eq!(grow_ahead_pages(32768), MAX_AHEAD_PAGES, "a 2 GiB memory grows by 128 MiB");
        assert_eq!(grow_ahead_pages(usize::MAX), MAX_AHEAD_PAGES);
    }

    #[test]
    fn a_memory_reaches_a_large_document_in_few_steps() {
        let (mut pages, mut steps) = (256usize, 0);
        while pages < 24_000 {
            pages += grow_ahead_pages(pages);
            steps += 1;
        }
        assert!(steps < 60, "{steps} steps to reach 1.5 GiB");
    }
}
