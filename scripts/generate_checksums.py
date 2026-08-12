#!/usr/bin/env python3
"""Generate a deterministic SHA256SUMS manifest for explicit release artifacts."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("artifacts", nargs="+", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "dist" / "SHA256SUMS")
    args = parser.parse_args()

    output = args.output.resolve()
    files = sorted({path.resolve() for path in args.artifacts}, key=lambda path: path.name)
    if output in files:
        parser.error("the checksum manifest cannot include itself")
    for path in files:
        if not path.is_file():
            parser.error(f"artifact is not a regular file: {path}")
    duplicate_names = {path.name for path in files if sum(item.name == path.name for item in files) > 1}
    if duplicate_names:
        parser.error(f"artifact basenames must be unique: {', '.join(sorted(duplicate_names))}")

    output.parent.mkdir(parents=True, exist_ok=True)
    contents = "".join(f"{sha256(path)}  {path.name}\n" for path in files)
    output.write_text(contents, encoding="utf-8", newline="\n")
    print(f"SHA256SUMS: {len(files)} artifacts -> {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
