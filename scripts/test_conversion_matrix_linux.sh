#!/usr/bin/env bash
# Linux-side conversion matrix: engines from the user-level conda env.
set -u
SRC="${FW_SRC:-/home/leo/linux-runs/FormatWright/src}"
FX="${FW_FIXTURES:-/home/leo/linux-runs/FormatWright/fixtures}"
OUT="${FW_OUT:-/home/leo/linux-runs/FormatWright/out}"
mkdir -p "$FX" "$OUT"
export PATH="$HOME/.cargo/bin:$HOME/miniforge/envs/ocr/bin:$PATH"
export FORMATWRIGHT_ENGINE_PDFINFO="$HOME/miniforge/envs/ocr/bin/pdfinfo"
export FORMATWRIGHT_ENGINE_PDFTOPPM="$HOME/miniforge/envs/ocr/bin/pdftoppm"
export FORMATWRIGHT_ENGINE_PDFTOTEXT="$HOME/miniforge/envs/ocr/bin/pdftotext"
export FORMATWRIGHT_ENGINE_PDFFONTS="$HOME/miniforge/envs/ocr/bin/pdffonts"
export FORMATWRIGHT_ENGINE_QPDF="$HOME/miniforge/envs/ocr/bin/qpdf"
export FORMATWRIGHT_ENGINE_FFMPEG="$HOME/miniforge/envs/ocr/bin/ffmpeg"
export FORMATWRIGHT_ENGINE_FFPROBE="$HOME/miniforge/envs/ocr/bin/ffprobe"
export FORMATWRIGHT_ENGINE_TESSERACT="$HOME/miniforge/envs/ocr/bin/tesseract"

PY="$HOME/miniforge/envs/ocr/bin/python"
"$PY" - <<'PYEOF'
import json, csv, io, tarfile, zipfile
data = [{"id": 1, "name": "alpha"}, {"id": 2, "name": "beta"}]
with open('/home/leo/linux-runs/FormatWright/fixtures/sample.csv','w',newline='') as f:
    w = csv.DictWriter(f, fieldnames=['id','name']); w.writeheader(); w.writerows(data)
json.dump(data, open('/home/leo/linux-runs/FormatWright/fixtures/sample.json','w'))
open('/home/leo/linux-runs/FormatWright/fixtures/sample.yaml','w').write("- id: 1\n  name: alpha\n- id: 2\n  name: beta\n")
open('/home/leo/linux-runs/FormatWright/fixtures/sample.md','w').write('# Matrix\n\nELECTRIC 440010147700 content 12345.\n')
open('/home/leo/linux-runs/FormatWright/fixtures/sample.txt','w').write('Plain matrix ELECTRIC 998877.\n')
open('/home/leo/linux-runs/FormatWright/fixtures/sample.html','w').write('<!DOCTYPE html><html><head><meta charset="UTF-8"></head><body><p>MATRIX HTML 440010147700</p></body></html>')
open('/home/leo/linux-runs/FormatWright/fixtures/sample.svg','w').write('<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200"><rect x="4" y="4" width="292" height="192" fill="none" stroke="black"/><text x="20" y="60" font-size="20">SVG MATRIX 440</text></svg>')
with zipfile.ZipFile('/home/leo/linux-runs/FormatWright/fixtures/sample.zip','w') as z:
    z.writestr('a.txt','alpha ELECTRIC'); z.writestr('b/c.txt','charlie matrix')
with tarfile.open('/home/leo/linux-runs/FormatWright/fixtures/sample.tar.gz','w:gz') as tf:
    d=b'tar.gz matrix ELECTRIC 440'; ti=tarfile.TarInfo('cross.txt'); ti.size=len(d)
    tf.addfile(ti, io.BytesIO(d))
