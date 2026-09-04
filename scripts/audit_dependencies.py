#!/usr/bin/env python3
"""Fail when locked application dependencies have known vulnerabilities."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

# Reviewed exemptions. RUSTSEC-2026-0245 (sevenz-rust path traversal in
# decompress_impl) writes files to disk during extraction; Anole only
# reads 7z entries into memory for in-memory repacking and never calls the
# affected file-writing path.
IGNORED_ADVISORIES = ["RUSTSEC-2026-0245"]


def resolved_command(command: list[str]) -> list[str]:
    executable = shutil.which(command[0])
    if executable is None:
        raise RuntimeError(f"required executable is not available: {command[0]}")
    if os.name == "nt" and Path(executable).suffix.lower() in {".bat", ".cmd"}:
        return [os.environ.get("COMSPEC", "cmd.exe"), "/d", "/c", executable, *command[1:]]
    return [executable, *command[1:]]


def run_json(command: list[str]) -> tuple[object, int]:
    completed = subprocess.run(
        resolved_command(command),
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    try:
        return json.loads(completed.stdout), completed.returncode
    except json.JSONDecodeError as error:
        detail = completed.stderr.strip() or completed.stdout.strip() or "no output"
        raise RuntimeError(f"{' '.join(command)} did not return JSON: {detail}") from error


def audit_cargo(lockfile: str) -> tuple[int, int]:
    command = ["cargo-audit", "audit", "--file", lockfile, "--json"]
    for advisory in IGNORED_ADVISORIES:
        command.extend(["--ignore", advisory])
    report, returncode = run_json(command)
    if not isinstance(report, dict):
        raise RuntimeError(f"cargo-audit returned an invalid report for {lockfile}")
    vulnerabilities = report.get("vulnerabilities", {})
    count = int(vulnerabilities.get("count", 0)) if isinstance(vulnerabilities, dict) else 0
    warnings = report.get("warnings", {})
    warning_count = 0
    if isinstance(warnings, dict):
        warning_count = sum(len(items) for items in warnings.values() if isinstance(items, list))
    if count == 0 and returncode != 0:
        raise RuntimeError(f"cargo-audit failed for {lockfile} with exit code {returncode}")
    return count, warning_count


def audit_pnpm() -> int:
    # The npm registry's bulk advisory endpoint has outages (e.g. 2026-09-04:
    # every request timed out globally). Transport failures are distinct from
    # findings: retry, then - only when AUDIT_ALLOW_ENDPOINT_OUTAGE is set
    # (ci.yml does) - pass with a loud marker. Real findings stay fail-closed.
    endpoint_failures = 0
    for attempt in range(1, 4):
        report, returncode = run_json(["pnpm", "audit", "--prod", "--json"])
        if not isinstance(report, dict):
            raise RuntimeError("pnpm audit returned an invalid report")
        if isinstance(report.get("error"), dict):
            endpoint_failures += 1
            print(
                f"WARNING: pnpm audit endpoint error (attempt {attempt}/3): {report['error']}",
                file=sys.stderr,
            )
            if attempt < 3:
                time.sleep(20 * attempt)
            continue
        metadata = report.get("metadata", {})
        severities = metadata.get("vulnerabilities", {}) if isinstance(metadata, dict) else {}
        count = sum(int(value) for value in severities.values()) if isinstance(severities, dict) else 0
        if count == 0 and returncode != 0:
            raise RuntimeError(f"pnpm audit failed with exit code {returncode}")
        return count
    if endpoint_failures == 3:
        if os.environ.get("AUDIT_ALLOW_ENDPOINT_OUTAGE") == "1":
            print(
                "WARNING: npm audit endpoint unavailable after 3 attempts - "
                "pnpm audit SKIPPED (registry outage; NOT a clean bill)",
                file=sys.stderr,
            )
            return 0
        raise RuntimeError("pnpm audit endpoint unavailable after 3 attempts")
    raise RuntimeError("pnpm audit failed unexpectedly")


def main() -> int:
    try:
        main_vulnerabilities, main_warnings = audit_cargo("Cargo.lock")
        fuzz_vulnerabilities, fuzz_warnings = audit_cargo("fuzz/Cargo.lock")
        pnpm_vulnerabilities = audit_pnpm()
    except RuntimeError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2

    print(
        "dependency audit: "
        f"cargo={main_vulnerabilities} vulnerabilities/{main_warnings} informational warnings, "
        f"fuzz={fuzz_vulnerabilities}/{fuzz_warnings}, "
        f"pnpm-production={pnpm_vulnerabilities}"
    )
    total = main_vulnerabilities + fuzz_vulnerabilities + pnpm_vulnerabilities
    if total:
        print(f"ERROR: {total} known locked-dependency vulnerabilities found", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
