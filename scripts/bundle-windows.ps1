# Empacota o visor nativo para Windows (NSIS CurrentUser).
# Uso (PowerShell): powershell -File scripts/bundle-windows.ps1
# Saída: dist/TsuroPDF-{versão}-x86_64-pc-windows-msvc-setup.exe
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

$Cargo = Get-Content "$Root\crates\tsuro\Cargo.toml" -Raw
if ($Cargo -notmatch 'version\s*=\s*"([^"]+)"') { throw "versão não encontrada em Cargo.toml" }
$Version = $Matches[1]
$PdfiumRelease = "chromium/8044"
$Asset = "pdfium-win-x64.tgz"
$WinDir = Join-Path $Root "dist\windows"
$Out = Join-Path $Root "dist\TsuroPDF-$Version-x86_64-pc-windows-msvc-setup.exe"
# Nomes legados do rename: não confundir com o instalador TsuroPDF.
Remove-Item "$Root\dist\Tsuro-*-setup.exe" -Force -ErrorAction SilentlyContinue

New-Item -ItemType Directory -Force -Path $WinDir | Out-Null
# Staging limpa: build anterior deixava tsuro.exe legado ao lado do binário novo.
Remove-Item (Join-Path $WinDir "*") -Recurse -Force -ErrorAction SilentlyContinue

$PdfiumDll = Join-Path $Root "pdfium.dll"
if (-not (Test-Path $PdfiumDll)) {
  $Tmp = Join-Path $env:TEMP ("tsuro-pdfium-" + [guid]::NewGuid().ToString())
  New-Item -ItemType Directory -Path $Tmp | Out-Null
  $Tgz = Join-Path $Tmp "pdfium.tgz"
  $Url = "https://github.com/bblanchon/pdfium-binaries/releases/download/$PdfiumRelease/$Asset"
  Write-Host "Baixando Pdfium ($Asset)..."
  Invoke-WebRequest -Uri $Url -OutFile $Tgz
  tar -xzf $Tgz -C $Tmp
  $Found = Get-ChildItem -Path $Tmp -Recurse -Filter "pdfium.dll" | Select-Object -First 1
  if (-not $Found) { throw "pdfium.dll não veio no tarball" }
  Copy-Item $Found.FullName $PdfiumDll
  Remove-Item -Recurse -Force $Tmp
}

cargo build --release -p tsuro --manifest-path "$Root\Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw "cargo build falhou" }

Copy-Item "$Root\target\release\TsuroPDF.exe" "$WinDir\TsuroPDF.exe" -Force
Copy-Item $PdfiumDll "$WinDir\pdfium.dll" -Force
Copy-Item "$Root\LICENSE" "$WinDir\LICENSE" -Force

$Makensis = $null
foreach ($candidate in @(
  "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
  "$env:ProgramFiles\NSIS\makensis.exe",
  "makensis"
)) {
  if ($candidate -eq "makensis") {
    $cmd = Get-Command makensis -ErrorAction SilentlyContinue
    if ($cmd) { $Makensis = $cmd.Source; break }
  } elseif (Test-Path $candidate) {
    $Makensis = $candidate
    break
  }
}
if (-not $Makensis) { throw "NSIS (makensis) não encontrado. choco install nsis" }

$Nsi = Join-Path $Root "scripts\windows\tsuro.nsi"
& $Makensis "/DVERSION=$Version" "/DSRC=$WinDir" "/DOUT=$Out" $Nsi
if ($LASTEXITCODE -ne 0) { throw "makensis falhou" }
Write-Host "Pronto: $Out"
