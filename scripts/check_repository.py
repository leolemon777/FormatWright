#!/usr/bin/env python3
"""Dependency-free checks for repository contracts."""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCHEMA_ROOT = ROOT / "schemas"
EXPECTED_SCHEMA_NAMES = {
    "probe",
    "plan",
    "job-event",
    "validation-report",
    "engine-manifest",
    "preset-library",
}
EXPECTED_WORKFLOWS = {f"GW-{number:02d}" for number in range(1, 13)}
EXACT_PACKAGE_MANAGER = re.compile(r"^pnpm@\d+\.\d+\.\d+$")


def check_schemas(errors: list[str]) -> None:
    found_names: set[str] = set()
    found_ids: set[str] = set()
    for path in sorted(SCHEMA_ROOT.glob("*/v1.schema.json")):
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            errors.append(f"{path.relative_to(ROOT)}: invalid JSON: {error}")
            continue

        name = path.parent.name
        found_names.add(name)
        schema_id = document.get("$id")
        if not isinstance(schema_id, str) or not schema_id.startswith(
            "urn:formatwright:schema:"
        ):
            errors.append(f"{path.relative_to(ROOT)}: missing canonical $id")
        elif schema_id in found_ids:
            errors.append(f"{path.relative_to(ROOT)}: duplicate $id {schema_id}")
        else:
            found_ids.add(schema_id)

        if document.get("$schema") != "https://json-schema.org/draft/2020-12/schema":
            errors.append(f"{path.relative_to(ROOT)}: unexpected JSON Schema draft")
        if document.get("type") != "object":
            errors.append(f"{path.relative_to(ROOT)}: top-level type must be object")
        schema_version = document.get("properties", {}).get("schema_version", {})
        if schema_version.get("const") != 1:
            errors.append(f"{path.relative_to(ROOT)}: schema_version must be const 1")

    missing = EXPECTED_SCHEMA_NAMES - found_names
    if missing:
        errors.append(f"missing schemas: {', '.join(sorted(missing))}")


def check_workflows(errors: list[str]) -> None:
    path = ROOT / "test-corpus" / "manifests" / "golden-workflows.toml"
    try:
        document = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        errors.append(f"{path.relative_to(ROOT)}: invalid TOML: {error}")
        return

    workflows = document.get("workflows", [])
    ids = {workflow.get("id") for workflow in workflows}
    if ids != EXPECTED_WORKFLOWS:
        errors.append(
            "golden workflow IDs differ: "
            f"expected {sorted(EXPECTED_WORKFLOWS)}, found {sorted(ids)}"
        )
    for workflow in workflows:
        spec = workflow.get("spec")
        if not isinstance(spec, str) or not spec.startswith(
            "docs/specs/GOLDEN_WORKFLOWS.md#"
        ):
            errors.append(f"{workflow.get('id')}: invalid spec link")


