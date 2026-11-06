//! Soak test: random round trips through every format, level, window, and
//! flush pattern for a set time, checking every result and that heap use
//! returns to where it started (no growth over the run).
//!
//! Ignored by default; run for four hours with:
//!
//! ```text
//! COMPRESS_SOAK_SECS=14400 cargo test --release --test soak -- --ignored --nocapture
//! ```

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use compress_lib::{
    Decoder, Encoder, Format, Level, Limits,
    deflate::{Flush, Status},
    message::{Compressor, Decompressor, MessageConfig},
};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator unchanged; only counts bytes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(live, Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn data(rng: &mut Rng) -> Vec<u8> {
    let len = rng.below(200_000) as usize;
    match rng.below(4) {
        0 => (0..len).map(|_| rng.next() as u8).collect(),
        1 => (0..len)
            .map(|i| (i % 251) as u8 ^ (rng.below(50) == 0) as u8)
            .collect(),
        2 => (0..len)
            .map(|_| b"the quick brown fox \n{}:,"[rng.below(25) as usize])
            .collect(),
        _ => vec![rng.below(256) as u8; len],
    }
}

fn round_trip(rng: &mut Rng) {
    let input = data(rng);
    let format = [Format::Raw, Format::Zlib, Format::Gzip][rng.below(3) as usize];
    let level = Level::new(rng.below(10) as u8).unwrap();
    let mut enc = Encoder::new(format, level);
    let mut packed = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        let end = (pos + 1 + rng.below(70_000) as usize).min(input.len());
        let flush = [Flush::None, Flush::None, Flush::Sync, Flush::Full][rng.below(4) as usize];
        enc.compress_vec(&input[pos..end], &mut packed, flush)
            .unwrap();
        pos = end;
    }
    enc.compress_vec(b"", &mut packed, Flush::Finish).unwrap();
    let mut dec = Decoder::new(format).with_limits(Limits::unlimited());
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 + rng.below(100_000) as usize];
    let mut rest = &packed[..];
    loop {
        let take = (1 + rng.below(50_000) as usize).min(rest.len());
        let last = take == rest.len();
        let p = if last {
            dec.finish(&rest[..take], &mut buf).unwrap()
        } else {
            dec.decompress(&rest[..take], &mut buf).unwrap()
        };
        out.extend_from_slice(&buf[..p.produced]);
        rest = &rest[p.consumed..];
        if p.status == Status::Done {
            break;
        }
    }
    assert_eq!(out, input, "{format:?} {level:?}");
}

fn messages(rng: &mut Rng) {
    let config = MessageConfig::new()
        .with_window_bits(8 + rng.below(8) as u8)
        .with_context_takeover(rng.below(2) == 0)
        .with_level(Level::new(rng.below(10) as u8).unwrap());
    let mut tx = Compressor::new(config).unwrap();
    let mut rx = Decompressor::new(config).unwrap();
    for _ in 0..50 {
        let len = rng.below(5000) as usize;
        let msg: Vec<u8> = (0..len)
            .map(|_| b"{\"k\":\"v\",1234567890}"[rng.below(20) as usize])
            .collect();
        let mut wire = Vec::new();
        tx.compress_message(&msg, &mut wire).unwrap();
        let mut out = Vec::new();
        rx.decompress_message(&wire, &mut out, len).unwrap();
        assert_eq!(out, msg);
    }
}

#[test]
#[ignore = "long-running; set COMPRESS_SOAK_SECS"]
fn soak() {
    let secs: u64 = std::env::var("COMPRESS_SOAK_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    let base = LIVE.load(Ordering::Relaxed);
    let mut rounds = 0u64;
    let mut next_report = Instant::now();
    while Instant::now() < deadline {
        round_trip(&mut rng);
        messages(&mut rng);
        rounds += 1;
        let live = LIVE.load(Ordering::Relaxed);
        assert!(
            live <= base + 4096,
            "heap grew: {live} bytes live, {base} at the start, round {rounds}"
        );
        if Instant::now() >= next_report {
            println!(
                "soak: {rounds} rounds, live {live} B, peak {} B",
                PEAK.load(Ordering::Relaxed)
            );
            next_report = Instant::now() + Duration::from_secs(600);
        }
    }
    println!(
        "soak passed: {rounds} rounds in {secs} s, peak heap {} B",
        PEAK.load(Ordering::Relaxed)
    );
}