from PIL import Image, ImageDraw
img = Image.new('RGB',(1240,1754),'white'); ImageDraw.Draw(img).text((100,200),'MATRIX IMAGE 440010147700',fill='black'); img.save('/home/leo/linux-runs/FormatWright/fixtures/sample.png'); img.save('/home/leo/linux-runs/FormatWright/fixtures/sample.jpg',quality=90)
img.save('/home/leo/linux-runs/FormatWright/fixtures/sample.tiff'); img.save('/home/leo/linux-runs/FormatWright/fixtures/sample.bmp')
open('/home/leo/linux-runs/FormatWright/fixtures/sample.eml','w').write('From: a@example.org
To: b@example.org
Subject: Matrix EML 440010147700
Content-Type: text/plain

ELECTRIC body 998877.
')
open('/home/leo/linux-runs/FormatWright/fixtures/sample.mbox','w').write('From alice@example.org Fri Sep  4 10:00:00 2026
From: Alice <alice@example.org>
Subject: First mail 440010147700

ELECTRIC body one 998877.

From carol@example.org Fri Sep  4 11:00:00 2026
From: Carol <carol@example.org>
Subject: Second mail MAIL2TOKEN

Body two 552233.
')
import matplotlib; matplotlib.use('Agg')
import matplotlib.pyplot as plt
from matplotlib.backends.backend_pdf import PdfPages
p=PdfPages('/home/leo/linux-runs/FormatWright/fixtures/sample.pdf'); fig,ax=plt.subplots(figsize=(6,4)); ax.text(0.3,0.5,'MATRIX PDF 440010147700',fontsize=18); ax.axis('off'); p.savefig(fig); p.close()
print('fixtures done')
PYEOF
cd "$SRC"
[ -x target/debug/formatwright ] || cargo build -p formatwright-cli --offline 2>&1 | tail -1

n=0; pass=0; fail=0
run() {
  local src="$1" tgt="$2"; shift 2
  n=$((n+1))
  local name="$(basename "$src" | sed 's/\..*//')_to_$(echo "$tgt" | tr . _)_$n"
  local res
  res=$(./target/debug/formatwright convert "$FX/$src" --to "$tgt" --output "$OUT/$name.$tgt" "$@" 2>&1)
  local code=$?
  local status
  status=$(printf '%s\n' "$res" | grep -oE "validation: (Pass|Warning|Fail)" | head -1 | cut -d' ' -f2)
  if [ $code -eq 0 ] && [ -n "$status" ] && [ "$status" != "Fail" ]; then
    pass=$((pass+1))
  else
    echo "FAIL  $src -> $tgt (exit $code) :: $(printf '%s' "$res" | grep -E '^error' | head -1)"
    fail=$((fail+1))
  fi
}
# structured
for s in csv json yaml; do for t in csv json yaml; do [ "$s" = "$t" ] && continue; run "sample.$s" "$t" --allow-lossy-data; done; done
# markup
for s in md html txt; do for t in pdf docx epub; do run "sample.$s" "$t"; done; done
# vector -> pdf
run sample.svg pdf
# pdf -> image
for t in jpg png; do run sample.pdf "$t"; done
# raster
for s in png jpg; do for t in webp avif tiff bmp; do run "sample.$s" "$t"; done; done
for s in tiff bmp; do for t in webp avif png; do run "sample.$s" "$t"; done; done
# OCR (engines available)
run sample.png txt
run sample.tiff txt
run sample.bmp txt
# email family (builtin adapters + chains; pdf needs the html->pdf lane)
for t in txt html; do run sample.eml "$t"; done
for t in txt html pdf; do run sample.mbox "$t"; done
if [ -f "$FX/sample.msg" ]; then
  for t in txt html pdf; do run sample.msg "$t"; done
fi
# PSD / camera-RAW through the discovered ImageMagick engine (opt-in:
# install ImageMagick user-level and export FORMATWRIGHT_ENGINE_MAGICK).
if command -v magick >/dev/null 2>&1 || [ -n "${FORMATWRIGHT_ENGINE_MAGICK:-}" ]; then
  "$PY" - <<'PYEOF2'
from PIL import Image
Image.new('RGB',(640,480),(30,90,200)).save('/home/leo/linux-runs/FormatWright/fixtures/sample.psd')
PYEOF2
  for t in png jpg tiff; do run sample.psd "$t"; done
  for rawext in dng cr2; do
    [ -f "$FX/sample.$rawext" ] || continue
    for t in png jpg tiff; do run "sample.$rawext" "$t"; done
    run "sample.$rawext" webp
  done
fi
# audio
FF="$HOME/miniforge/envs/ocr/bin/ffmpeg"
"$FF" -y -f lavfi -i sine=frequency=440:duration=3 -c:a pcm_s16le "$FX/sample.wav" -loglevel error
run sample.wav mp3
run sample.wav m4a
# archives
run sample.zip tar.gz
run sample.zip 7z
run sample.tar.gz 7z
run sample.tar.gz zip
echo "=== linux matrix: $pass pass / $fail fail / $n total ==="
