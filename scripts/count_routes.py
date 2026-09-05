#!/usr/bin/env python3
"""Count reachable conversion combos from the core routing table.

Single source of truth for the "N reachable routes" figure used in the
README badge and plan docs. Mirrors crates/core/src/capabilities.rs
(`supported_targets`) and crates/core/src/chain.rs (one-hop BFS over the
lossless intermediate whitelist). Canonical formats only: alias input
extensions (jpeg/yml/htm/tgz/...) normalize onto their canonical row, and
target alias variants are not double-counted. When the routing table
changes, extend TABLE below in the same commit and rerun.

Run: python scripts/count_routes.py [--table]
"""

from __future__ import annotations

import argparse

# Direct routes: canonical input extension -> declared direct targets.
# Keep in sync with supported_targets() in crates/core/src/capabilities.rs.
TABLE: dict[str, list[str]] = {
    "csv": ["csv", "json", "yaml", "xml"],
    "json": ["csv", "json", "yaml", "xml"],
    "yaml": ["csv", "json", "yaml", "xml"],
    "xml": ["csv", "json", "yaml", "xml"],
    "pdf": ["jpg", "png"],
    "heic": ["jpg", "png"],
    "heif": ["jpg", "png"],
    "docx": ["pdf", "txt", "md", "html", "epub", "odt"],
    "odt": ["pdf", "docx"],
    "pptx": ["pdf"],
    "xlsx": ["pdf"],
    "ods": ["pdf"],
    "odp": ["pdf"],
    "rtf": ["pdf"],
    "svg": ["pdf"],
    "md": ["pdf", "docx", "epub"],
    "html": ["pdf", "docx", "epub"],
    "txt": ["pdf", "docx", "epub"],
    "zip": ["tar.gz", "7z"],
    "tar.gz": ["zip", "7z"],
    "7z": ["zip", "tar.gz"],
    "eml": ["txt", "html"],
    "msg": ["txt", "html"],
    "png": ["webp", "avif", "tiff", "bmp", "pdf", "txt"],
    "jpg": ["webp", "avif", "tiff", "bmp", "pdf", "txt"],
    "tiff": ["webp", "avif", "png", "pdf", "txt"],
    "bmp": ["webp", "avif", "png", "pdf", "txt"],
    "psd": ["png", "jpg", "tiff"],
    "dng": ["png", "jpg", "tiff"],
    "cr2": ["png", "jpg", "tiff"],
    "cr3": ["png", "jpg", "tiff"],
    "arw": ["png", "jpg", "tiff"],
    "nef": ["png", "jpg", "tiff"],
    "orf": ["png", "jpg", "tiff"],
    "rw2": ["png", "jpg", "tiff"],
    "pef": ["png", "jpg", "tiff"],
    "raf": ["png", "jpg", "tiff"],
    "mov": ["mp4", "gif", "mp3"],
    "mkv": ["mp4", "gif", "mp3"],
    "avi": ["mp4", "gif", "mp3"],
    "webm": ["mp4", "gif", "mp3"],
    "mp4": ["mp4", "gif", "mp3"],
    "wav": ["m4a", "mp3", "wav"],
    "flac": ["m4a", "mp3", "wav"],
    "aac": ["m4a", "mp3", "wav"],
    "m4a": ["m4a", "mp3", "wav"],
    "ogg": ["m4a", "mp3", "wav"],
    "opus": ["m4a", "mp3", "wav"],
    "mp3": ["m4a", "mp3", "wav"],
}

# Whitelisted one-hop chain intermediates (chain.rs INTERMEDIATE_WHITELIST).
WHITELIST = {
    "html", "png", "jpg", "tiff", "pdf", "docx", "txt", "json", "csv", "yaml", "xml", "wav",
}


def count(table: dict[str, list[str]] | None = None) -> tuple[int, int]:
    table = table or TABLE
    inputs = {src: set(tgts) for src, tgts in table.items()}
    direct = chained = 0
    for src, tgts in inputs.items():
        direct += len(tgts - {src})
        reach: set[str] = set()
        for mid in (tgts & WHITELIST) - {src}:
            reach |= (inputs.get(mid, set()) - {mid, src}) - tgts
        chained += len(reach)
    return direct, chained


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--table", action="store_true", help="print the routing table")
    args = parser.parse_args()
    if args.table:
        for src in sorted(TABLE):
            print(f"{src} -> {', '.join(TABLE[src])}")
        return 0
    direct, chained = count()
    print(f"{direct} direct + {chained} chained = {direct + chained} reachable routes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
