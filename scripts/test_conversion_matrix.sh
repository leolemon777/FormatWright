#!/usr/bin/env bash
# FormatWright conversion-matrix smoke run: every supported route, one shot.
set -u
FW="E:\\Desktop\\FormatWright\\target\\debug\\formatwright.exe"
FX="/e/Desktop/FormatWright/target/matrix/fixtures"
OUT="/e/Desktop/FormatWright/target/matrix/out"
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
for s in png jpg; do for t in webp avif pdf; do run "sample.$s" "$t"; done; done
# video containers
for s in mp4 webm; do for t in mp4 gif mp3; do run "sample.$s" "$t"; done; done
# audio
for s in wav flac mp3 m4a ogg opus; do for t in m4a mp3 wav; do
  [ "$s" = "$t" ] && continue
  run "sample.$s" "$t"
done; done
# archives
run sample.zip tar.gz
echo "=== matrix summary: $pass pass / $fail fail / $n total ==="
