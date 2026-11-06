//! zlib and gzip framing: interoperation with zlib (through zlib-rs) both
//! ways, every header field, every check, the member and header caps,
//! format detection, and the `std::io` adapters.

use std::io::{Read, Write};

use compress_lib::{
    Decoder, Encoder, ErrorKind, Format, Level, Limits,
    checksum::crc32,
    deflate::{Flush, Status},
    gzip::{self, GzipHeader},
    io, zlib,
};

fn sample() -> Vec<u8> {
    let mut x = 0x1357_9BDFu32;
    let mut out = Vec::new();
    while out.len() < 120_000 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        out.extend_from_slice(
            ["log ", "line ", "ok ", "error ", "\n"][(x % 5) as usize].as_bytes(),
        );
    }
    out
}

/// zlib-rs inflate with `window_bits` 15 (zlib) or 31 (gzip).
fn zlib_rs_inflate(data: &[u8], bits: i32, len: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; len + 64];
    let (got, code) =
        zlib_rs::decompress_slice(&mut out, data, zlib_rs::InflateConfig { window_bits: bits });
    (code == zlib_rs::ReturnCode::Ok).then(|| got.to_vec())
}

fn zlib_rs_deflate(data: &[u8], bits: i32, level: i32) -> Vec<u8> {
    let mut out = vec![0u8; data.len() + 1024];
    let config = zlib_rs::DeflateConfig {
        level,
        window_bits: bits,
        ..Default::default()
    };
    let (got, code) = zlib_rs::compress_slice(&mut out, data, config);
    assert_eq!(code, zlib_rs::ReturnCode::Ok);
    got.to_vec()
}

#[test]
fn test_framing_interop_with_zlib_both_ways() {
    let data = sample();
    for level in [0u8, 1, 6, 9] {
        let ours_z = zlib::compress_with(&data, Level::new(level).unwrap());
        assert_eq!(
            zlib_rs_inflate(&ours_z, 15, data.len()).as_deref(),
            Some(&data[..]),
            "zlib level {level}"
        );
        let ours_g = gzip::compress_with(&data, Level::new(level).unwrap());
        assert_eq!(
            zlib_rs_inflate(&ours_g, 31, data.len()).as_deref(),
            Some(&data[..]),
            "gzip level {level}"
        );
        let theirs_z = zlib_rs_deflate(&data, 15, i32::from(level));
        assert_eq!(zlib::decompress(&theirs_z).unwrap(), data);
        let theirs_g = zlib_rs_deflate(&data, 31, i32::from(level));
        assert_eq!(gzip::decompress(&theirs_g).unwrap(), data);
    }
}

