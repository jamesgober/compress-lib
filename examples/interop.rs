//! compress-lib's side of the C zlib interoperation check
//! (`tests/interop/check.py`).
//!
//! ```text
//! python3 tests/interop/check.py write DIR
//! cargo run --example interop -- read DIR    # decode CPython's streams
//! cargo run --example interop -- write DIR   # write streams for CPython
//! python3 tests/interop/check.py read DIR
//! ```

use std::{fs, path::Path};

use compress_lib::{Encoder, Format, Level, Limits, deflate, deflate::Flush, gzip, zlib};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (mode, dir) = (args[1].as_str(), Path::new(&args[2]));
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let plains: Vec<String> = names
        .iter()
        .filter(|n| n.starts_with("plain-"))
        .cloned()
        .collect();
    let mut checked = 0;
    match mode {
        "read" => {
            for name in names.iter().filter(|n| n.starts_with("py-")) {
                let data = fs::read(dir.join(name)).unwrap();
                let index = name.rsplit('-').next().unwrap();
                let plain = fs::read(dir.join(format!("plain-{index}"))).unwrap();
                let mut expect = plain.clone();
                let got = if name.starts_with("py-gzip-members-") {
                    expect.extend(plain.iter().rev());
                    gzip::decompress_with(&data, Limits::unlimited())
                } else if name.starts_with("py-raw-") {
                    deflate::decompress_with(&data, Limits::unlimited())
                } else if name.starts_with("py-zlib-") {
                    zlib::decompress_with(&data, Limits::unlimited())
                } else {
                    gzip::decompress_with(&data, Limits::unlimited())
                };
                assert_eq!(
                    got.unwrap_or_else(|e| panic!("{name}: {e}")),
                    expect,
                    "{name}"
                );
                checked += 1;
            }
            println!("compress-lib decoded {checked} CPython streams");
        }
        "write" => {
            for plain_name in &plains {
                let index = plain_name.trim_start_matches("plain-");
                let plain = fs::read(dir.join(plain_name)).unwrap();
                for (tag, format) in [
                    ("raw", Format::Raw),
                    ("zlib", Format::Zlib),
                    ("gzip", Format::Gzip),
                ] {
                    for level in [0u8, 1, 6, 9] {
                        let mut enc = Encoder::new(format, Level::new(level).unwrap());
                        let mut out = Vec::new();
                        // Sync points in the middle, as streaming servers write.
                        let half = plain.len() / 2;
                        enc.compress_vec(&plain[..half], &mut out, Flush::Sync)
                            .unwrap();
                        enc.compress_vec(&plain[half..], &mut out, Flush::Finish)
                            .unwrap();
                        fs::write(dir.join(format!("ours-{tag}-{level}-{index}")), out).unwrap();
                        checked += 1;
                    }
                }
            }
            println!("wrote {checked} compress-lib streams");
        }
        _ => panic!("mode is read or write"),
    }
}
