# Engine Distribution and License Inventory

- Status: Phase 1 decision record
- Snapshot: 2026-08-10
- Legal status: engineering inventory, not legal advice

FormatWright keeps engines outside the Apache-2.0 application package unless a reviewed engine pack includes exact source, build, hash, notice, and SBOM evidence. System-discovered tools remain `unverified`; an executable is never described as certified merely because it runs.

## Inventory

| Engine | Intended v0.1 role | Upstream licensing baseline | Initial distribution decision | Required pack evidence |
|---|---|---|---|---|
| FFmpeg + ffprobe | Media inspect, remux, transcode | FFmpeg states LGPL-2.1-or-later by default; optional GPL parts and external libraries can change the whole binary's obligations | Two separate packs: a restricted LGPL candidate with `--disable-gpl --disable-nonfree`, and an explicitly labeled optional GPL media pack. Never promote the currently discovered Gyan full build to a default certified pack | Exact configure output, source revision/archive, every external library, source offer, notices, protocols/codecs snapshot, binary hashes, SBOM |
| libvips | Bounded-memory image conversion | Upstream repository identifies LGPL-2.1 | Separate image pack after dependency audit; do not assume loaders/savers inherit only the libvips license | libvips plus every codec/delegate library, build flags, notices, source and hashes |
| LibreOffice | Office to PDF | LibreOffice says released builds are subject to MPL-2.0 and include components under other licenses; contributions are jointly MPL-2.0/LGPL-3.0-or-later | v0.1 first discovers or imports an official user installation. Repackaging is deferred until the complete installed license inventory and source-delivery path pass review | Exact official package identity, installation license directory, component notices, source links, headless-profile isolation evidence |
| Pandoc | Markdown/HTML/document transforms | Upstream states GPL-2.0-or-later, with documented compatible exceptions | Optional separate document pack or system discovery; never merge its binary into the Apache application artifact | COPYRIGHT/COPYING, exact source tag, transitive license inventory, binary hash, templates/data-file inventory |
| qpdf | Structural PDF inspect, repair, merge, split | Current qpdf is Apache-2.0; embedded zlib/JPEG and optional crypto providers have separate notices | Preferred default structural PDF candidate after dependency/SBOM review | qpdf NOTICE/LICENSE, crypto-provider selection, embedded dependency notices, signed upstream release verification, hashes |
| PDFium | PDF rendering and visual validation | BSD-style core with a substantial third-party dependency notice set | Dedicated rendering pack only; pin a source revision and generate a complete third-party notice from the actual GN target | Source revision, GN args, generated third-party notice, sandbox evidence, binary/shared-library hashes, SBOM |
| Ghostscript | Possible PDF/PostScript fallback | License choice requires a separate AGPL/commercial decision | Excluded from the default v0.1 pack | Explicit legal decision before any adapter or distribution work |

Primary references:

- [FFmpeg legal and compliance checklist](https://www.ffmpeg.org/legal.html)
- [libvips upstream repository and license](https://github.com/libvips/libvips)
- [LibreOffice license page](https://www.libreoffice.org/licenses/)
- [Pandoc COPYRIGHT and license exceptions](https://github.com/jgm/pandoc/blob/main/COPYRIGHT)
- [qpdf upstream license and distribution verification](https://github.com/qpdf/qpdf)
- [Chromium third-party license tooling used for PDFium notices](https://chromium.googlesource.com/chromium/src.git/+/HEAD/tools/licenses/licenses.py)

## Certification boundary

The runtime verifier currently proves schema/protocol compatibility, safe relative paths, host OS/architecture, executable SHA-256 values, and presence of declared license/source-offer files. A signature field is only reported as present. It is not trusted until Phase 5 introduces a pinned public-key keyring, revocation data, and verified signature algorithm.

## Development discovery

`formatwright doctor` may inspect programs already installed by the developer. Those identities are useful for tests but always remain `unverified` and carry no redistribution conclusion.
