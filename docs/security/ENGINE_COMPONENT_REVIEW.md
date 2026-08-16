# Engine Component Review Workbook (Batch C)

- Generated: 2026-08-16T06:42:23+00:00 by `scripts/generate_engine_component_inventory.py`
- Status: **pending human review** - regeneratable, but the Decision/Signature columns are hand-maintained
- Rule: a component is only `reviewed` with a named signature and date (`ENGINE_SUPPLY_CHAIN.md` §6);
  `sources.json.review_status` may then be promoted by the same signature.

## formatwright-media 9.0 (pack `media`)

- Build license verdict: **GPL-3.0-or-later**
- FFmpeg configuration: `--enable-gpl --enable-version3 --enable-static --disable-w32threads --disable-autodetect --enable-cairo --enable-fontconfig --enable-iconv --enable-gnutls --enable-libxml2 --enable-gmp --enable-bzlib --enable-lzma --enable-zlib --enable-libsrt --enable-libssh --enable-libzmq --enable-avisynth --enable-sdl2 --enable-libwebp --enable-libx264 --enable-libx265 --enable-libxvid --enable-libaom --enable-libopenjpeg --enable-libvpx --enable-mediafoundation --enable-libass --enable-libfreetype --enable-libfribidi --enable-libharfbuzz --enable-libvidstab --enable-libvmaf --enable-libzimg --enable-amf --enable-cuda-llvm --enable-cuvid --enable-dxva2 --enable-d3d11va --enable-d3d12va --enable-ffnvcodec --enable-libvpl --enable-nvdec --enable-nvenc --enable-vaapi --enable-openal --enable-libgme --enable-libopenmpt --enable-libopencore-amrwb --enable-libmp3lame --enable-libtheora --enable-libvo-amrwbenc --enable-libgsm --enable-libopencore-amrnb --enable-libopus --enable-libspeex --enable-libvorbis --enable-librubberband`
- FFmpeg libraries: libavutil 61., libavcodec 63., libavformat 63., libavdevice 63., libavfilter 12., libswscale 10., libswresample 7.

| Component | Upstream | License (engineering KB) | Patent note | Review status | Reviewer / date |
|---|---|---|---|---|---|
| ffmpeg | https://ffmpeg.org | LGPL-2.1-or-later (GPL with --enable-gpl; GPL-3.0-or-later with --enable-version3) | Codec patents must be reviewed per region | pending | — |
| libavcodec | https://ffmpeg.org | LGPL-2.1-or-later / GPL per build | Contains codec implementations; patent review required | pending | — |
| libavdevice | https://ffmpeg.org | LGPL-2.1-or-later / GPL per build | — | pending | — |
| libavfilter | https://ffmpeg.org | LGPL-2.1-or-later / GPL per build | — | pending | — |
| libavformat | https://ffmpeg.org | LGPL-2.1-or-later / GPL per build | — | pending | — |
| libavutil | https://ffmpeg.org | LGPL-2.1-or-later / GPL per build | — | pending | — |
| libswresample | https://ffmpeg.org | LGPL-2.1-or-later / GPL per build | — | pending | — |
| libswscale | https://ffmpeg.org | LGPL-2.1-or-later / GPL per build | — | pending | — |
| bzlib | https://sourceware.org/bzip2/ | bzip2 license (BSD-style) | — | pending | — |
| cairo | https://cairographics.org | LGPL-2.1-or-later OR MPL-1.1 | — | pending | — |
| fontconfig | https://www.freedesktop.org/wiki/Software/fontconfig | MIT-style permissive | — | pending | — |
| gmp | https://gmplib.org | LGPL-3.0-or-later OR GPL-2.0-or-later | — | pending | — |
| gnutls | https://gnutls.org | LGPL-2.1-or-later | — | pending | — |
| iconv | https://www.gnu.org/software/libiconv/ | LGPL-2.1-or-later | — | pending | — |
| libaom | https://aomedia.org | BSD-2-Clause + patent license | AV1 patent pool notice required | pending | — |
| libass | https://github.com/libass/libass | ISC | — | pending | — |
| libfreetype | https://freetype.org | FreeType License OR GPL-2.0-or-later | — | pending | — |
| libfribidi | https://github.com/fribidi/fribidi | LGPL-2.1-or-later | — | pending | — |
| libgme | https://bitbucket.org/mpyne/game-music-emu | LGPL-2.1-or-later | — | pending | — |
| libgsm | https://www.quut.com/gsm | ISC-like (specific license text) | GSM 06.10 patent status must be reviewed | pending | — |
| libharfbuzz | https://harfbuzz.github.io | Old MIT | — | pending | — |
| libmp3lame | https://lame.sourceforge.io | LGPL-2.0-or-later | MP3 patents expired (2021 worldwide); confirm | pending | — |
| libopencore-amrnb | https://sourceforge.net/projects/opencore-amr | Apache-2.0 | AMR patent status must be reviewed | pending | — |
| libopencore-amrwb | https://sourceforge.net/projects/opencore-amr | Apache-2.0 | AMR patent status must be reviewed | pending | — |
| libopenjpeg | https://github.com/uclouvain/openjpeg | BSD-2-Clause | JPEG 2000 patents largely expired; verify | pending | — |
| libopenmpt | https://lib.openmpt.org | BSD-3-Clause | — | pending | — |
| libopus | https://www.opus-codec.org | BSD-3-Clause | Opus declared patent-royalty-free; verify Xiph/Patent License | pending | — |
| librubberband | https://breakfastquay.com/rubberband/ | GPL-2.0-or-later (commercial option) | — | pending | — |
| libspeex | https://www.speex.org | BSD-3-Clause | — | pending | — |
| libsrt | https://github.com/Haivision/srt | MPL-2.0 | — | pending | — |
| libssh | https://www.libssh.org | LGPL-2.1-or-later | — | pending | — |
| libtheora | https://www.theora.org | BSD-3-Clause + Xiph patent grant | — | pending | — |
| libvidstab | https://github.com/georgmartius/vid.stab | GPL-2.0-or-later | — | pending | — |
| libvmaf | https://github.com/Netflix/vmaf | BSD-2-Clause + Patent | VMAF patent notice required | pending | — |
| libvo-amrwbenc | https://sourceforge.net/projects/opencore-amr | Apache-2.0 | AMR encoding patents must be reviewed | pending | — |
| libvorbis | https://xiph.org/vorbis | BSD-3-Clause | — | pending | — |
| libvpx | https://www.webmproject.org | BSD-3-Clause | VP8/VP9 patents granted royalty-free by Google; verify terms | pending | — |
| libwebp | https://developers.google.com/speed/webp | BSD-3-Clause | — | pending | — |
| libx264 | https://www.videolan.org/developers/x264.html | GPL-2.0-or-later | Commercial encoder license option exists | pending | — |
| libx265 | https://bitbucket.org/multicoreware/x265 | GPL-2.0-or-later | Commercial encoder license option exists | pending | — |
| libxml2 | https://gitlab.gnome.org/GNOME/libxml2 | MIT | — | pending | — |
| libxvid | https://www.xvid.com | GPL-2.0-or-later | — | pending | — |
| libzimg | https://github.com/sekrit-twc/zimg | MIT | — | pending | — |
| libzmq | https://zeromq.org | LGPL-3.0-or-later | — | pending | — |
| lzma | https://tukaani.org/xz/ | 0BSD (XZ embedded / lzma) | — | pending | — |
| zlib | https://zlib.net | Zlib | — | pending | — |

