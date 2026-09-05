#!/usr/bin/env bash
# FormatWright conversion-matrix smoke run: every supported route, one shot.
set -u
FW="E:\\Desktop\\FormatWright\\target\\debug\\formatwright.exe"
FX="/e/Desktop/FormatWright/target/matrix/fixtures"
OUT="/e/Desktop/FormatWright/target/matrix/out"
# A previous run's numbered outputs collide with this run's counters.
rm -rf "$OUT"
mkdir -p "$OUT"
export FORMATWRIGHT_ENGINE_PDFINFO="E:\\DevCaches\\poppler-26.02.0\\Library\\bin\\pdfinfo.exe"
export FORMATWRIGHT_ENGINE_PDFTOPPM="E:\\DevCaches\\poppler-26.02.0\\Library\\bin\\pdftoppm.exe"
export FORMATWRIGHT_ENGINE_PDFTOTEXT="E:\\DevCaches\\poppler-26.02.0\\Library\\bin\\pdftotext.exe"
export FORMATWRIGHT_ENGINE_PDFFONTS="E:\\DevCaches\\poppler-26.02.0\\Library\\bin\\pdffonts.exe"
export FORMATWRIGHT_ENGINE_QPDF="E:\\DevCaches\\qpdf-12.4.1\\bin\\qpdf.exe"
export FORMATWRIGHT_ENGINE_SOFFICE="E:\\DevCaches\\LibreOffice\\program\\soffice.com"
FFDIR="/c/Users/leo lemon/AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.1.1-full_build/bin"
export FORMATWRIGHT_ENGINE_FFMPEG="$FFDIR/ffmpeg.exe"
export FORMATWRIGHT_ENGINE_FFPROBE="$FFDIR/ffprobe.exe"
# C1 long-tail fixtures: opaque TIFF/BMP generated on demand.
[ -f "$FX/sample.tiff" ] || "$FFDIR/ffmpeg.exe" -y -f lavfi -i "testsrc2=size=320x240" -frames:v 1 -pix_fmt bgr24 -c:v tiff "$FX/sample.tiff" -loglevel error
[ -f "$FX/sample.bmp" ] || "$FFDIR/ffmpeg.exe" -y -f lavfi -i "testsrc2=size=320x240" -frames:v 1 -pix_fmt bgr24 -c:v bmp "$FX/sample.bmp" -loglevel error
# C1 wave 2: PSD/RAW via the discovered ImageMagick engine.
MAGICK="E:\\DevCaches\\ImageMagick\\magick.exe"
export FORMATWRIGHT_ENGINE_MAGICK="$MAGICK"
[ -f "$FX/sample.psd" ] || "$MAGICK" -size 320x240 gradient:blue-red "$FX/sample.psd"
# Real camera-RAW fixtures are large downloads; run those rows only when present.
for rawext in dng cr2; do
  if [ ! -f "$FX/sample.$rawext" ] && [ -f "/e/Desktop/FormatWright/target/c1-raw/sample.$rawext" ]; then
    cp "/e/Desktop/FormatWright/target/c1-raw/sample.$rawext" "$FX/sample.$rawext"
  fi
done
export FORMATWRIGHT_ENGINE_PANDOC="D:\\Anaconda3\\Library\\bin\\pandoc.exe"

n=0; pass=0; fail=0
run() { # run <source> <target> [extra args...]
  local src="$1" tgt="$2"; shift 2
  n=$((n+1))
  local name="${src%%.*}_to_${tgt//./_}_$n"
  local out="$OUT/$name.out"
  local res
  res=$("$FW" convert "$FX/$src" --to "$tgt" --output "$OUT/$name.$tgt" "$@" 2>&1)
  local code=$?
  local status
  status=$(printf '%s\n' "$res" | grep -oE "validation: (Pass|Warning|Fail)" | head -1 | cut -d' ' -f2)
  if [ $code -eq 0 ] && [ -n "$status" ] && [ "$status" != "Fail" ]; then
    echo "PASS  $src -> $tgt [$status]"; pass=$((pass+1))
  else
    echo "FAIL  $src -> $tgt (exit $code) :: $(printf '%s' "$res" | grep -E '^error|validation' | head -2 | tr '\n' ' ')"
    printf '%s\n' "$res" > "$out"
    fail=$((fail+1))
  fi
}

cd "$FX"
# structured 4x4
for s in csv json yaml xml; do for t in csv json yaml xml; do
  [ "$s" = "$t" ] && continue
  run "sample.$s" "$t" --allow-lossy-data
done; done
# markup lanes
for s in md html txt; do for t in pdf docx epub; do run "sample.$s" "$t"; done; done
# office/presentation/vector -> pdf
for s in svg odt ods odp docx pptx xlsx rtf; do run "sample.$s" pdf; done
# pdf -> image
for t in jpg png; do run sample.pdf "$t"; done
# raster images
for s in png jpg; do for t in webp avif tiff bmp pdf; do run "sample.$s" "$t"; done; done
for s in tiff bmp; do for t in webp avif png pdf; do run "sample.$s" "$t"; done; done
# PSD / camera-RAW through the discovered ImageMagick engine
for t in png jpg tiff; do run sample.psd "$t"; done
for rawext in dng cr2; do
  [ -f "$FX/sample.$rawext" ] || continue
  for t in png jpg tiff; do run "sample.$rawext" "$t"; done
  # one chain row: raw -> tiff -> webp proves the whitelisted TIFF pivot
  run "sample.$rawext" webp
done
# video containers
for s in mp4 webm; do for t in mp4 gif mp3; do run "sample.$s" "$t"; done; done
# audio
for s in wav flac mp3 m4a ogg opus; do for t in m4a mp3 wav; do
  [ "$s" = "$t" ] && continue
  run "sample.$s" "$t"
done; done
# email family (builtin adapters; regresses the immediate-path report persistence)
printf 'From: a@example.org
To: b@example.org
Subject: Matrix EML 440010147700
Content-Type: text/plain

ELECTRIC body 998877.
' > "$FX/sample.eml"
for t in txt html; do run sample.eml "$t"; done
if [ -f "$FX/sample.msg" ] || { cp "/e/Desktop/FormatWright/target/c2-msg/real.msg" "$FX/sample.msg" 2>/dev/null; [ -f "$FX/sample.msg" ]; }; then
  for t in txt html pdf; do run sample.msg "$t"; done
fi
# C3 MBOX aggregation (builtin split; pdf needs the html->pdf lane + qpdf)
cat > "$FX/sample.mbox" <<'MBOXEOF'
From alice@example.org Fri Sep  4 10:00:00 2026
From: Alice <alice@example.org>
To: bob@example.org
Subject: First mail 440010147700

ELECTRIC body one 998877.
>From the escaped line stays.

From carol@example.org Fri Sep  4 11:00:00 2026
From: Carol <carol@example.org>
Subject: Second mail MAIL2TOKEN

Body two 552233.

From dave@example.org Fri Sep  4 12:00:00 2026
From: Dave <dave@example.org>
Subject: Third mail MAIL3TOKEN
Content-Type: text/html

<html><body><p>MAIL3TOKEN html body</p><script>alert(1)</script></body></html>
MBOXEOF
for t in txt html pdf; do run sample.mbox "$t"; done
# archives
run sample.zip tar.gz
echo "=== matrix summary: $pass pass / $fail fail / $n total ==="
