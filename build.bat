@echo off
REM Build script for MP3CutAd-RS

echo Building MP3CutAd-RS...
echo.

echo [1/4] Building core library...
cargo build --release --lib

echo.
echo [2/4] Building CLI...
cargo build --release --features cli --bin mp3cutad-cli

echo.
echo [3/4] Building GUI...
cargo build --release --features gui --bin mp3cutad-gui

echo.
echo [4/4] Copying files...
if not exist "release" mkdir release
copy /Y "target\release\mp3cutad-gui.exe" "release\MP3CutAd-RS.exe"
copy /Y "target\release\mp3cutad-cli.exe" "release\mp3cutad-cli.exe"
copy /Y "target\release\mp3cutad_rs.dll" "release\mp3cutad_rs.dll"

echo.
echo Build complete!
echo.
echo Output files:
echo   GUI: release\MP3CutAd-RS.exe
echo   CLI: release\mp3cutad-cli.exe
echo   DLL: release\mp3cutad_rs.dll
echo.
pause
