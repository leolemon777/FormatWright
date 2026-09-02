@echo off
set "INCLUDE=E:\VS Studio\SDK\ScopeCppSDK\vc15\VC\include;C:\Program Files (x86)\Windows Kits\10\Include\10.0.22621.0\ucrt;C:\Program Files (x86)\Windows Kits\10\Include\10.0.22621.0\um;C:\Program Files (x86)\Windows Kits\10\Include\10.0.22621.0\shared;C:\Program Files (x86)\Windows Kits\10\Include\10.0.22621.0\winrt;C:\Program Files (x86)\Windows Kits\10\Include\10.0.22621.0\cppwinrt"
set "LIB=E:\VS Studio\VC\Tools\MSVC\14.51.36231\lib\onecore\x64;C:\Program Files (x86)\Windows Kits\10\Lib\10.0.22621.0\ucrt\x64;C:\Program Files (x86)\Windows Kits\10\Lib\10.0.22621.0\um\x64"
set "PATH=E:\DevCaches\poppler-26.02.0\Library\bin;E:\DevCaches\Rust\cargo\bin;%PATH%"
cd /d "E:\Desktop\FormatWright"
cargo clippy -p formatwright-core --all-targets 2>&1 | findstr /C:"-->" | findstr /V "generated"
