# Format and Platform Support Matrix

- Status: Phase 0 baseline
- Version: 0.1
- Updated: 2026-08-18

## 1. Support labels

- Certified: all golden fixtures pass on the platform with a certified engine pack.
- Experimental: adapter exists, but the full fixture/platform matrix has not passed.
- Detected: Doctor can inspect the engine, but the workflow is not claimed.
- Unsupported: planning must reject the workflow.

Marketing and UI must use these labels. Engine-advertised formats are never automatically described as Certified.

## 2. Candidate v0.1 platform matrix

The following is the test target, not yet a support claim:

| Platform | Architecture | Alpha target | Public Beta gate |
|---|---|---|---|
| Windows 11 | x86_64 | Primary development platform | Installer, process-tree cancellation, long paths, signing |
| macOS | Apple Silicon | Phase 1 CI and Phase 4 desktop | Signed/notarized app, engine packs, Finder action |
| macOS | x86_64 | Build verification | Best-effort Beta unless full golden matrix is available |
| Ubuntu LTS | x86_64 | Phase 1 CI | AppImage, engine discovery, file-manager action |

Minimum exact OS releases must be frozen in ADR-0005 after Tauri, WebView, code-signing, and engine-pack tests. Windows 10 is not a default Public Beta target because its general support lifecycle has ended; a community build may remain possible.

## 3. Filesystem behavior

| Environment | v0.1 behavior |
|---|---|
| Local NTFS/APFS/ext4 | Required and tested |
| Removable local drive | Supported when staging and final output share a filesystem |
| Network share/UNC | Experimental; no atomic guarantee unless proven |
| Cloud-synced hydrated file | Best effort; source identity is rechecked |
| Cloud placeholder | Block until hydrated locally |
| Directory symlink | Not traversed by default |
| File symlink | Allowed only inside authorized input root |
| Hardlink | Read as a file; output hardlink identity is not preserved |
| Remote URL | Unsupported in v0.1 |

## 4. Workflow matrix

| Workflow | Inputs | Outputs | Primary engine | Initial status |
|---|---|---|---|---|
| GW-01 | HEIC, HEIF | JPG, PNG | libvips target; libheif development fallback | Experimental on Windows |
| GW-02 | PNG, JPG | WebP, AVIF | libvips (FFmpeg development fallback) | Experimental on Windows |
| GW-03 | Supported image directory | WebP, AVIF, JPG, PNG | libvips (FFmpeg development fallback) | Experimental on Windows |
| GW-04 | MOV, MKV, AVI, WebM | MP4 | FFmpeg | Experimental on Windows |
| GW-05 | Video containers with audio | MP3, M4A, WAV | FFmpeg | Experimental on Windows |
| GW-06 | Supported video | GIF | FFmpeg | Experimental on Windows |
| GW-07 | FLAC, WAV, MP3, AAC, M4A, OGG, Opus | Selected audio target | FFmpeg | Experimental on Windows |
| GW-08 | DOCX, PPTX, XLSX | PDF | LibreOffice + Poppler validation | Experimental on Windows |
| GW-09 | PDF | PNG, JPG | Poppler pdfinfo/pdftoppm | Experimental on Windows |
| GW-10 | Markdown, HTML, plain text, SVG | PDF, DOCX, EPUB | HTML/SVG→PDF: system-discovered Edge print + Poppler vector validation (preferred); Markdown/plain text keeps Pandoc + LibreOffice | Experimental on Windows (browser lane: formal sandbox evidence 2026-09-01, `scripts/test_browser_print_sandbox.ps1`) |
| GW-11 | CSV, JSON, YAML, XML | CSV, JSON, YAML, XML | Rust native | Experimental on Windows |
| GW-12 | Supported media/document | Cleaned copy | Type-specific adapter | Experimental media slice on Windows |

Windows Starter Media（FFmpeg）为本机 GW-04/05/06/07 切片提供 Experimental 证据（沙箱 remux、Explorer Convert to MP4 等）。全部行仍非 Certified：干净机 / 全 fixture / 签名包未关闭。

GW-10 的浏览器打印 lane（ADR-0012）：HTML/HTM 与新增 SVG 输入在开发构建下经系统发现的 Edge 无头打印产出矢量 PDF，并用 pdfinfo/pdftoppm/pdftotext/pdffonts 验证（文字层可提取、字体全内嵌）；HTML 保留 Pandoc lane 作为回退，SVG 仅此 lane。Release 构建仍需激活已验证引擎包。

## 5. MP4 planning baseline

Dynamic engine inspection remains authoritative, but the first planner fixture uses:

- H.264/AVC video: remux candidate.
- H.265/HEVC video: remux candidate subject to selected compatibility profile and tags.
- AAC audio: remux candidate.
- MP3 audio: allowed only when the selected MP4 profile accepts it; otherwise planned audio transcode.
- VP8/VP9 video: transcode candidate.
- Opus audio: explicit compatibility decision; never silently retained.
- Text subtitles: convert, drop, or externalize only when shown in the Plan.
- Image-based subtitles: do not silently discard.

## 6. Engine certification record

Each certified row records:

- Engine and semantic version.
- Binary SHA-256.
- Build configuration.
- Platform and architecture.
- Capability manifest hash.
- Fixture suite revision.
- Pass date.
- Known warnings.
- License review ID.

## 7. Promotion rule

A status moves from Planned to Experimental when the adapter and at least one end-to-end fixture pass. It moves to Certified only after all required fixtures and release gates pass on the named platform.
