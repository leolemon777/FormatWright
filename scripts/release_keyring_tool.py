#!/usr/bin/env python3
"""Release keyring tooling for ADR-0011 engine-manifest signatures.

Modes:
  keygen  Generate an Ed25519 seed (PRIVATE - ceremony host only) and the
          matching public release-keyring JSON entry.
  sign    Sign one engine manifest in place with a seed, producing the
          `signature` field over the canonical manifest bytes.

Verification is deliberately NOT implemented here: use the real gate
`formatwright engines verify <manifest> --keyring <keyring.json>` so every
signature is checked by the shipped Rust implementation.

Canonical manifest bytes (must match `formatwright_engine_sdk::
canonical_manifest_bytes` byte-for-byte, cross-validated 2026-08-15):
compact JSON, schema struct field order, `signature` set to null,
capability `constraints` maps sorted by key, non-ASCII unescaped.

Requires PyNaCl (pip install pynacl). Dependency-free stdlib otherwise.
"""

from __future__ import annotations

import argparse
import json
import secrets
import sys
import time
from pathlib import Path

import nacl.signing

CANONICAL_FIELD_ORDER = [
    "schema_version",
    "engine_id",
    "version",
    "platform",
    "architecture",
    "protocol_version",
    "formatwright_compatibility",
    "executables",
    "runtime_files",
    "source",
    "licenses",
    "supply_chain",
    "capabilities",
    "signature",
]


def canonical_manifest_bytes(manifest: dict) -> bytes:
    value = dict(manifest)
    value["signature"] = None
    ordered = {key: value[key] for key in CANONICAL_FIELD_ORDER if key in value}
    ordered.update({key: item for key, item in value.items() if key not in CANONICAL_FIELD_ORDER})
    for capability in ordered.get("capabilities", []):
        if isinstance(capability.get("constraints"), dict):
            capability["constraints"] = dict(sorted(capability["constraints"].items()))
    return json.dumps(ordered, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def parse_seed(seed_text: str) -> bytes:
    seed = bytes.fromhex(seed_text.strip())
    if len(seed) != 32:
        raise SystemExit("seed file must contain exactly 32 bytes as hex")
    return seed


def command_keygen(args: argparse.Namespace) -> None:
    seed = secrets.token_bytes(32)
    key = nacl.signing.SigningKey(seed)
    now_ms = int(time.time() * 1000)
    keyring = {
        "schema_version": 1,
        "keys": [
            {
                "key_id": args.key_id,
                "algorithm": "ed25519",
                "purpose": "engine-manifest",
                "public_key": bytes(key.verify_key).hex(),
                "valid_from_unix_ms": now_ms,
                "valid_until_unix_ms": now_ms + args.valid_days * 86_400_000,
            }
        ],
        "revocations": [],
    }
    seed_path = Path(args.seed_out)
    keyring_path = Path(args.keyring_out)
    if seed_path.exists() or keyring_path.exists():
        raise SystemExit("refusing to overwrite an existing seed or keyring file")
    seed_path.write_text(seed.hex() + "\n", encoding="utf-8")
    keyring_path.write_text(json.dumps(keyring, indent=2) + "\n", encoding="utf-8")
    print(f"PRIVATE seed written to {seed_path} - protect and never commit it")
    print(f"public keyring entry written to {keyring_path}")
    print(f"key_id={args.key_id} public_key={bytes(key.verify_key).hex()}")


def command_sign(args: argparse.Namespace) -> None:
    manifest_path = Path(args.manifest)
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("signature") is not None:
        raise SystemExit("manifest already carries a signature; refusing to re-sign in place")
    key = nacl.signing.SigningKey(parse_seed(Path(args.seed).read_text(encoding="utf-8")))
    signature = key.sign(canonical_manifest_bytes(manifest)).signature
    manifest["signature"] = {
        "algorithm": "ed25519",
        "key_id": args.key_id,
        "value": signature.hex(),
    }
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"signed {manifest_path} as key {args.key_id}")
    print("verify with: formatwright engines verify <manifest> --keyring <keyring.json>")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)

    keygen = commands.add_parser("keygen", help="generate a release seed and keyring entry")
    keygen.add_argument("--key-id", required=True, help="[a-z0-9][a-z0-9._-]+ e.g. release-2026h2")
    keygen.add_argument("--valid-days", type=int, default=540, help="key validity window")
    keygen.add_argument("--seed-out", required=True, help="private seed output (hex)")
    keygen.add_argument("--keyring-out", required=True, help="public keyring JSON output")
    keygen.set_defaults(handler=command_keygen)

    sign = commands.add_parser("sign", help="sign one engine manifest in place")
    sign.add_argument("--manifest", required=True, help="path to manifest.json")
    sign.add_argument("--seed", required=True, help="private seed file (hex)")
    sign.add_argument("--key-id", required=True, help="key id the seed belongs to")
    sign.set_defaults(handler=command_sign)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    args.handler(args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
