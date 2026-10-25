param(
    [switch]$Sign
)

cargo build --release --target x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$iscc = "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
if (-not (Test-Path $iscc)) { $iscc = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" }
if (-not (Test-Path $iscc)) { $iscc = "C:\Program Files\Inno Setup 6\ISCC.exe" }
if (-not (Test-Path $iscc)) { $iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source }

if (-not $iscc -or -not (Test-Path $iscc)) {
    Write-Error "Inno Setup Compiler (ISCC.exe) not found!"
    exit 1
}

if ($Sign) {
    & $iscc /DSIGN "/Sstremiosign=`$qsigntool`$q sign /fd SHA256 /t http://timestamp.digicert.com /n `$qSmart Code OOD`$q `$f" "setup\Stremio.iss"
} else {
    & $iscc "setup\Stremio.iss"
}
