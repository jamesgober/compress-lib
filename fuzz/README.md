# Fuzzing

cargo-fuzz targets for every decoder in compress-lib. Targets are added with the code they cover, from `0.3.0` on; the list is in [`../dev/PLAN.md`](../dev/PLAN.md) section 10.

Each target asserts: no panic, no output past the configured cap, and, where a reference decoder exists (miniz_oxide), the same verdict and the same output.

## Running

Requires nightly and `cargo install cargo-fuzz`. Keep the working corpus **outside** the repository:

```
cargo +nightly fuzz run <target> ~/fuzz-corpus/compress-lib/<target>
```

Nothing under `fuzz/corpus/` or `fuzz/artifacts/` is committed. A crash found by fuzzing is fixed test-first: the input becomes a unit test before the fix lands.
