param(
    [switch]$Sign
)

# If cl.exe targeting arm64 is not in PATH, auto-import the MSVC ARM64 developer environment
$arm64ClFound = $false
try {
    $clPaths = where.exe cl.exe 2>$null
    if ($clPaths -and ($clPaths -match '(?i)arm64')) {
        $arm64ClFound = $true
    }
} catch {}

if (-not $arm64ClFound) {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $vsPath = $null
    if (Test-Path $vswhere) {
        $vsPath = (& $vswhere -latest -property installationPath).Trim()
    }
    if (-not $vsPath -or -not (Test-Path $vsPath)) {
        $vsPath = "C:\Program Files\Microsoft Visual Studio\18\Community"
    }

    $vcvarsall = "$vsPath\VC\Auxiliary\Build\vcvarsall.bat"
    if (Test-Path $vcvarsall) {
        Write-Host "Initializing MSVC ARM64 build environment..." -ForegroundColor Cyan

        $vcvarsArgs = "x64_arm64"
        if (Test-Path "$vsPath\VC\Tools\MSVC\14.44.35207\bin\Hostx64\arm64\cl.exe") {
            $vcvarsArgs = "x64_arm64 -vcvars_ver=14.44.35207"
        }

        cmd /c "`"$vcvarsall`" $vcvarsArgs > nul && set" | ForEach-Object {
            if ($_ -match '^(.*?)=(.*)$') {
                [System.Environment]::SetEnvironmentVariable($matches[1], $matches[2], "Process")
            }
        }
    }
}

Write-Host "Compiling ARM64 release binary..." -ForegroundColor Cyan
cargo build --release --target aarch64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Locate Inno Setup Compiler
$iscc = "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
if (-not (Test-Path $iscc)) { $iscc = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" }
if (-not (Test-Path $iscc)) { $iscc = "C:\Program Files\Inno Setup 6\ISCC.exe" }
if (-not (Test-Path $iscc)) { $iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source }

if (-not $iscc -or -not (Test-Path $iscc)) {
    Write-Error "Inno Setup Compiler (ISCC.exe) not found!"
    exit 1
}

Write-Host "Packaging ARM64 setup installer with Inno Setup..." -ForegroundColor Cyan
if ($Sign) {
    & $iscc /DSIGN "/Sstremiosign=`$qsigntool`$q sign /fd SHA256 /t http://timestamp.digicert.com /n `$qSmart Code OOD`$q `$f" "setup\Stremio-arm64.iss"
} else {
    & $iscc "setup\Stremio-arm64.iss"
}

if ($LASTEXITCODE -eq 0) {
    Write-Host "ARM64 setup installer built successfully!" -ForegroundColor Green
}
