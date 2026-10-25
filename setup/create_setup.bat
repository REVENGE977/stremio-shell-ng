@echo off
set mypath=%~dp0

:: Find Inno Setup Compiler
set "ISCC_PATH="
if exist "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" set "ISCC_PATH=%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"
if not defined ISCC_PATH if exist "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" set "ISCC_PATH=C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
if not defined ISCC_PATH if exist "C:\Program Files\Inno Setup 6\ISCC.exe" set "ISCC_PATH=C:\Program Files\Inno Setup 6\ISCC.exe"
if not defined ISCC_PATH (
    where iscc >nul 2>&1
    if not errorlevel 1 set "ISCC_PATH=iscc"
)

if not defined ISCC_PATH (
    echo Error: Inno Setup 6 (ISCC.exe) was not found. Please install Inno Setup 6.
    exit /b 1
)

:: Compile the main executable
if not exist "%mypath%..\target\release\stremio-enhanced-ng.exe" (
    if not exist "%mypath%..\target\x86_64-pc-windows-msvc\release\stremio-enhanced-ng.exe" (
        cargo build --release
    ) else (
        echo Main executable is already built in target\x86_64-pc-windows-msvc\release\
    )
) else (
    echo Main executable is already built in target\release\
)

:: Compile the installer
"%ISCC_PATH%" "%mypath%Stremio.iss"