#[test]
fn test_framing_python_gzip_vectors() {
    // gzip.compress(b"hello gzip", mtime=0) and zlib.compressobj(wbits=31)
    // from CPython 3.12.
    let a = hex("1f8b08000000000002ffcb48cdc9c95748afca2c0000196ad2df0a000000");
    let b = hex("1f8b080000000000000acb48cdc9c95748afca2c0000196ad2df0a000000");
    assert_eq!(gzip::decompress(&a).unwrap(), b"hello gzip");
    assert_eq!(gzip::decompress(&b).unwrap(), b"hello gzip");
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn test_framing_gzip_header_fields_round_trip() {
    let header = GzipHeader::new()
        .with_name(b"data.log")
        .with_comment(b"written by the tests")
        .with_extra(b"XY\x03\x00abc")
        .with_mtime(1_700_000_000)
        .with_os(3)
        .with_text(true);
    let mut enc = gzip::Encoder::new(Level::BEST).with_header(header.clone());
    let mut packed = Vec::new();
    enc.compress_vec(b"field test", &mut packed, Flush::Finish)
        .unwrap();
    assert_eq!(
        zlib_rs_inflate(&packed, 31, 10).as_deref(),
        Some(&b"field test"[..])
    );
    let mut dec = gzip::Decoder::new().with_keep_header(true);
    let mut out = Vec::new();
    dec.finish_vec(&packed, &mut out).unwrap();
    assert_eq!(out, b"field test");
    let got = dec.header().unwrap();
    assert_eq!(got.name(), Some(&b"data.log"[..]));
    assert_eq!(got.comment(), Some(&b"written by the tests"[..]));
    assert_eq!(got.extra(), Some(&b"XY\x03\x00abc"[..]));
    assert_eq!(
        (got.mtime(), got.os(), got.text(), got.extra_flags()),
        (1_700_000_000, 3, true, 2)
    );
    // Without keeping, nothing is stored.
    let mut dec = gzip::Decoder::new();
    dec.finish_vec(&packed, &mut Vec::new()).unwrap();
    assert!(dec.header().is_none());
}

/// A gzip member with FHCRC, built by hand around our deflate output.
fn with_header_crc(good: bool) -> Vec<u8> {
    let body = compress_lib::deflate::compress(b"crc checked header");
    let mut m = vec![0x1F, 0x8B, 8, 0x02 | 0x08, 0, 0, 0, 0, 0, 255];
    m.extend_from_slice(b"name\0");
    let hcrc = (crc32(&m) & 0xFFFF) as u16 ^ u16::from(!good);
    m.extend_from_slice(&hcrc.to_le_bytes());
    m.extend_from_slice(&body);
    m.extend_from_slice(&crc32(b"crc checked header").to_le_bytes());
    m.extend_from_slice(&18u32.to_le_bytes());
    m
}

#[test]
fn test_framing_gzip_header_crc_checked() {
    let good = with_header_crc(true);
    assert_eq!(gzip::decompress(&good).unwrap(), b"crc checked header");
    assert_eq!(
        zlib_rs_inflate(&good, 31, 18).as_deref(),
        Some(&b"crc checked header"[..])
    );
    assert_eq!(
        gzip::decompress(&with_header_crc(false))
            .unwrap_err()
            .kind(),
        ErrorKind::ChecksumMismatch
    );
}

#[test]
fn test_framing_gzip_checks_fail_closed() {
    let packed = gzip::compress(b"integrity");
    let n = packed.len();
    // Reserved flag bits, wrong method, wrong magic.
    for (i, v) in [(3, 0x20u8), (2, 7), (1, 0x8C)] {
        let mut bad = packed.clone();
        bad[i] = v;
        assert_eq!(
            gzip::decompress(&bad).unwrap_err().kind(),
            ErrorKind::InvalidHeader,
            "byte {i}"
        );
    }
    let mut bad = packed.clone();
    bad[n - 8] ^= 1;
    assert_eq!(
        gzip::decompress(&bad).unwrap_err().kind(),
        ErrorKind::ChecksumMismatch
    );
    let mut bad = packed.clone();
    bad[n - 4] ^= 1;
    assert_eq!(
        gzip::decompress(&bad).unwrap_err().kind(),
        ErrorKind::LengthMismatch
    );
    for cut in 0..n {
        assert_eq!(
            gzip::decompress(&packed[..cut]).unwrap_err().kind(),
            ErrorKind::UnexpectedEnd,
            "prefix {cut}"
        );
    }
    let mut extra = packed.clone();
    extra.extend_from_slice(b"junk");
    assert_eq!(
        gzip::decompress(&extra).unwrap_err().kind(),
        ErrorKind::TrailingData
    );
}

#[test]
fn test_framing_gzip_members_and_cap() {
    let mut many = Vec::new();
    for _ in 0..1025 {
        many.extend_from_slice(&gzip::compress(b""));
    }
    assert_eq!(
        gzip::decompress(&many).unwrap_err().kind(),
        ErrorKind::TooManyMembers
    );
    let limits = Limits::new().with_max_members(2000);
    assert_eq!(gzip::decompress_with(&many, limits).unwrap(), b"");

    // The output cap covers all members together.
    let one = gzip::compress(&[b'z'; 600]);
    let mut two = one.clone();
    two.extend_from_slice(&one);
    let tight = Limits::new().with_max_output(1000);
    assert_eq!(
        gzip::decompress_with(&two, tight).unwrap_err().kind(),
        ErrorKind::OutputLimit
    );
    assert_eq!(
        gzip::decompress_with(&two, Limits::new().with_max_output(1200))
            .unwrap()
            .len(),
        1200
    );
}

#[test]
fn test_framing_gzip_header_cap() {
    let header = GzipHeader::new().with_name(&vec![b'n'; 70_000]);
    let mut enc = gzip::Encoder::new(Level::FAST).with_header(header);
    let mut packed = Vec::new();
    enc.compress_vec(b"x", &mut packed, Flush::Finish).unwrap();
    assert_eq!(
        gzip::decompress(&packed).unwrap_err().kind(),
        ErrorKind::HeaderTooLarge
    );
    let roomy = Limits::new().with_max_header(80_000);
    assert_eq!(gzip::decompress_with(&packed, roomy).unwrap(), b"x");
}

#[test]
fn test_framing_gzip_byte_at_a_time() {
    let data = sample();
    let mut packed = gzip::compress(&data[..5000]);
    packed.extend_from_slice(&with_header_crc(true));
    let mut expect = data[..5000].to_vec();
    expect.extend_from_slice(b"crc checked header");
    let mut dec = gzip::Decoder::new();
    let mut out = Vec::new();
    let mut buf = [0u8; 3];
    for (i, b) in packed.iter().enumerate() {
        let last = i + 1 == packed.len();
        let mut input = std::slice::from_ref(b);
        loop {
            let p = if last {
                dec.finish(input, &mut buf).unwrap()
            } else {
                dec.decompress(input, &mut buf).unwrap()
            };
            out.extend_from_slice(&buf[..p.produced]);
            input = &input[p.consumed..];
            if p.status != Status::NeedsOutput {
                break;
            }
        }
    }
    assert!(dec.is_done());
    assert_eq!(dec.members(), 2);
    assert_eq!(out, expect);
}

#[test]
fn test_framing_zlib_byte_at_a_time_with_window_check() {
    let data = sample();
    let packed = zlib_rs_deflate(&data[..20_000], 10, 6);
    assert_eq!(packed[0] >> 4, 2);
    let mut dec = zlib::Decoder::new().with_window_bits(10).unwrap();
    let mut out = Vec::new();
    for (i, b) in packed.iter().enumerate() {
        let r = if i + 1 == packed.len() {
            dec.finish_vec(std::slice::from_ref(b), &mut out)
        } else {
            dec.decompress_vec(std::slice::from_ref(b), &mut out)
        };
        r.unwrap();
    }
    assert_eq!(out, &data[..20_000]);
    let mut small = zlib::Decoder::new().with_window_bits(9).unwrap();
    assert_eq!(
        small
            .finish_vec(&packed, &mut Vec::new())
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidWindowBits
    );
}

#[test]
fn test_framing_format_detect_and_auto_decompress() {
    let data = sample();
    assert_eq!(Format::detect(&gzip::compress(&data)), Some(Format::Gzip));
    for level in 0..=9 {
        assert_eq!(
            Format::detect(&zlib::compress_with(&data, Level::new(level).unwrap())),
            Some(Format::Zlib)
        );
    }
    assert_eq!(Format::detect(&compress_lib::deflate::compress(b"x")), None);
    assert_eq!(
        compress_lib::decompress(&gzip::compress(&data)).unwrap(),
        data
    );
    assert_eq!(
        compress_lib::decompress(&zlib::compress(&data)).unwrap(),
        data
    );
    assert_eq!(
        compress_lib::decompress(b"not compressed")
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidHeader
    );
}

#[test]
fn test_framing_generic_decoder_and_encoder_every_format() {
    let data = sample();
    for format in [Format::Raw, Format::Zlib, Format::Gzip] {
        let mut enc = Encoder::new(format, Level::DEFAULT);
        let mut packed = Vec::new();
        enc.compress_vec(&data[..30_000], &mut packed, Flush::Sync)
            .unwrap();
        enc.compress_vec(&data[30_000..], &mut packed, Flush::Finish)
            .unwrap();
        let mut dec = Decoder::new(format).with_limits(Limits::unlimited());
        let mut out = Vec::new();
        dec.finish_vec(&packed, &mut out).unwrap();
        assert_eq!(out, data, "{format:?}");
        assert!(dec.is_done());
        assert_eq!(dec.total_in(), packed.len() as u64);
    }
}

/// A source that hands out `step` bytes per read.
struct Trickle<'a> {
    data: &'a [u8],
    step: usize,
}