Plus 2 non-binary data/license files (poppler share data, notices) - covered by the `poppler-data` row above.

## formatwright-pdf 26.02.0-0 (pack `pdf`)

- Build license verdict: **GPL-2.0-or-later (poppler core)**

| Component | Upstream | License (engineering KB) | Patent note | Review status | Reviewer / date |
|---|---|---|---|---|---|
| poppler | https://poppler.freedesktop.org | GPL-2.0-or-later | — | pending | — |
| poppler-data | https://poppler.freedesktop.org | Adobe CMap/unicode data redistribution terms + BSD-style poppler-data license | Free redistribution permitted with notices; verify Adobe terms | pending | — |
| cairo | https://cairographics.org | LGPL-2.1-or-later OR MPL-1.1 | — | pending | — |
| charset | https://www.gnu.org/software/libiconv/ | LGPL-2.1-or-later (libiconv) | — | pending | — |
| deflate | https://github.com/richgel999/miniz | MIT | — | pending | — |
| expat | https://libexpat.github.io | MIT | — | pending | — |
| libfontconfig | https://www.freedesktop.org/wiki/Software/fontconfig | MIT-style permissive | — | pending | — |
| libfreetype | https://freetype.org | FreeType License OR GPL-2.0-or-later | — | pending | — |
| iconv | https://www.gnu.org/software/libiconv/ | LGPL-2.1-or-later | — | pending | — |
| jpeg | https://libjpeg-turbo.org | IJG license OR BSD-3-Clause (turbo) | — | pending | — |
| lcms2 | https://www.littlecms.com | MIT | — | pending | — |
| lerc | https://github.com/Esri/lerc | Apache-2.0 | — | pending | — |
| libcrypto | https://openssl.org | Apache-2.0 | — | pending | — |
| libcurl | https://curl.se | curl license (MIT-like) | — | pending | — |
| liblzma | https://tukaani.org/xz/ | 0BSD | — | pending | — |
| libpng | https://libpng.org | libpng-2.0 (PNG Reference License) | — | pending | — |
| libssh2 | https://libssh2.org | BSD-3-Clause | — | pending | — |
| libtiff | https://libtiff.gitlab.io/libtiff/ | libtiff (MIT-like) | — | pending | — |
| zstd | https://facebook.github.io/zstd/ | BSD-3-Clause OR GPL-2.0-or-later | — | pending | — |
| libopenjpeg | https://github.com/uclouvain/openjpeg | BSD-2-Clause | JPEG 2000 patents largely expired; verify | pending | — |
| pixman | https://cairographics.org | MIT | — | pending | — |
| poppler-cpp | https://poppler.freedesktop.org | GPL-2.0-or-later | — | pending | — |
| poppler-glib | https://poppler.freedesktop.org | GPL-2.0-or-later | — | pending | — |
| zlib | https://zlib.net | Zlib | — | pending | — |

Plus 269 non-binary data/license files (poppler share data, notices) - covered by the `poppler-data` row above.
