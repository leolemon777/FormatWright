#!/usr/bin/env python3
"""Generate and verify a deterministic SPDX 2.3 engine-pack file SBOM."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import re
from pathlib import Path, PurePosixPath
from typing import Any


SHA256 = re.compile(r"^[0-9a-fA-F]{64}$")


def safe_relative_path(value: object) -> str:
    if not isinstance(value, str) or not value or "\\" in value:
        raise ValueError(f"unsafe pack path: {value!r}")
    path = PurePosixPath(value)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts):
        raise ValueError(f"unsafe pack path: {value!r}")
    return path.as_posix()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def sha1_file(path: Path) -> str:
    digest = hashlib.sha1()  # noqa: S324 - SPDX package verification requires SHA-1.
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def spdx_id(kind: str, value: str) -> str:
    readable = re.sub(r"[^A-Za-z0-9.-]", "-", value)
    suffix = hashlib.sha256(f"{kind}:{value}".encode()).hexdigest()[:12]
    return f"SPDXRef-{kind}-{readable}-{suffix}"


def creation_time(epoch: int | None) -> str:
    effective = epoch
    if effective is None:
        effective = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))
    moment = dt.datetime.fromtimestamp(effective, tz=dt.timezone.utc)
    return moment.replace(microsecond=0).isoformat().replace("+00:00", "Z")


def manifest_inventory(manifest_path: Path) -> tuple[dict[str, Any], list[dict[str, str]]]:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if not isinstance(manifest, dict):
        raise ValueError("engine manifest must be an object")
    root = manifest_path.resolve().parent
    inventory: dict[str, str | None] = {}

    def add(relative_value: object, expected_hash: object | None) -> None:
        relative = safe_relative_path(relative_value)
        if relative in inventory:
            raise ValueError(f"duplicate manifest path: {relative}")
        if expected_hash is not None and (
            not isinstance(expected_hash, str) or not SHA256.fullmatch(expected_hash)
        ):
            raise ValueError(f"invalid SHA-256 for {relative}")
        inventory[relative] = expected_hash.lower() if isinstance(expected_hash, str) else None

    for entry in manifest.get("executables", []):
        add(entry.get("relative_path"), entry.get("sha256"))
    for entry in manifest.get("runtime_files", []):
        add(entry.get("relative_path"), entry.get("sha256"))
    for entry in manifest.get("licenses", []):
        add(entry.get("notice_path"), None)
        if entry.get("source_offer_path") is not None:
            add(entry.get("source_offer_path"), None)
    supply_chain = manifest.get("supply_chain")
    if isinstance(supply_chain, dict):
        # The source inventory is ordinary pack content. The SPDX document
        # cannot contain its own hash without creating a circular identity.
        add(supply_chain.get("sources_path"), supply_chain.get("sources_sha256"))

    files: list[dict[str, str]] = []
    for relative, expected in sorted(inventory.items()):
        path = (root / relative).resolve()
        if root not in path.parents or not path.is_file():
            raise ValueError(f"declared pack file is unavailable or escapes the pack: {relative}")
        observed = sha256_file(path)
        if expected is not None and observed != expected:
            raise ValueError(
                f"manifest hash mismatch for {relative}: expected {expected}, observed {observed}"
            )
        files.append({"relative": relative, "sha256": observed, "sha1": sha1_file(path)})
    if not files:
        raise ValueError("engine manifest declares no files")
    return manifest, files


def license_expression(manifest: dict[str, Any]) -> str:
    licenses = sorted(
        {
            entry.get("spdx")
            for entry in manifest.get("licenses", [])
            if isinstance(entry.get("spdx"), str) and entry.get("spdx").strip()
        }
    )
    return " AND ".join(licenses) if licenses else "NOASSERTION"


def generate(manifest_path: Path, epoch: int | None) -> dict[str, Any]:
    manifest, inventory = manifest_inventory(manifest_path)
    engine_id = str(manifest.get("engine_id", ""))
    version = str(manifest.get("version", ""))
    platform = str(manifest.get("platform", ""))
    architecture = str(manifest.get("architecture", ""))
    source = manifest.get("source") or {}
    if not engine_id or not version:
        raise ValueError("engine manifest identity is incomplete")

    package_id = spdx_id("Package", f"{engine_id}-{version}-{platform}-{architecture}")
    files = []
    relationships = [
        {
            "spdxElementId": "SPDXRef-DOCUMENT",
            "relationshipType": "DESCRIBES",
            "relatedSpdxElement": package_id,
        }
    ]
    for entry in inventory:
        file_id = spdx_id("File", entry["relative"])
        files.append(
            {
                "SPDXID": file_id,
                "fileName": entry["relative"],
                "checksums": [
                    {"algorithm": "SHA1", "checksumValue": entry["sha1"]},
                    {"algorithm": "SHA256", "checksumValue": entry["sha256"]},
                ],
                "licenseConcluded": "NOASSERTION",
                "copyrightText": "NOASSERTION",
            }
        )
        relationships.append(
            {
                "spdxElementId": package_id,
                "relationshipType": "CONTAINS",
                "relatedSpdxElement": file_id,
            }
        )

    verification = hashlib.sha1(  # noqa: S324 - required by SPDX 2.3.
        "".join(sorted(entry["sha1"] for entry in inventory)).encode()
    ).hexdigest()
    identity_digest = hashlib.sha256(
        json.dumps(
            {
                "engine_id": engine_id,
                "version": version,
                "platform": platform,
                "architecture": architecture,
                "files": inventory,
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    document = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"FormatWright-{engine_id}-{version}-engine-pack-SBOM",
        "documentNamespace": f"https://formatwright.local/spdx/engine/{identity_digest}",
        "creationInfo": {
            "created": creation_time(epoch),
            "creators": ["Tool: FormatWright scripts/generate_engine_sbom.py"],
        },
        "packages": [
            {
                "SPDXID": package_id,
                "name": engine_id,
                "versionInfo": version,
                "downloadLocation": str(source.get("source_url") or "NOASSERTION"),
                "filesAnalyzed": True,
                "packageVerificationCode": {"packageVerificationCodeValue": verification},
                "licenseConcluded": "NOASSERTION",
                "licenseDeclared": license_expression(manifest),
                "copyrightText": "NOASSERTION",
                "externalRefs": [
                    {
                        "referenceCategory": "PACKAGE-MANAGER",
                        "referenceType": "purl",
                        "referenceLocator": (
                            f"pkg:generic/{engine_id}@{version}?os={platform}&arch={architecture}"
                        ),
                    }
                ],
            }
        ],
        "files": files,
        "relationships": sorted(
            relationships,
            key=lambda item: (
                item["spdxElementId"],
                item["relationshipType"],
                item["relatedSpdxElement"],
            ),
        ),
        "annotations": [
            {
                "annotationDate": creation_time(epoch),
                "annotationType": "OTHER",
                "annotator": "Tool: FormatWright scripts/generate_engine_sbom.py",
                "comment": (
                    "Complete hash inventory of manifest-declared binary pack files. "
                    "Component attribution and legal review status are recorded separately "
                    "in sources.json; this file inventory alone is not certification."
                ),
            }
        ],
    }
    verify_document(document, manifest_path)
    return document


def verify_document(document: dict[str, Any], manifest_path: Path) -> None:
    manifest, expected_files = manifest_inventory(manifest_path)
    if document.get("spdxVersion") != "SPDX-2.3" or document.get("dataLicense") != "CC0-1.0":
        raise ValueError("engine SBOM is not SPDX 2.3 JSON")
    if document.get("SPDXID") != "SPDXRef-DOCUMENT":
        raise ValueError("engine SBOM has no canonical document SPDXID")
    packages = document.get("packages")
    if not isinstance(packages, list) or not any(
        package.get("name") == manifest.get("engine_id")
        and package.get("versionInfo") == manifest.get("version")
        for package in packages
        if isinstance(package, dict)
    ):
        raise ValueError("engine SBOM does not describe the manifest identity")

    observed: dict[str, str] = {}
    files = document.get("files")
    if not isinstance(files, list):
        raise ValueError("engine SBOM has no file inventory")
    for entry in files:
        if not isinstance(entry, dict):
            raise ValueError("engine SBOM file entry is not an object")
        relative = safe_relative_path(entry.get("fileName"))
        hashes = {
            checksum.get("algorithm"): checksum.get("checksumValue")
            for checksum in entry.get("checksums", [])
            if isinstance(checksum, dict)
        }
        value = hashes.get("SHA256")
        if not isinstance(value, str) or not SHA256.fullmatch(value):
            raise ValueError(f"engine SBOM file has no SHA-256: {relative}")
        if relative in observed:
            raise ValueError(f"engine SBOM repeats a file: {relative}")
        observed[relative] = value.lower()
    expected = {entry["relative"]: entry["sha256"] for entry in expected_files}
    if observed != expected:
        raise ValueError("engine SBOM file inventory differs from the manifest-declared payload")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--verify", type=Path)
    parser.add_argument("--source-date-epoch", type=int)
    args = parser.parse_args()
    manifest_path = args.manifest.resolve()
    if args.verify is not None:
        document = json.loads(args.verify.read_text(encoding="utf-8"))
        if not isinstance(document, dict):
            raise ValueError("engine SBOM must be a JSON object")
        verify_document(document, manifest_path)
        print(f"Engine SPDX SBOM valid: {len(document.get('files', []))} files")
        return 0
    if args.output is None:
        parser.error("--output is required unless --verify is used")
    document = generate(manifest_path, args.source_date_epoch)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    print(f"Engine SPDX SBOM: {len(document['files'])} files -> {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
