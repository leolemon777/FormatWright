#!/usr/bin/env python3
"""Dependency-free regression test for deterministic engine-pack SPDX output."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
GENERATOR = ROOT / "scripts" / "generate_engine_sbom.py"


def sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def run(*arguments: str, success: bool = True) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        [sys.executable, str(GENERATOR), *arguments],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if success and completed.returncode != 0:
        raise AssertionError(completed.stderr or completed.stdout)
    if not success and completed.returncode == 0:
        raise AssertionError("engine SBOM command unexpectedly succeeded")
    return completed


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="formatwright-engine-sbom-") as temporary:
        pack = Path(temporary)
        (pack / "bin").mkdir()
        (pack / "runtime").mkdir()
        (pack / "licenses").mkdir()
        executable = b"engine fixture"
        runtime = b"runtime fixture"
        (pack / "bin" / "fixture.bin").write_bytes(executable)
        (pack / "runtime" / "fixture.dat").write_bytes(runtime)
        (pack / "licenses" / "NOTICE.txt").write_text("fixture notice\n", encoding="utf-8")
        manifest = {
            "schema_version": 1,
            "engine_id": "fixture-engine",
            "version": "1.0.0",
            "platform": "linux",
            "architecture": "x86_64",
            "protocol_version": 1,
            "formatwright_compatibility": {"minimum": "0.1.0", "maximum_exclusive": "0.2.0"},
            "executables": [
                {"name": "fixture", "relative_path": "bin/fixture.bin", "sha256": sha256(executable)}
            ],
            "runtime_files": [
                {"relative_path": "runtime/fixture.dat", "sha256": sha256(runtime)}
            ],
            "source": {
                "project_url": "https://example.invalid/fixture",
                "source_url": "https://example.invalid/fixture/source.tar.xz",
                "source_revision": "v1.0.0",
                "build_configuration": "test-only",
            },
            "licenses": [
                {"spdx": "Apache-2.0", "notice_path": "licenses/NOTICE.txt", "source_offer_path": None}
            ],
            "capabilities": [],
            "signature": None,
        }
        manifest_path = pack / "manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
        first = pack / "first.spdx.json"
        second = pack / "second.spdx.json"
        common = ("--manifest", str(manifest_path), "--source-date-epoch", "0")
        run(*common, "--output", str(first))
        run(*common, "--output", str(second))
        if first.read_bytes() != second.read_bytes():
            raise AssertionError("engine SBOM generation is not deterministic")
        run("--manifest", str(manifest_path), "--verify", str(first))
        document = json.loads(first.read_text(encoding="utf-8"))
        if len(document["files"]) != 3 or document["creationInfo"]["created"] != "1970-01-01T00:00:00Z":
            raise AssertionError("engine SBOM does not contain the expected deterministic inventory")

        sources = b'{"schema_version":1,"engine_id":"fixture-engine","version":"1.0.0","artifacts":[{}]}\n'
        (pack / "sources.json").write_bytes(sources)
        manifest["supply_chain"] = {
            "sbom_path": "first.spdx.json",
            "sbom_sha256": sha256(first.read_bytes()),
            "sources_path": "sources.json",
            "sources_sha256": sha256(sources),
        }
        manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
        sidecar = pack / "sidecar.spdx.json"
        run(*common, "--output", str(sidecar))
        sidecar_document = json.loads(sidecar.read_text(encoding="utf-8"))
        if not any(entry["fileName"] == "sources.json" for entry in sidecar_document["files"]):
            raise AssertionError("engine SBOM omitted the source inventory sidecar")

        manifest["supply_chain"] = None
        manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")

        (pack / "runtime" / "fixture.dat").write_bytes(b"tampered")
        run("--manifest", str(manifest_path), "--verify", str(first), success=False)

        manifest["runtime_files"][0]["relative_path"] = "../outside"
        manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
        run(*common, "--output", str(second), success=False)

    print("engine SBOM regression valid: deterministic, complete, tamper and traversal checks pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
