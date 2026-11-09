//! Quick interleaved timing for tuning on a shared machine: every round
//! times this checkout, the last release (0.9.0), and miniz_oxide back to
//! back, and the best round of each counts. Not a replacement for the
//! criterion benchmarks.
//!
//! ```text
//! cargo run --release --bin quick                  # everything
//! cargo run --release --bin quick -- deflate 1     # compression at level 1
//! cargo run --release --bin quick -- inflate       # decompression
//! QUICK_ROUNDS=25 cargo run --release --bin quick  # more rounds
//! ```

use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use compress_lib::{Level, Limits, deflate};
use compress_lib_bench_compare::corpora;

fn time(f: impl FnOnce()) -> Duration {
    let t = Instant::now();
    f();
    t.elapsed()
}

fn mbps(len: usize, d: Duration) -> f64 {
    len as f64 / d.as_secs_f64() / 1e6
}

/// Best-of-`rounds` times for (this checkout, 0.9.0, miniz_oxide).
fn race(
    rounds: usize,
    mut ours: impl FnMut(),
    mut base: impl FnMut(),
    mut miniz: impl FnMut(),
) -> [Duration; 3] {
    let mut best = [Duration::MAX; 3];
    for _ in 0..rounds {
        best[0] = best[0].min(time(&mut ours));
        best[1] = best[1].min(time(&mut base));
        best[2] = best[2].min(time(&mut miniz));
    }
    best
}

fn report(
    kind: &str,
    name: &str,
    level: u8,
    len: usize,
    t: [Duration; 3],
    sizes: Option<[usize; 3]>,
) {
    let x = |a: Duration, b: Duration| b.as_secs_f64() / a.as_secs_f64();
    print!(
        "{kind} {name:<6} L{level}: {:>7.1} MB/s  vs 0.9.0 x{:.2}  vs miniz x{:.2}",
        mbps(len, t[0]),
        x(t[0], t[1]),
        x(t[0], t[2]),
    );
    if let Some([a, b, c]) = sizes {
        print!(
            "  size {a:>8} (0.9.0 {:+.2}%, miniz {:+.2}%)",
            (a as f64 / b as f64 - 1.0) * 100.0,
            (a as f64 / c as f64 - 1.0) * 100.0
        );
    }
    println!();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let what = args.first().map_or("all", String::as_str);
    let levels: Vec<u8> = match args.get(1) {
        Some(l) => vec![l.parse().expect("level")],
        None => vec![1, 6, 9],
    };
    let rounds = std::env::var("QUICK_ROUNDS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(11);
    let data = corpora();
    if what == "all" || what == "deflate" {
        for &level in &levels {
            let lv = Level::new(level).expect("level");
            let lv_base = compress_lib_base::Level::new(level).expect("level");
            for (name, input) in &data {
                let t = race(
                    rounds,
                    || {
                        black_box(deflate::compress_with(black_box(input), lv));
                    },
                    || {
                        black_box(compress_lib_base::deflate::compress_with(
                            black_box(input),
                            lv_base,
                        ));
                    },
                    || {
                        black_box(miniz_oxide::deflate::compress_to_vec(
                            black_box(input),
                            level,
                        ));
                    },
                );
                let ours = deflate::compress_with(input, lv);
                assert_eq!(
                    deflate::decompress_with(&ours, Limits::unlimited()).expect("round trip"),
                    *input
                );
                let sizes = [
                    ours.len(),
                    compress_lib_base::deflate::compress_with(input, lv_base).len(),
                    miniz_oxide::deflate::compress_to_vec(input, level).len(),
                ];
                report("deflate", name, level, input.len(), t, Some(sizes));
            }
        }
    }
    if what == "all" || what == "inflate" {
        for level in [1u8, 6] {
            for (name, input) in &data {
                let packed = miniz_oxide::deflate::compress_to_vec(input, level);
                let t = race(
                    rounds,
                    || {
                        black_box(
                            deflate::decompress_with(black_box(&packed), Limits::unlimited())
                                .expect("ours"),
                        );
                    },
                    || {
                        black_box(
                            compress_lib_base::deflate::decompress_with(
                                black_box(&packed),
                                compress_lib_base::Limits::unlimited(),
                            )
                            .expect("base"),
                        );
                    },
                    || {
                        black_box(
                            miniz_oxide::inflate::decompress_to_vec(black_box(&packed))
                                .expect("miniz"),
                        );
                    },
                );
                report("inflate", name, level, input.len(), t, None);
            }
        }
    }
}
