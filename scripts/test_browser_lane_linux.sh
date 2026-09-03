#!/usr/bin/env bash
set -e
export PATH="$HOME/.cargo/bin:$HOME/miniforge/envs/ocr/bin:$PATH"
export FORMATWRIGHT_ENGINE_MSEDGE=$HOME/browsers/chrome-linux64/chrome
export FORMATWRIGHT_ENGINE_PDFINFO="$HOME/miniforge/envs/ocr/bin/pdfinfo"
export FORMATWRIGHT_ENGINE_PDFTOPPM="$HOME/miniforge/envs/ocr/bin/pdftoppm"
export FORMATWRIGHT_ENGINE_PDFTOTEXT="$HOME/miniforge/envs/ocr/bin/pdftotext"
export FORMATWRIGHT_ENGINE_PDFFONTS="$HOME/miniforge/envs/ocr/bin/pdffonts"
export FORMATWRIGHT_ENGINE_QPDF="$HOME/miniforge/envs/ocr/bin/qpdf"
cd "${FW_SRC:-/home/leo/linux-runs/FormatWright/src}"
cargo build -p formatwright-cli 2>&1 | tail -1
FX=/home/leo/linux-runs/FormatWright/fixtures
rm -f /tmp/svg-linux.pdf
./target/debug/formatwright convert $FX/sample.svg --to pdf --output /tmp/svg-linux.pdf 2>&1 | tail -2
"$HOME/miniforge/envs/ocr/bin/pdftotext" /tmp/svg-linux.pdf - 2>/dev/null | grep -m1 "SVG MATRIX"
rm -f /tmp/html-linux.pdf
./target/debug/formatwright convert $FX/sample.html --to pdf --output /tmp/html-linux.pdf 2>&1 | tail -2
"$HOME/miniforge/envs/ocr/bin/pdftotext" /tmp/html-linux.pdf - 2>/dev/null | grep -m1 "MATRIX HTML"
echo "BROWSER-LANE-LINUX-PASS"
