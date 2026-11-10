//! Deterministic corpora shared by the criterion benchmarks and the
//! `quick` tool. Not published (dev/PLAN.md, D-004 and D-012).

fn next(x: &mut u32) -> u32 {
    *x ^= *x << 13;
    *x ^= *x >> 17;
    *x ^= *x << 5;
    *x
}

/// English-like text from a fixed word list, common words more often.
pub fn text(len: usize) -> Vec<u8> {
    let words = [
        "the",
        "of",
        "and",
        "to",
        "in",
        "a",
        "is",
        "that",
        "for",
        "it",
        "as",
        "was",
        "with",
        "be",
        "by",
        "on",
        "not",
        "he",
        "this",
        "are",
        "or",
        "his",
        "from",
        "at",
        "which",
        "but",
        "have",
        "an",
        "had",
        "they",
        "you",
        "were",
        "their",
        "one",
        "all",
        "we",
        "can",
        "her",
        "has",
        "there",
        "been",
        "if",
        "more",
        "when",
        "will",
        "would",
        "who",
        "so",
        "no",
        "compression",
        "window",
        "stream",
        "decoder",
        "message",
        "server",
        "request",
        "response",
        "header",
    ];
    let mut x = 0x2545_F491;
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        let r = next(&mut x) as usize;
        let i = (r % words.len()).min(r % 17 + (r >> 8) % 7);
        out.extend_from_slice(words[i % words.len()].as_bytes());
        out.push(match next(&mut x) % 16 {
            0 => b'\n',
            1 => b',',
            2 => b'.',
            _ => b' ',
        });
    }
    out.truncate(len);
    out
}

/// JSON API records.
pub fn json(len: usize) -> Vec<u8> {
    let mut x = 0x9E37_79B9;
    let mut out = Vec::with_capacity(len + 160);
    let mut i = 0u32;
    while out.len() < len {
        out.extend_from_slice(
            format!(
                "{{\"id\":{i},\"user\":\"u{}\",\"score\":{},\"tags\":[\"a\",\"b{}\"],\"active\":{},\"ts\":\"2026-11-{:02}T{:02}:{:02}:00Z\"}}\n",
                next(&mut x) % 5000,
                next(&mut x) % 100_000,
                next(&mut x) % 9,
                next(&mut x) % 2 == 0,
                1 + next(&mut x) % 28,
                next(&mut x) % 24,
                next(&mut x) % 60
            )
            .as_bytes(),
        );
        i += 1;
    }
    out.truncate(len);
    out
}

/// HTML-like markup.
pub fn html(len: usize) -> Vec<u8> {
    let mut x = 0xC0DE_CAFE;
    let tags = ["div", "span", "p", "a", "li", "td", "section"];
    let mut out = Vec::with_capacity(len + 200);
    while out.len() < len {
        let t = tags[(next(&mut x) % 7) as usize];
        out.extend_from_slice(
            format!(
                "<{t} class=\"c{}\" id=\"n{}\">item {} of the list</{t}>\n",
                next(&mut x) % 12,
                next(&mut x) % 10_000,
                next(&mut x) % 500
            )
            .as_bytes(),
        );
    }
    out.truncate(len);
    out
}

/// Structured binary: counters, small fields, some noise.
pub fn binary(len: usize) -> Vec<u8> {
    let mut x = 0xDEAD_BEEF;
    let mut out = Vec::with_capacity(len + 8);
    let mut i = 0u32;
    while out.len() < len {
        out.extend_from_slice(&i.to_le_bytes());
        out.extend_from_slice(&((next(&mut x) % 64) as u16).to_le_bytes());
        out.push(next(&mut x) as u8);
        i += 1;
    }
    out.truncate(len);
    out
}

pub fn random(len: usize) -> Vec<u8> {
    let mut x = 0x0BAD_F00D;
    (0..len).map(|_| next(&mut x) as u8).collect()
}

pub fn corpora() -> Vec<(&'static str, Vec<u8>)> {
    let len = 1 << 20;
    vec![
        ("text", text(len)),
        ("json", json(len)),
        ("html", html(len)),
        ("binary", binary(len)),
        ("random", random(len)),
    ]
}
