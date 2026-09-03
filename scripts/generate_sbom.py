#!/usr/bin/env python3
"""Generate a deterministic-package SPDX 2.3 application SBOM."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import re
import shutil
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def command_json(command: list[str]) -> object:
    executable = shutil.which(command[0]) or command[0]
    resolved = Path(executable)
    if os.name == "nt" and resolved.suffix.lower() in {".bat", ".cmd"}:
        command = [os.environ.get("COMSPEC", "cmd.exe"), "/d", "/c", executable, *command[1:]]
    else:
        command = [executable, *command[1:]]
    completed = subprocess.run(
        command,
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return json.loads(completed.stdout)


def spdx_id(ecosystem: str, name: str, version: str) -> str:
    readable = re.sub(r"[^A-Za-z0-9.-]", "-", f"{ecosystem}-{name}-{version}")
    suffix = hashlib.sha256(f"{ecosystem}:{name}:{version}".encode()).hexdigest()[:12]
    return f"SPDXRef-{readable}-{suffix}"


def license_value(value: object) -> str:
    return value if isinstance(value, str) and value.strip() else "NOASSERTION"


def cargo_packages(cargo: str) -> tuple[list[dict[str, object]], list[dict[str, str]]]:
    metadata = command_json([cargo, "metadata", "--format-version", "1", "--locked"])
    assert isinstance(metadata, dict)
    workspace_ids = set(metadata.get("workspace_members", []))
    packages: list[dict[str, object]] = []
    relationships: list[dict[str, str]] = []
    id_map: dict[str, str] = {}
    for package in metadata.get("packages", []):
        name = str(package["name"])
        version = str(package["version"])
        identifier = spdx_id("cargo", name, version)
        id_map[str(package["id"])] = identifier
        packages.append(
            {
                "SPDXID": identifier,
                "name": name,
                "versionInfo": version,
                "downloadLocation": str(package.get("source") or "NOASSERTION"),
                "filesAnalyzed": False,
                "licenseConcluded": "NOASSERTION",
                "licenseDeclared": license_value(package.get("license")),
                "copyrightText": "NOASSERTION",
                "externalRefs": [
                    {
                        "referenceCategory": "PACKAGE-MANAGER",
                        "referenceType": "purl",
                        "referenceLocator": f"pkg:cargo/{name}@{version}",
                    }
                ],
            }
        )
        if str(package["id"]) in workspace_ids:
            relationships.append(
                {
                    "spdxElementId": "SPDXRef-DOCUMENT",
                    "relationshipType": "DESCRIBES",
                    "relatedSpdxElement": identifier,
                }
            )
    resolve = metadata.get("resolve") or {}
    for node in resolve.get("nodes", []):
        source = id_map.get(str(node.get("id")))
        if source is None:
            continue
        for dependency in node.get("dependencies", []):
            target = id_map.get(str(dependency))
            if target is not None:
                relationships.append(
                    {
                        "spdxElementId": source,
                        "relationshipType": "DEPENDS_ON",
                        "relatedSpdxElement": target,
                    }
                )
    return packages, relationships


def pnpm_packages(pnpm: str) -> list[dict[str, object]]:
    inventory = command_json([pnpm, "licenses", "list", "--json", "--prod"])
    assert isinstance(inventory, dict)
    packages: list[dict[str, object]] = []
    for entries in inventory.values():
        for entry in entries:
            for version in entry.get("versions", []):
                name = str(entry["name"])
                version = str(version)
                packages.append(
                    {
                        "SPDXID": spdx_id("npm", name, version),
                        "name": name,
                        "versionInfo": version,
                        "downloadLocation": str(entry.get("homepage") or "NOASSERTION"),
                        "filesAnalyzed": False,
                        "licenseConcluded": "NOASSERTION",
                        "licenseDeclared": license_value(entry.get("license")),
                        "copyrightText": "NOASSERTION",
                        "externalRefs": [
                            {
                                "referenceCategory": "PACKAGE-MANAGER",
                                "referenceType": "purl",
                                "referenceLocator": f"pkg:npm/{name}@{version}",
                            }
                        ],
                    }
                )
    return packages


def creation_time() -> str:
    epoch = os.environ.get("SOURCE_DATE_EPOCH")
    moment = (
        dt.datetime.fromtimestamp(int(epoch), tz=dt.timezone.utc)
        if epoch is not None
        else dt.datetime.now(tz=dt.timezone.utc)
    )
    return moment.replace(microsecond=0).isoformat().replace("+00:00", "Z")


def validate_document(document: dict[str, object]) -> None:
    packages = document["packages"]
    relationships = document["relationships"]
    assert isinstance(packages, list)
    assert isinstance(relationships, list)
    identifiers = [str(package["SPDXID"]) for package in packages]
    if len(identifiers) != len(set(identifiers)):
        raise ValueError("SBOM contains duplicate SPDX package identifiers")
    known = {"SPDXRef-DOCUMENT", *identifiers}
    for relationship in relationships:
        if (
            relationship["spdxElementId"] not in known
            or relationship["relatedSpdxElement"] not in known
        ):
            raise ValueError("SBOM relationship references an unknown SPDX identifier")
    serialized = json.dumps(document)
    if re.search(r"[A-Za-z]:\\\\", serialized):
        raise ValueError("SBOM must not contain a Windows absolute installation path")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--pnpm", default="pnpm")
    parser.add_argument("--output", type=Path, default=ROOT / "dist" / "sbom.spdx.json")
    args = parser.parse_args()
    cargo, relationships = cargo_packages(args.cargo)
    npm = pnpm_packages(args.pnpm)
    packages = sorted(cargo + npm, key=lambda item: str(item["SPDXID"]))
    lock_digest = hashlib.sha256(
        (ROOT / "Cargo.lock").read_bytes() + (ROOT / "pnpm-lock.yaml").read_bytes()
    ).hexdigest()
    document = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": "Anole-application-SBOM",
        "documentNamespace": f"https://formatwright.local/spdx/{lock_digest}",
        "creationInfo": {
            "created": creation_time(),
            "creators": ["Tool: Anole scripts/generate_sbom.py"],
        },
        "packages": packages,
        "relationships": sorted(
            relationships,
            key=lambda item: (
                item["spdxElementId"],
                item["relationshipType"],
                item["relatedSpdxElement"],
            ),
        ),
    }
    validate_document(document)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    print(f"SPDX SBOM: {len(packages)} packages -> {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
