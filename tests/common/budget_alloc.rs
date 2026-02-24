use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct BudgetAlloc {
    current: AtomicUsize,
    peak: AtomicUsize,
    count: AtomicUsize,
}

impl BudgetAlloc {
    pub const fn new() -> Self {
        Self {
            current: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            count: AtomicUsize::new(0),
        }
    }

    pub fn reset(&self) {
        self.current.store(0, Ordering::SeqCst);
        self.peak.store(0, Ordering::SeqCst);
        self.count.store(0, Ordering::SeqCst);
    }

    pub fn peak_bytes(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    pub fn current_bytes(&self) -> usize {
        self.current.load(Ordering::SeqCst)
    }

    pub fn alloc_count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn add_current(&self, bytes: usize) {
        let old = self.current.fetch_add(bytes, Ordering::SeqCst);
        let new = old + bytes;
        let mut peak = self.peak.load(Ordering::SeqCst);
        while new > peak {
            match self
                .peak
                .compare_exchange_weak(peak, new, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(actual) => peak = actual,
            }
        }
    }

    fn sub_current(&self, bytes: usize) {
        let mut current = self.current.load(Ordering::SeqCst);
        loop {
            let next = current.saturating_sub(bytes);
            match self.current.compare_exchange_weak(
                current,
                next,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }
}

unsafe impl GlobalAlloc for BudgetAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            self.add_current(layout.size());
            self.count.fetch_add(1, Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        self.sub_current(layout.size());
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            self.add_current(layout.size());
            self.count.fetch_add(1, Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size >= layout.size() {
                self.add_current(new_size - layout.size());
            } else {
                self.sub_current(layout.size() - new_size);
            }
            self.count.fetch_add(1, Ordering::SeqCst);
        }
        new_ptr
    }
}