impl Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.step.min(buf.len()).min(self.data.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

#[test]
fn test_framing_io_adapters_every_format() {
    let data = sample();
    for format in [Format::Raw, Format::Zlib, Format::Gzip] {
        // Writer -> Reader.
        let mut w = io::Writer::new(Vec::new(), format, Level::DEFAULT);
        for chunk in data.chunks(777) {
            w.write_all(chunk).unwrap();
        }
        w.flush().unwrap();
        let packed = w.finish().unwrap();
        for step in [1usize, 5000] {
            let mut r = io::Reader::new(
                Trickle {
                    data: &packed,
                    step,
                },
                format,
            )
            .with_limits(Limits::unlimited());
            let mut out = Vec::new();
            r.read_to_end(&mut out).unwrap();
            assert_eq!(out, data, "{format:?} step {step}");
        }
        // EncodeReader -> DecodeWriter.
        let mut packed2 = Vec::new();
        io::EncodeReader::new(
            Trickle {
                data: &data,
                step: 999,
            },
            format,
            Level::FAST,
        )
        .read_to_end(&mut packed2)
        .unwrap();
        let mut dw = io::DecodeWriter::new(Vec::new(), format).with_limits(Limits::unlimited());
        for chunk in packed2.chunks(321) {
            dw.write_all(chunk).unwrap();
        }
        assert_eq!(dw.finish().unwrap(), data, "{format:?} encoder");
    }
}

#[test]
fn test_framing_io_errors_surface_as_io_errors() {
    let mut packed = gzip::compress(b"damaged");
    let n = packed.len();
    packed[n - 6] ^= 0xFF;
    let mut out = Vec::new();
    let err = io::Reader::new(&packed[..], Format::Gzip)
        .read_to_end(&mut out)
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    let truncated = &gzip::compress(b"short")[..10];
    let err = io::Reader::new(truncated, Format::Gzip)
        .read_to_end(&mut Vec::new())
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
}