def check_required_files(errors: list[str]) -> None:
    required = [
        "SPEC_PLAN.md",
        "deny.toml",
        "README.md",
        "LICENSE",
        "SECURITY.md",
        "PRIVACY.md",
        "CONTRIBUTING.md",
        "docs/USER_GUIDE.md",
        "docs/MASTER_EXECUTION_PLAN.md",
        "docs/TROUBLESHOOTING.md",
        "docs/specs/GOLDEN_WORKFLOWS.md",
        "docs/specs/JOB_RECOVERY.md",
        "docs/specs/VALIDATION_RULES.md",
        "docs/specs/RESOURCE_SCHEDULER.md",
        "docs/specs/TRACEABILITY.md",
        "docs/security/THREAT_MODEL.md",
        "docs/security/ENGINE_SUPPLY_CHAIN.md",
        "docs/security/DEPENDENCY_AUDIT.md",
        "docs/security/FUZZING.md",
        "docs/release/RELEASE_CHECKLIST.md",
        "docs/release/SBOM.md",
        "docs/release/WINDOWS_PACKAGING.md",
        "docs/testing/SANDBOX_TESTS.md",
        "docs/testing/LARGE_FILE.md",
        "docs/testing/QUEUE_BRIDGE.md",
        "docs/testing/DURABLE_QUEUE.md",
        "docs/testing/STRUCTURED_SANDBOX.md",
        "docs/testing/IMAGE_SANDBOX.md",
        "docs/testing/HEIC_SANDBOX.md",
        "docs/testing/METADATA_SANDBOX.md",
        "docs/testing/BATCH_SANDBOX.md",
        "docs/testing/DOCUMENT_SANDBOX.md",
        "docs/testing/PDF_SANDBOX.md",
        "docs/testing/OFFICE_SANDBOX.md",
        "docs/testing/DESKTOP_MVP.md",
        "docs/testing/TEN_THOUSAND_CONVERSIONS.md",
        "docs/testing/MIXED_SCHEDULER.md",
        "docs/testing/JOB_EXECUTION_SERVICE.md",
        "docs/testing/PRESET_SANDBOX.md",
        "docs/testing/ZERO_NETWORK.md",
        "docs/adr/0005-bounded-desktop-queue-projection.md",
        "docs/adr/0006-cross-platform-process-tree-control.md",
        "schemas/preset-library/v1.schema.json",
        "apps/desktop/package.json",
        "apps/desktop/src-tauri/tauri.conf.json",
        "apps/desktop/src-tauri/tauri.windows.conf.json",
        "apps/desktop/src-tauri/capabilities/main.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "engines/README.md",
        "engines/manifests/README.md",
        "engines/manifests/templates/ffmpeg.windows-x86_64.template.json",
        "scripts/test_ffmpeg_sandbox.ps1",
        "scripts/test_large_file.ps1",
        "scripts/test_audio_sandbox.ps1",
        "scripts/test_gif_sandbox.ps1",
        "scripts/test_structured_sandbox.ps1",
        "scripts/test_image_sandbox.ps1",
        "scripts/test_heic_sandbox.ps1",
        "scripts/test_metadata_sandbox.ps1",
        "scripts/test_batch_sandbox.ps1",
        "scripts/test_document_sandbox.ps1",
        "scripts/test_pdf_sandbox.ps1",
        "scripts/test_office_sandbox.ps1",
        "scripts/test_zero_network.ps1",
        "scripts/test_mixed_scheduler.ps1",
        "scripts/test_preset_sandbox.ps1",
        "scripts/generate_sbom.py",
        "scripts/generate_checksums.py",
        "scripts/audit_dependencies.py",
        "fuzz/Cargo.toml",
        "fuzz/Cargo.lock",
        "fuzz/fuzz_targets/engine_manifest.rs",
        "fuzz/fuzz_targets/structured_file.rs",
    ]
    for relative in required:
        if not (ROOT / relative).is_file():
            errors.append(f"required file is missing: {relative}")


def read_json(relative: str, errors: list[str]) -> dict[str, object] | None:
    path = ROOT / relative
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        errors.append(f"{relative}: invalid JSON: {error}")
        return None
    if not isinstance(document, dict):
        errors.append(f"{relative}: top-level JSON value must be an object")
        return None
    return document


def check_desktop_contract(errors: list[str]) -> None:
    root_package = read_json("package.json", errors)
    tauri_config = read_json("apps/desktop/src-tauri/tauri.conf.json", errors)
    capability = read_json("apps/desktop/src-tauri/capabilities/main.json", errors)
    if root_package is not None:
        package_manager = root_package.get("packageManager")
        if not isinstance(package_manager, str) or not EXACT_PACKAGE_MANAGER.fullmatch(
            package_manager
        ):
            errors.append("package.json: packageManager must pin an exact pnpm version")
    if tauri_config is not None:
        build = tauri_config.get("build", {})
        security = tauri_config.get("app", {}).get("security", {})
        if not isinstance(build, dict) or build.get("frontendDist") != "../dist":
            errors.append("tauri.conf.json: production frontendDist must be ../dist")
        if not isinstance(build, dict) or build.get("beforeBuildCommand") != "pnpm build":
            errors.append("tauri.conf.json: beforeBuildCommand must run pnpm build")
        if not isinstance(security, dict) or not security.get("csp"):
            errors.append("tauri.conf.json: a non-empty CSP is required")
    if capability is not None:
        permissions = capability.get("permissions")
        expected_permissions = [
            "core:event:default",
            "dialog:allow-open",
            "dialog:allow-save",
        ]
        if permissions != expected_permissions:
            errors.append(
                "capabilities/main.json: desktop permissions must remain the reviewed event/dialog allowlist"
            )


def check_engine_templates(errors: list[str]) -> None:
    templates = sorted((ROOT / "engines" / "manifests" / "templates").glob("*.json"))
    if not templates:
        errors.append("no engine manifest templates found")
        return
    for path in templates:
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            errors.append(f"{path.relative_to(ROOT)}: invalid JSON: {error}")
            continue
        if document.get("schema_version") != 1:
            errors.append(f"{path.relative_to(ROOT)}: schema_version must be 1")
        if document.get("protocol_version") != 1:
            errors.append(f"{path.relative_to(ROOT)}: protocol_version must be 1")


def main() -> int:
    errors: list[str] = []
    check_schemas(errors)
    check_workflows(errors)
    check_required_files(errors)
    check_desktop_contract(errors)
    check_engine_templates(errors)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print(
        f"repository contracts valid: "
        f"{len(EXPECTED_SCHEMA_NAMES)} schemas, "
        f"{len(EXPECTED_WORKFLOWS)} golden workflows, desktop and engine contracts"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
