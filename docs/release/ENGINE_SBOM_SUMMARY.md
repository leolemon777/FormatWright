# Engine SBOM Summary (development inventory)

- Snapshot: 2026-09-03, Windows development host
- License posture: all system-discovered (ADR-0011/0012); none bundled with the app
- The pinned Windows Starter pack (Poppler 26.02.0 + FFmpeg) carries its own hash-checked manifests

| Engine | Version | SHA-256 (first 16) | Discovery override |
|---|---|---|---|
| poppler-pdfinfo | 26.02.0 | 34040ff62bef73d6 | `FORMATWRIGHT_ENGINE_PDFINFO` |
| poppler-pdftoppm | 26.02.0 | 575a2f66073b256f | `FORMATWRIGHT_ENGINE_PDFTOPPM` |
| poppler-pdftotext | 26.02.0 | c7392e92727abbb5 | `FORMATWRIGHT_ENGINE_PDFTOTEXT` |
| poppler-pdffonts | 26.02.0 | 5866916326bf67de | `FORMATWRIGHT_ENGINE_PDFFONTS` |
| qpdf | 12.4.1 | 57c003e868fb66cd | `FORMATWRIGHT_ENGINE_QPDF` |
| libreoffice-soffice | 26.2.4.2 | 5d8954c7fbe457b5 | `FORMATWRIGHT_ENGINE_SOFFICE` |
| libheif-heif-dec | 1.23.2 | 7eb55f6c2e8d80fa | `FORMATWRIGHT_ENGINE_DEC` |
| pandoc | 3.8 | 58cee786007a7ba0 | `FORMATWRIGHT_ENGINE_PANDOC` |
| ffmpeg | 8.1.1 | 09948d4cdd0650da | `FORMATWRIGHT_ENGINE_FFMPEG` |
| ffprobe | 8.1.1 | a6618e99bb58869d | `FORMATWRIGHT_ENGINE_FFPROBE` |

Linux runner equivalents live in a conda-forge env (tesseract 5.5.3, poppler, qpdf, ffmpeg, pandoc); OCR engine is optional and doctor reports EngineMissing without it.