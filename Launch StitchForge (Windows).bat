@echo off
rem Double-click to build (first time) and open StitchForge on Windows.
rem Works from any folder, including names with spaces or symbols.
cd /d "%~dp0"
title StitchForge launcher
echo StitchForge launcher - folder: %CD%
echo.
if exist "%USERPROFILE%\.cargo\bin\cargo.exe" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >/dev/null 2>nul
if not errorlevel 1 goto tools
echo Installing Rust (one time, about 5 minutes)...
curl.exe -sSfL https://win.rustup.rs/x86_64 -o "%TEMP%\rustup-init.exe"
if errorlevel 1 goto nodownload
"%TEMP%\rustup-init.exe" -y --default-host x86_64-pc-windows-gnu --profile minimal
if errorlevel 1 goto failed
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

:tools
rem Rust on Windows needs the MinGW tools (dlltool) for some system libraries.
where dlltool >/dev/null 2>nul
if not errorlevel 1 goto build
call :finddlltool
if defined DLLTOOLDIR goto addtools
echo Installing the MinGW build tools (one time, about 5 minutes)...
winget install -e --id BrechtSanders.WinLibs.POSIX.MSVCRT --accept-source-agreements --accept-package-agreements --silent
call :finddlltool
if defined DLLTOOLDIR goto addtools
goto notools
:addtools
echo Using MinGW tools in %DLLTOOLDIR%
set "PATH=%DLLTOOLDIR%;%PATH%"

:build
echo Building StitchForge... the first build takes several minutes, later ones seconds.
cargo build --release
if errorlevel 1 goto failed
copy /y "target\release\stitchforge-app.exe" "StitchForge.exe" >nul
echo.
echo Done. Next time you can just double-click StitchForge.exe in this folder.
start "" "StitchForge.exe"
exit /b 0

:finddlltool
set "DLLTOOLDIR="
if exist "C:\mingw64\bin\dlltool.exe" set "DLLTOOLDIR=C:\mingw64\bin"
if defined DLLTOOLDIR exit /b 0
if not exist "%LOCALAPPDATA%\Microsoft\WinGet\Packages" exit /b 0
for /f "delims=" %%F in ('dir /s /b "%LOCALAPPDATA%\Microsoft\WinGet\Packages\dlltool.exe" 2^>nul') do set "DLLTOOLDIR=%%~dpF"
exit /b 0

:notools
echo.
echo Could not install the MinGW tools automatically (winget may be blocked on this PC).
echo Do this once by hand:
echo   1. Go to https://winlibs.com and download the latest "GCC ... MSVCRT runtime, Win64, Zip archive".
echo   2. Unzip it so you have the folder C:\mingw64  (it contains bin\dlltool.exe).
echo   3. Double-click this launcher again.
pause
exit /b 1

:nodownload
echo Could not download the Rust installer. Check the internet connection, or install
echo Rust by hand from https://rustup.rs and run this file again.
pause
exit /b 1

:failed
echo.
echo Something went wrong. Take a photo of the text above and send it to Claude.
pause
exit /b 1
