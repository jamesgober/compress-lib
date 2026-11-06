"""Interoperation with C zlib through CPython's zlib and gzip modules.

    python3 tests/interop/check.py write DIR   # CPython's streams for compress-lib
    python3 tests/interop/check.py read DIR    # check compress-lib's streams

compress-lib's side is `cargo run --example interop -- read|write DIR`.
"""

import gzip
import os
import sys
import zlib

WBITS = {"raw": -15, "zlib": 15, "gzip": 31}


def samples():
    words = b"the of and stream window header deflate inflate gzip zlib message server ".split()
    x = 0x12345678
    out = []
    for n in (0, 1, 100, 5000, 70000):
        buf = bytearray()
        while len(buf) < n:
            x ^= (x << 13) & 0xFFFFFFFF
            x ^= x >> 17
            x ^= (x << 5) & 0xFFFFFFFF
            buf += words[x % len(words)] + b" "
        out.append(bytes(buf[:n]))
    out.append(bytes(range(256)) * 300)
    out.append(os.urandom(0) + bytes((i * 7919) & 0xFF for i in range(40000)))
    return out


def write(d):
    os.makedirs(d, exist_ok=True)
    for i, s in enumerate(samples()):
        open(os.path.join(d, f"plain-{i}"), "wb").write(s)
        for fmt, wbits in WBITS.items():
            for level in (0, 1, 6, 9):
                c = zlib.compressobj(level, zlib.DEFLATED, wbits)
                open(os.path.join(d, f"py-{fmt}-{level}-{i}"), "wb").write(c.compress(s) + c.flush())
        # A two-member gzip file, as `cat a.gz b.gz` makes.
        open(os.path.join(d, f"py-gzip-members-{i}"), "wb").write(gzip.compress(s) + gzip.compress(s[::-1]))
    print("wrote", len(os.listdir(d)), "files")


def read(d):
    checked = 0
    for name in sorted(os.listdir(d)):
        if not name.startswith("ours-"):
            continue
        _, fmt, rest = name.split("-", 2)
        index = rest.rsplit("-", 1)[1]
        plain = open(os.path.join(d, f"plain-{index}"), "rb").read()
        data = open(os.path.join(d, name), "rb").read()
        got = zlib.decompress(data, WBITS[fmt])
        if got != plain:
            sys.exit(f"{name}: C zlib decoded {len(got)} bytes, expected {len(plain)}")
        if fmt == "gzip" and gzip.decompress(data) != plain:
            sys.exit(f"{name}: gzip module disagrees")
        checked += 1
    if checked == 0:
        sys.exit("no compress-lib streams found")
    print("C zlib decoded", checked, "compress-lib streams")


if __name__ == "__main__":
    {"write": write, "read": read}[sys.argv[1]](sys.argv[2])
