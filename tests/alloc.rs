//! Counting-allocator proofs: the slice API allocates nothing after
//! construction, and `decompress_vec` never reserves far past the cap.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    sync::atomic::{AtomicUsize, Ordering},
};

use compress_lib::{
    ErrorKind, Limits,
    deflate::{Inflater, Status},
};

struct Counting;

thread_local! {
    // Allocations by this thread only: the test harness allocates on other
    // threads at any time. Const-initialized, so reading it never allocates.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator unchanged; only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|c| c.set(c.get() + 1));
        let live = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
        PEAK.fetch_max(live, Ordering::SeqCst);
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.with(|c| c.set(c.get() + 1));
        let live = LIVE.fetch_add(new_size, Ordering::SeqCst) + new_size;
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
        PEAK.fetch_max(live, Ordering::SeqCst);
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// The tests share the counters, so they run one at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn packed(data: &[u8], level: u8) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(data, level)
}

#[test]
fn test_alloc_slice_api_allocates_nothing_after_construction() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let data: Vec<u8> = (0..300_000u32)
        .map(|i| (i % 251) as u8 ^ (i / 1000) as u8)
        .collect();
    let stream = packed(&data, 6);
    let mut inflater = Inflater::new().with_limits(Limits::unlimited());
    let mut buf = vec![0u8; 4096];
    let mut total = 0;
    let before = ALLOCATIONS.with(Cell::get);
    let mut input = &stream[..];
    for round in 0..3 {
        loop {
            let p = inflater.decompress(input, &mut buf).unwrap();
            input = &input[p.consumed..];
            total += p.produced;
            if p.status == Status::Done {
                break;
            }
        }
        inflater.reset();
        input = &stream[..];
        assert_eq!(total, data.len() * (round + 1));
    }
    assert_eq!(
        ALLOCATIONS.with(Cell::get),
        before,
        "allocation on the slice API"
    );
}

#[test]
fn test_alloc_bomb_peak_bounded_by_cap() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // 64 MiB of zeros compresses to about 64 KiB.
    let bomb = packed(&vec![0u8; 64 << 20], 9);
    let cap = 1 << 20;
    let base = LIVE.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let err = compress_lib::deflate::decompress_with(&bomb, Limits::new().with_max_output(cap))
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::OutputLimit);
    let peak = PEAK.load(Ordering::SeqCst) - base;
    // The output vector grows by doubling and never past the cap; a
    // reallocation briefly holds the old buffer (at most half the cap) and
    // the new one. Plus the inflater: tables only, the one-shot path keeps
    // no window.
    assert!(
        peak <= cap as usize * 3 / 2 + 64 * 1024,
        "peak {peak} bytes for a {cap}-byte cap"
    );
}

#[test]
fn test_alloc_decompress_vec_reserves_within_cap() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let bomb = packed(&vec![7u8; 8 << 20], 9);
    let cap = 300_000;
    let mut inflater = Inflater::new().with_limits(Limits::new().with_max_output(cap));
    let mut out = Vec::new();
    let err = inflater.decompress_vec(&bomb, &mut out).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::OutputLimit);
    assert!(
        out.capacity() <= cap as usize,
        "capacity {} past cap {cap}",
        out.capacity()
    );
}
