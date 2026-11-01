#!/usr/bin/env python3
"""Snapshot Koopman's CRC Zoo and conservatively import its HD metadata.

Only Python's standard library is needed. Cached HTML supports offline imports.
Both reciprocal polynomials are retained because their simplification can differ.
"""
import argparse
import datetime
import hashlib
from html.parser import HTMLParser
import json
import os
from pathlib import Path
import re
import tempfile
from urllib.request import urlopen

BASE = "https://users.ece.cmu.edu/~koopman/crc/"
ATTRIBUTION = "Philip Koopman, Carnegie Mellon University; CRC Polynomial Zoo"
LICENSE = "https://creativecommons.org/licenses/by/4.0/"


def atomic_write(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix=path.name + ".")
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        if hasattr(os, "O_DIRECTORY"):
            fd = os.open(path.parent, os.O_DIRECTORY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


class Rows(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.rows = []
        self.current = None

    def handle_starttag(self, tag, attrs):
        if tag.lower() == "li":
            self.finish()
            self.current = []

    def handle_endtag(self, tag):
        if tag.lower() == "li":
            self.finish()

    def handle_data(self, data):
        if self.current is not None:
            self.current.append(data)

    def finish(self):
        if self.current is not None:
            self.rows.append(" ".join("".join(self.current).split()))
            self.current = None


PAIR = re.compile(r"\(\s*(0x[0-9a-fA-F]+)\s*;\s*(0x[0-9a-fA-F]+)\s*\)")


def parse_page(html, width, source):
    parser = Rows()
    parser.feed(html)
    parser.finish()
    candidates = []
    seen = {}
    for row in parser.rows:
        # Lists also contain navigation and copyright material. Polynomial rows
        # must have both complete representations and an HD vector.
        if not row.startswith("(") or "0x" not in row:
            continue
        pairs = PAIR.findall(row)
        vector = re.search(r"\{([^{}]*)\}", row)
        if len(pairs) != 2 or vector is None:
            raise ValueError(f"unrecognized polynomial row: {row}")
        hd = []
        entries = vector.group(1).split(",") if vector.group(1).strip() else []
        for distance, entry in enumerate(entries, 3):
            entry = entry.strip()
            label = re.search(r"HD\s*=\s*(\d+)", entry)
            if label and int(label.group(1)) != distance:
                raise ValueError(f"misaligned HD annotation: {entry}")
            if re.fullmatch(r"\d+", entry):
                status, value = "exact", str(int(entry))
            elif re.fullmatch(r">=\s*\d+", entry):
                status, value = "lower_bound", str(int(entry[2:].strip()))
            else:
                # Even '?HD=8?>=1024' is kept unknown: the question marks and
                # tentative trailing '?' must not become proven guarantees.
                status, value = "unknown", None
            if value is not None and int(value) > 2**64 - 1:
                raise ValueError("HD length exceeds u64")
            hd.append({"distance": distance, "max_data_bits": value, "status": status, "raw": entry})
        for orientation, (implicit, explicit) in zip(("original", "reciprocal"), pairs):
            implicit, explicit = int(implicit, 16), int(explicit, 16)
            if explicit != (implicit << 1 | 1) or explicit.bit_length() != width + 1:
                raise ValueError(f"invalid Koopman notation at width {width}: {row}")
            polynomial = explicit & ((1 << width) - 1)
            if polynomial in seen:
                if seen[polynomial] != row:
                    raise ValueError(f"duplicate polynomial in different source rows: {hex(polynomial)}")
                continue  # A self-reciprocal row describes the same polynomial twice.
            seen[polynomial] = row
            candidates.append({"width": width, "polynomial": hex(polynomial), "koopman": hex(implicit),
                               "orientation": orientation, "hd": hd, "source": source["url"],
                               "source_sha256": source["sha256"], "raw_row": row})
    if not candidates:
        raise ValueError(f"no CRC-{width} candidates found; refusing an empty catalogue")
    return candidates


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=Path(__file__).resolve().parents[1] / "data/koopman")
    parser.add_argument("--widths", default="8,16,32,64")
    parser.add_argument("--refresh", action="store_true", help="fetch new HTML even if a snapshot exists")
    parser.add_argument("--offline", action="store_true", help="require existing HTML snapshots")
    args = parser.parse_args()
    if args.refresh and args.offline:
        parser.error("--refresh and --offline are mutually exclusive")
    widths = sorted(set(int(value) for value in args.widths.split(",")))
    if not widths or any(width not in (8, 16, 32, 64) for width in widths):
        parser.error("widths must be chosen from 8,16,32,64")
    sources, candidates = [], []
    for width in widths:
        url = BASE + f"crc{width}.html"
        path = args.directory / "sources" / f"crc{width}.html"
        stamp = path.with_suffix(".json")
        if args.refresh or not path.exists():
            if args.offline:
                parser.error(f"missing snapshot: {path}")
            with urlopen(url, timeout=60) as response:
                html = response.read()
            retrieved = datetime.datetime.now(datetime.timezone.utc).isoformat()
            atomic_write(path, html)
        else:
            html = path.read_bytes()
            metadata = json.loads(stamp.read_text()) if stamp.exists() else {}
            retrieved = metadata.get("retrieved_utc", datetime.datetime.fromtimestamp(path.stat().st_mtime, datetime.timezone.utc).isoformat())
        source = {"url": url, "sha256": hashlib.sha256(html).hexdigest(), "retrieved_utc": retrieved,
                  "file": f"sources/crc{width}.html", "license": LICENSE, "attribution": ATTRIBUTION}
        parsed = parse_page(html.decode("utf-8"), width, source)
        atomic_write(stamp, (json.dumps(source, indent=2) + "\n").encode())
        sources.append(source)
        candidates.extend(parsed)
    candidates.sort(key=lambda c: (c["width"], int(c["polynomial"], 16)))
    result = {"version": 1, "sources": sources, "candidates": candidates}
    output = args.directory / "catalogue.json"
    atomic_write(output, (json.dumps(result, indent=2) + "\n").encode())
    print(f"Wrote {len(candidates)} candidates to {output}")


if __name__ == "__main__":
    main()
