//! Decompress raw DEFLATE from stdin to stdout through fixed 16 KiB
//! buffers, with a 1 GiB output cap.
//!
//! ```text
//! cargo run --example streaming < data.deflate > data
//! ```

use std::io::{self, Read, Write};

use compress_lib::{
    Limits,
    deflate::{Inflater, Status},
};

fn main() -> io::Result<()> {
    let mut inflater = Inflater::new().with_limits(Limits::new().with_max_output(1 << 30));
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut input = [0u8; 16 * 1024];
    let mut output = [0u8; 16 * 1024];
    loop {
        let n = stdin.read(&mut input)?;
        let last = n == 0;
        let mut rest = &input[..n];
        loop {
            let p = if last {
                inflater.finish(rest, &mut output)?
            } else {
                inflater.decompress(rest, &mut output)?
            };
            rest = &rest[p.consumed..];
            stdout.write_all(&output[..p.produced])?;
            match p.status {
                Status::Done => return stdout.flush(),
                Status::NeedsOutput => continue,
                _ => break,
            }
        }
    }
}
