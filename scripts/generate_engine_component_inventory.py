#!/usr/bin/env python3
"""Batch-C preparation: transitive component inventory for engine packs.

Reads each Starter pack manifest, enumerates declared executables and
runtime files, runs the pack binaries to capture build/version evidence
(FFmpeg configuration flags, library versions, Poppler version), and maps
every DLL / configure flag onto a curated component knowledge base with
license, upstream URL, and patent notes. Unrecognized binaries are listed
as `unmapped` - the review must identify them; nothing is guessed.

Outputs:
  --out JSON inventory (.artifacts by default)
  --emit-workbook PATH  regenerate the markdown review workbook
                        (docs/security/ENGINE_COMPONENT_REVIEW.md)

The tool is evidence collection, not legal review: every component row
starts `review_status: pending` and only the human sign-off recorded in
the workbook may promote it (ENGINE_SUPPLY_CHAIN.md rule).

Stdlib only.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

# component name -> (upstream, license, patent_note)
# Sources: upstream project license files; entries are engineering
# knowledge for review routing, NOT legal conclusions.
COMPONENT_KB: dict[str, tuple[str, str, str]] = {
    "ffmpeg": ("https://ffmpeg.org", "LGPL-2.1-or-later (GPL with --enable-gpl; GPL-3.0-or-later with --enable-version3)", "Codec patents must be reviewed per region"),
    "libavutil": ("https://ffmpeg.org", "LGPL-2.1-or-later / GPL per build", ""),
    "libavcodec": ("https://ffmpeg.org", "LGPL-2.1-or-later / GPL per build", "Contains codec implementations; patent review required"),
    "libavformat": ("https://ffmpeg.org", "LGPL-2.1-or-later / GPL per build", ""),
    "libavfilter": ("https://ffmpeg.org", "LGPL-2.1-or-later / GPL per build", ""),
    "libavdevice": ("https://ffmpeg.org", "LGPL-2.1-or-later / GPL per build", ""),
    "libswresample": ("https://ffmpeg.org", "LGPL-2.1-or-later / GPL per build", ""),
    "libswscale": ("https://ffmpeg.org", "LGPL-2.1-or-later / GPL per build", ""),
    "libpostproc": ("https://ffmpeg.org", "GPL-2.0-or-later (always)", ""),
    "libx264": ("https://www.videolan.org/developers/x264.html", "GPL-2.0-or-later", "Commercial encoder license option exists"),
    "libx265": ("https://bitbucket.org/multicoreware/x265", "GPL-2.0-or-later", "Commercial encoder license option exists"),
    "libxvid": ("https://www.xvid.com", "GPL-2.0-or-later", ""),
    "libvidstab": ("https://github.com/georgmartius/vid.stab", "GPL-2.0-or-later", ""),
    "libaom": ("https://aomedia.org", "BSD-2-Clause + patent license", "AV1 patent pool notice required"),
    "libdav1d": ("https://code.videolan.org/videolan/dav1d", "BSD-2-Clause", "AV1 decode only"),
    "libsvtav1": ("https://gitlab.com/AOMediaCodec/SVT-AV1", "BSD-2-Clause + patent license", "AV1 patent pool notice required"),
    "libvpx": ("https://www.webmproject.org", "BSD-3-Clause", "VP8/VP9 patents granted royalty-free by Google; verify terms"),
    "libopus": ("https://www.opus-codec.org", "BSD-3-Clause", "Opus declared patent-royalty-free; verify Xiph/Patent License"),
    "libmp3lame": ("https://lame.sourceforge.io", "LGPL-2.0-or-later", "MP3 patents expired (2021 worldwide); confirm"),
    "libvorbis": ("https://xiph.org/vorbis", "BSD-3-Clause", ""),
    "libtheora": ("https://www.theora.org", "BSD-3-Clause + Xiph patent grant", ""),
    "libspeex": ("https://www.speex.org", "BSD-3-Clause", ""),
    "libopencore-amrnb": ("https://sourceforge.net/projects/opencore-amr", "Apache-2.0", "AMR patent status must be reviewed"),
    "libopencore-amrwb": ("https://sourceforge.net/projects/opencore-amr", "Apache-2.0", "AMR patent status must be reviewed"),
    "libvo-amrwbenc": ("https://sourceforge.net/projects/opencore-amr", "Apache-2.0", "AMR encoding patents must be reviewed"),
    "libgsm": ("https://www.quut.com/gsm", "ISC-like (specific license text)", "GSM 06.10 patent status must be reviewed"),
    "libass": ("https://github.com/libass/libass", "ISC", ""),
    "libfreetype": ("https://freetype.org", "FreeType License OR GPL-2.0-or-later", ""),
    "freetype": ("https://freetype.org", "FreeType License OR GPL-2.0-or-later", ""),
    "libfontconfig": ("https://www.freedesktop.org/wiki/Software/fontconfig", "MIT-style permissive", ""),
    "fontconfig": ("https://www.freedesktop.org/wiki/Software/fontconfig", "MIT-style permissive", ""),
    "libfribidi": ("https://github.com/fribidi/fribidi", "LGPL-2.1-or-later", ""),
    "fribidi": ("https://github.com/fribidi/fribidi", "LGPL-2.1-or-later", ""),
    "libharfbuzz": ("https://harfbuzz.github.io", "Old MIT", ""),
    "harfbuzz": ("https://harfbuzz.github.io", "Old MIT", ""),
    "libwebp": ("https://developers.google.com/speed/webp", "BSD-3-Clause", ""),
    "libsharpyuv": ("https://developers.google.com/speed/webp", "BSD-3-Clause", ""),
    "libopenjpeg": ("https://github.com/uclouvain/openjpeg", "BSD-2-Clause", "JPEG 2000 patents largely expired; verify"),
    "openjp2": ("https://github.com/uclouvain/openjpeg", "BSD-2-Clause", ""),
    "libtiff": ("https://libtiff.gitlab.io/libtiff/", "libtiff (MIT-like)", ""),
    "tiff": ("https://libtiff.gitlab.io/libtiff/", "libtiff (MIT-like)", ""),
    "libpng": ("https://libpng.org", "libpng-2.0 (PNG Reference License)", ""),
    "libpng16": ("https://libpng.org", "libpng-2.0 (PNG Reference License)", ""),
    "zlib": ("https://zlib.net", "Zlib", ""),
    "zlib1": ("https://zlib.net", "Zlib", ""),
    "deflate": ("https://github.com/richgel999/miniz", "MIT", ""),
    "brotli": ("https://github.com/google/brotli", "MIT", ""),
    "libbrotlicommon": ("https://github.com/google/brotli", "MIT", ""),
    "libbrotlidec": ("https://github.com/google/brotli", "MIT", ""),
    "libbrotlienc": ("https://github.com/google/brotli", "MIT", ""),
    "liblzma": ("https://tukaani.org/xz/", "0BSD", ""),
    "lzma": ("https://tukaani.org/xz/", "0BSD (XZ embedded / lzma)", ""),
    "libopenmpt": ("https://lib.openmpt.org", "BSD-3-Clause", ""),
    "librubberband": ("https://breakfastquay.com/rubberband/", "GPL-2.0-or-later (commercial option)", ""),
    "zstd": ("https://facebook.github.io/zstd/", "BSD-3-Clause OR GPL-2.0-or-later", ""),
    "libzstd": ("https://facebook.github.io/zstd/", "BSD-3-Clause OR GPL-2.0-or-later", ""),
    "bz2": ("https://sourceware.org/bzip2/", "bzip2 license (BSD-style)", ""),
    "bzlib": ("https://sourceware.org/bzip2/", "bzip2 license (BSD-style)", ""),
    "expat": ("https://libexpat.github.io", "MIT", ""),
    "libexpat": ("https://libexpat.github.io", "MIT", ""),
    "libxml2": ("https://gitlab.gnome.org/GNOME/libxml2", "MIT", ""),
    "iconv": ("https://www.gnu.org/software/libiconv/", "LGPL-2.1-or-later", ""),
    "charset": ("https://www.gnu.org/software/libiconv/", "LGPL-2.1-or-later (libiconv)", ""),
    "gettext": ("https://www.gnu.org/software/gettext/", "LGPL-2.1-or-later (runtime)", ""),
    "libintl": ("https://www.gnu.org/software/gettext/", "LGPL-2.1-or-later (runtime)", ""),
    "glib": ("https://wiki.gnome.org/Projects/GLib", "LGPL-2.1-or-later", ""),
    "gobject": ("https://wiki.gnome.org/Projects/GLib", "LGPL-2.1-or-later", ""),
    "gio": ("https://wiki.gnome.org/Projects/GLib", "LGPL-2.1-or-later", ""),
    "cairo": ("https://cairographics.org", "LGPL-2.1-or-later OR MPL-1.1", ""),
    "pixman": ("https://cairographics.org", "MIT", ""),
    "pixman-1-0": ("https://cairographics.org", "MIT", ""),
    "pango": ("https://pango.gnome.org", "LGPL-2.0-or-later", ""),
    "lcms2": ("https://www.littlecms.com", "MIT", ""),
    "jpeg": ("https://libjpeg-turbo.org", "IJG license OR BSD-3-Clause (turbo)", ""),
    "jpeg8": ("https://libjpeg-turbo.org", "IJG license OR BSD-3-Clause (turbo)", ""),
    "libcurl": ("https://curl.se", "curl license (MIT-like)", ""),
    "curl": ("https://curl.se", "curl license (MIT-like)", ""),
    "libssh2": ("https://libssh2.org", "BSD-3-Clause", ""),
    "libssh": ("https://www.libssh.org", "LGPL-2.1-or-later", ""),
    "libcrypto": ("https://openssl.org", "Apache-2.0", ""),
    "libssl": ("https://openssl.org", "Apache-2.0", ""),
    "gnutls": ("https://gnutls.org", "LGPL-2.1-or-later", ""),
    "nettle": ("https://www.lysator.liu.se/~nisse/nettle/", "LGPL-3.0-or-later OR GPL-2.0-or-later", ""),
    "gmp": ("https://gmplib.org", "LGPL-3.0-or-later OR GPL-2.0-or-later", ""),
    "srt": ("https://github.com/Haivision/srt", "MPL-2.0", ""),
    "libsrt": ("https://github.com/Haivision/srt", "MPL-2.0", ""),
    "libzmq": ("https://zeromq.org", "LGPL-3.0-or-later", ""),
    "openmpt": ("https://lib.openmpt.org", "BSD-3-Clause", ""),
    "libgme": ("https://bitbucket.org/mpyne/game-music-emu", "LGPL-2.1-or-later", ""),
    "rubberband": ("https://breakfastquay.com/rubberband/", "GPL-2.0-or-later (commercial option)", ""),
    "libzimg": ("https://github.com/sekrit-twc/zimg", "MIT", ""),
    "libvmaf": ("https://github.com/Netflix/vmaf", "BSD-2-Clause + Patent", "VMAF patent notice required"),
    "boost": ("https://boost.org", "BSL-1.0", ""),
    "lerc": ("https://github.com/Esri/lerc", "Apache-2.0", ""),
    "poppler": ("https://poppler.freedesktop.org", "GPL-2.0-or-later", ""),
    "poppler-data": ("https://poppler.freedesktop.org", "Adobe CMap/unicode data redistribution terms + BSD-style poppler-data license", "Free redistribution permitted with notices; verify Adobe terms"),
    "poppler-cpp": ("https://poppler.freedesktop.org", "GPL-2.0-or-later", ""),
    "poppler-glib": ("https://poppler.freedesktop.org", "GPL-2.0-or-later", ""),
    "nspr": ("https://firefox-source-docs.mozilla.org/nspr/", "MPL-2.0", ""),
    "nss": ("https://firefox-source-docs.mozilla.org/nss/", "MPL-2.0", ""),
    "sqlite": ("https://sqlite.org", "Public Domain", ""),
    "nsis": ("https://nsis.sourceforge.io", "Zlib", ""),
}

FLAG_COMPONENTS = {
    "libx264", "libx265", "libxvid", "libvidstab", "libaom", "libdav1d",
    "libsvtav1", "libvpx", "libopus", "libmp3lame", "libvorbis", "libtheora",
    "libspeex", "libopencore-amrnb", "libopencore-amrwb", "libvo-amrwbenc",
    "libgsm", "libass", "libfreetype", "libfontconfig", "libfribidi",
    "libharfbuzz", "libwebp", "libopenjpeg", "libxml2", "libzimg", "libvmaf",
    "libsrt", "libssh", "libzmq", "libgme", "libopenmpt", "librubberband",
    "gnutls", "gmp", "bzlib", "lzma", "zlib", "iconv", "fontconfig", "cairo",
}

# DLL name (lowercase, extension stripped, trailing digits/version kept in
# canonical form) -> component
DLL_COMPONENTS = {
    "freetype": "libfreetype", "freetype-6": "libfreetype",
    "fontconfig-1": "libfontconfig", "fontconfig": "libfontconfig",
    "fribidi-0": "libfribidi", "fribidi": "libfribidi",
    "harfbuzz-0": "libharfbuzz", "harfbuzz": "libharfbuzz",
    "libpng16-16": "libpng", "libpng16": "libpng",
    "zlib1": "zlib", "zlib": "zlib",
    "liblzma-5": "liblzma", "liblzma": "liblzma",
    "libzstd": "zstd", "zstd": "zstd",
    "libbrotlicommon": "brotli", "libbrotlidec": "brotli", "libbrotlienc": "brotli",
    "brotlicommon": "brotli", "brotlidec": "brotli", "brotlienc": "brotli",
    "jpeg8": "jpeg", "jpeg62": "jpeg", "jpeg-8": "jpeg",
    "openjp2": "libopenjpeg",
    "libtiff-6": "libtiff", "tiff": "libtiff", "tiff-6": "libtiff", "libtiff": "libtiff",
    "lcms2": "lcms2",
    "libcurl": "libcurl", "curl": "libcurl",
    "libssh2": "libssh2",
    "libcrypto-3-x64": "libcrypto", "libssl-3-x64": "libssl",
    "libexpat": "expat", "expat": "expat",
    "iconv": "iconv", "charset": "charset",
    "libintl-8": "gettext", "intl-8": "gettext",
    "glib-2.0": "glib", "gobject-2.0": "gobject", "gio-2.0": "gio",
    "cairo-2": "cairo", "cairo": "cairo",
    "pixman-1-0": "pixman", "pixman-1": "pixman",
    "pango-1.0": "pango", "pango": "pango",
    "lerc": "lerc",
    "deflate": "deflate",
    "bz2": "bz2", "libbz2": "bz2",
    "poppler": "poppler", "poppler-cpp": "poppler-cpp", "poppler-glib": "poppler-glib",
    "libsharpyuv": "libsharpyuv",
    "libwebp-7": "libwebp", "libwebp": "libwebp",
    "libwebpdemux-2": "libwebp", "libwebpmux-3": "libwebp",
}


def dll_component(file_name: str) -> str | None:
    stem = file_name.lower()
    if stem.endswith(".dll"):
        stem = stem[:-4]
    return DLL_COMPONENTS.get(stem)


def run_capture(executable: Path, args: list[str]) -> str:
    try:
        result = subprocess.run(
            [str(executable), *args], capture_output=True, text=True, timeout=30, check=False
        )
        return (result.stdout or "") + (result.stderr or "")
    except (OSError, subprocess.TimeoutExpired):
        return ""


def inventory_media_pack(pack_root: Path, manifest: dict) -> dict:
    binaries = {
        entry["name"]: pack_root / entry["relative_path"]
        for entry in manifest.get("executables", [])
    }
    version_text = run_capture(binaries["ffmpeg"], ["-version"]) if "ffmpeg" in binaries else ""
    version_match = re.search(r"ffmpeg version (\S+)", version_text)
    configuration = ""
    for line in version_text.splitlines():
        if line.strip().startswith("configuration:"):
            configuration = line.split("configuration:", 1)[1].strip()
            break
    flags = sorted(re.findall(r"--(?:enable|disable)-([a-z0-9][a-z0-9-]*)", configuration))
    library_versions = {}
    for line in version_text.splitlines():
        match = re.match(r"\s*(lib[a-z]+)\s+([\d.]+)", line)
        if match:
            library_versions[match.group(1)] = match.group(2)

    flag_components = sorted(
        flag for flag in flags if flag in FLAG_COMPONENTS
    )
    runtime_unmapped = []
    data_files = []
    for entry in manifest.get("runtime_files", []):
        name = Path(entry["relative_path"]).name
        if name.lower().endswith((".dll", ".exe")):
            component = dll_component(name)
            if component is None:
                runtime_unmapped.append(entry["relative_path"])
                continue
            components.setdefault(component, kb_row(component))
        else:
            data_files.append(entry["relative_path"])
    components = {}
    for component in ["ffmpeg", *sorted(library_versions)]:
        components[component] = kb_row(component)
    for component in flag_components:
        if component not in components:
            components[component] = kb_row(component)
    nonfree = "--enable-nonfree" in configuration
    gpl = "--enable-gpl" in configuration
    version3 = "--enable-version3" in configuration
    build_license = "LGPL-2.1-or-later"
    if gpl:
        build_license = "GPL-3.0-or-later" if version3 else "GPL-2.0-or-later"
    return {
        "pack": "media",
        "engine_id": manifest["engine_id"],
        "version": manifest["version"],
        "ffmpeg_version": version_match.group(1) if version_match else "unknown",
        "configuration": configuration,
        "configure_flags": flags,
        "detected_external_components": flag_components,
        "ffmpeg_library_versions": library_versions,
        "build_license": build_license,
        "nonfree_build": nonfree,
        "components": components,
        "runtime_files": [entry["relative_path"] for entry in manifest.get("runtime_files", [])],
        "unmapped_runtime_files": runtime_unmapped,
        "non_binary_data_files": data_files,
    }


def inventory_pdf_pack(pack_root: Path, manifest: dict) -> dict:
    binaries = {
        entry["name"]: pack_root / entry["relative_path"]
        for entry in manifest.get("executables", [])
    }
    version_text = run_capture(binaries["pdftoppm"], ["-v"]) if "pdftoppm" in binaries else ""
    version_match = re.search(r"pdftoppm version (\S+)", version_text)
    components = {"poppler": kb_row("poppler")}
    runtime_unmapped = []
    data_files = []
    runtime_components = {}
    all_files = [entry["relative_path"] for entry in manifest.get("runtime_files", [])]
    all_files += [entry["relative_path"] for entry in manifest.get("executables", [])]
    for relative in all_files:
        name = Path(relative).name
        lowered = relative.lower()
        if lowered.startswith("share/poppler/") or lowered.startswith("licenses/"):
            data_files.append(relative)
            components.setdefault("poppler-data", kb_row("poppler-data"))
            continue
        if name.lower() in ("pdftoppm.exe", "pdfinfo.exe"):
            continue  # poppler tools themselves, already covered by "poppler"
        component = dll_component(name)
        if component is None:
            if name.lower().endswith((".dll", ".exe")):
                runtime_unmapped.append(relative)
            else:
                data_files.append(relative)
            continue
        runtime_components[component] = kb_row(component)
    components.update(runtime_components)
    return {
        "pack": "pdf",
        "engine_id": manifest["engine_id"],
        "version": manifest["version"],
        "poppler_version": version_match.group(1) if version_match else "unknown",
        "build_license": "GPL-2.0-or-later (poppler core)",
        "nonfree_build": False,
        "components": components,
        "runtime_files": all_files,
        "unmapped_runtime_files": sorted(set(runtime_unmapped)),
        "non_binary_data_files": sorted(set(data_files)),
    }


def kb_row(component: str) -> dict:
    upstream, license_text, patent = COMPONENT_KB.get(
        component, ("UNKNOWN", "UNKNOWN", "UNKNOWN")
    )
    return {
        "component": component,
        "upstream": upstream,
        "license": license_text,
        "patent_note": patent,
        "review_status": "pending",
    }


def collect(starter_root: Path) -> dict:
    packs = []
    for pack_name, inventory in (
        ("media", inventory_media_pack),
        ("pdf", inventory_pdf_pack),
    ):
        manifest_path = starter_root / pack_name / "manifest.json"
        if not manifest_path.is_file():
            continue
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        packs.append(inventory(starter_root / pack_name, manifest))
    return {
        "schema_version": 1,
        "generated_utc": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
        "starter_root": str(starter_root),
        "review_rule": "All rows are pending until the human sign-off recorded in docs/security/ENGINE_COMPONENT_REVIEW.md; this inventory is evidence collection, not a legal conclusion.",
        "packs": packs,
    }


def emit_workbook(inventory: dict, path: Path) -> None:
    lines = [
        "# Engine Component Review Workbook (Batch C)",
        "",
        f"- Generated: {inventory['generated_utc']} by `scripts/generate_engine_component_inventory.py`",
        "- Status: **pending human review** - regeneratable, but the Decision/Signature columns are hand-maintained",
        "- Rule: a component is only `reviewed` with a named signature and date (`ENGINE_SUPPLY_CHAIN.md` §6);",
        "  `sources.json.review_status` may then be promoted by the same signature.",
        "",
    ]
    for pack in inventory["packs"]:
        lines += [
            f"## {pack['engine_id']} {pack['version']} (pack `{pack['pack']}`)",
            "",
            f"- Build license verdict: **{pack['build_license']}**"
            + ("  - ⚠️ `--enable-nonfree` PRESENT - MUST NOT SHIP" if pack["nonfree_build"] else ""),
        ]
        if pack.get("configuration"):
            lines.append(f"- FFmpeg configuration: `{pack['configuration']}`")
        if pack.get("ffmpeg_library_versions"):
            libs = ", ".join(f"{k} {v}" for k, v in pack["ffmpeg_library_versions"].items())
            lines.append(f"- FFmpeg libraries: {libs}")
        lines += [
            "",
            "| Component | Upstream | License (engineering KB) | Patent note | Review status | Reviewer / date |",
            "|---|---|---|---|---|---|",
        ]
        for row in pack["components"].values():
            lines.append(
                f"| {row['component']} | {row['upstream']} | {row['license']} |"
                f" {row['patent_note'] or '—'} | {row['review_status']} | — |"
            )
        if pack.get("unmapped_runtime_files"):
            lines += [
                "",
                "**Unmapped binaries (identify before review closes):**",
                "",
            ]
            lines += [f"- `{name}`" for name in pack["unmapped_runtime_files"]]
        data_count = len(pack.get("non_binary_data_files", []))
        if data_count:
            lines += [
                "",
                f"Plus {data_count} non-binary data/license files (poppler share data, notices) -"
                " covered by the `poppler-data` row above.",
            ]
        lines.append("")
    path.write_text("\n".join(lines), encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--starter-root",
        default="dist/engine-packs/windows-x86_64/starter",
        help="starter pack root containing media/ and pdf/",
    )
    parser.add_argument("--out", default=".artifacts/engine-component-inventory/inventory.json")
    parser.add_argument("--emit-workbook", default=None, help="markdown workbook output path")
    args = parser.parse_args(argv)

    starter_root = Path(args.starter_root)
    if not starter_root.is_dir():
        raise SystemExit(f"starter root not found: {starter_root}")
    inventory = collect(starter_root)

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(inventory, indent=2) + "\n", encoding="utf-8")

    if args.emit_workbook:
        emit_workbook(inventory, Path(args.emit_workbook))

    nonfree = [pack["engine_id"] for pack in inventory["packs"] if pack["nonfree_build"]]
    if nonfree:
        print(f"FAIL-CLOSED: nonfree build detected in {nonfree}; pack must not ship", file=sys.stderr)
        return 2
    total = sum(len(pack["components"]) for pack in inventory["packs"])
    unmapped = sum(len(pack.get("unmapped_runtime_files", [])) for pack in inventory["packs"])
    print(f"inventory written to {out_path} ({total} components, {unmapped} unmapped files)")
    for pack in inventory["packs"]:
        print(
            f"  {pack['engine_id']}: build license {pack['build_license']},"
            f" {len(pack['components'])} components,"
            f" {len(pack.get('unmapped_runtime_files', []))} unmapped"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
