//! Compress or decompress a file with gzip through the `std::io` adapters.
//!
//! ```text
//! cargo run --example gzip_file -- compress notes.txt notes.txt.gz
//! cargo run --example gzip_file -- decompress notes.txt.gz notes.txt
//! ```

use std::{
    fs::File,
    io::{self, BufReader, BufWriter},
};

use compress_lib::{
    Format, Level, Limits,
    io::{Reader, Writer},
};

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, mode, from, to] = args.as_slice() else {
        eprintln!("usage: gzip_file compress|decompress <from> <to>");
        std::process::exit(2);
    };
    let input = BufReader::new(File::open(from)?);
    let output = BufWriter::new(File::create(to)?);
    match mode.as_str() {
        "compress" => {
            let mut writer = Writer::new(output, Format::Gzip, Level::DEFAULT);
            io::copy(&mut { input }, &mut writer)?;
            writer.finish()?;
        }
        "decompress" => {
            // Your own files: lift the default 64 MiB output cap.
            let mut reader = Reader::new(input, Format::Gzip).with_limits(Limits::unlimited());
            io::copy(&mut reader, &mut { output })?;
        }
        _ => {
            eprintln!("mode must be compress or decompress");
            std::process::exit(2);
        }
    }
    Ok(())
}
