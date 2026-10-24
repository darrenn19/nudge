<#
.SYNOPSIS
  Build Nudge for Windows as an MSIX package.

.DESCRIPTION
  Builds the release app, lays it out with AppxManifest.xml and its logos,
  then either:
    -Install  registers the layout on this PC (needs Developer Mode; no
              certificate). For trying the package while developing.
    default   packs build\msix\Nudge.msix, signed with -Certificate (a .pfx)
              if given. To install a self-signed package, its certificate
              must be trusted first (Local Machine > Trusted People).

  Run from app\: powershell -File windows\packaging\package.ps1 [-Install]

.PARAMETER Publisher
  Must match the signing certificate's subject. Defaults to CN=Nudge Dev.
#>
param(
  [switch]$Install,
  [string]$Certificate,
  # Read-Host -AsSecureString, or ConvertTo-SecureString in a build script.
  [SecureString]$CertificatePassword,
  [string]$Publisher = "CN=Nudge Dev",
  [switch]$SkipBuild
)
$ErrorActionPreference = "Stop"
$app = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $app

$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" |
  Sort-Object FullName -Descending | Select-Object -First 1
if (-not $sdk) { throw "The Windows SDK (makeappx.exe) isn't installed." }
$tools = Split-Path $sdk.FullName

if (-not $SkipBuild) {
  flutter build windows --release
  if ($LASTEXITCODE -ne 0) { throw "flutter build failed" }
}

# 1.2.3+4 in pubspec.yaml -> 1.2.3.0 (MSIX wants four numbers; the last is
# reserved for the Store).
$version = (Select-String -Path pubspec.yaml -Pattern '^version:\s*(\d+)\.(\d+)\.(\d+)').Matches[0].Groups
$version = "$($version[1]).$($version[2]).$($version[3]).0"

$out = Join-Path $app "build\msix"
# A developer install runs from its folder, so packing uses another.
$layout = Join-Path $out $(if ($Install) { "installed" } else { "layout" })
if ($Install) {
  # The installed copy runs from its folder, which is about to be rebuilt.
  Get-Process nudge -ErrorAction SilentlyContinue | Stop-Process -Force
  Get-AppxPackage io.github.darrenn19.Nudge | Remove-AppxPackage
}
if (Test-Path $layout) { Remove-Item $layout -Recurse -Force }
New-Item -ItemType Directory -Force $layout | Out-Null
Copy-Item "build\windows\x64\runner\Release\*" $layout -Recurse

# Logos, drawn from the app icon.
Add-Type -AssemblyName System.Drawing
$assets = New-Item -ItemType Directory -Force (Join-Path $layout "Assets")
$icon = New-Object System.Drawing.Icon (Join-Path $app "windows\runner\resources\app_icon.ico"), 256, 256
$source = $icon.ToBitmap()
foreach ($logo in @(@("Square44x44Logo", 44), @("Square150x150Logo", 150), @("StoreLogo", 50))) {
  $size = $logo[1]
  $bitmap = New-Object System.Drawing.Bitmap $size, $size
  $g = [System.Drawing.Graphics]::FromImage($bitmap)
  $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $g.DrawImage($source, 0, 0, $size, $size)
  $g.Dispose()
  $bitmap.Save((Join-Path $assets "$($logo[0]).png"), [System.Drawing.Imaging.ImageFormat]::Png)
  $bitmap.Dispose()
}

(Get-Content (Join-Path $PSScriptRoot "AppxManifest.xml") -Raw).
  Replace('$VERSION$', $version).Replace('$PUBLISHER$', $Publisher) |
  Set-Content (Join-Path $layout "AppxManifest.xml") -Encoding UTF8

# What an unpackaged Nudge registered (registry menu entry, login entry,
# "Send to" shortcut) would point at an old exe and start a second copy at
# login; the package does all three itself.
function Remove-UnpackagedLeftovers {
  Remove-Item -LiteralPath "HKCU:\Software\Classes\*\shell\Nudge" -Recurse -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath "HKCU:\Software\Classes\Directory\shell\Nudge" -Recurse -ErrorAction SilentlyContinue
  Remove-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name Nudge -ErrorAction SilentlyContinue
  Remove-Item "$env:APPDATA\Microsoft\Windows\SendTo\Nudge.lnk" -ErrorAction SilentlyContinue
}

if ($Install) {
  Remove-UnpackagedLeftovers
  Add-AppxPackage -Register (Join-Path $layout "AppxManifest.xml")
  Write-Host "Installed from $layout (Developer Mode). Start Nudge from the Start menu."
  return
}

$msix = Join-Path $out "Nudge.msix"
& (Join-Path $tools "makeappx.exe") pack /d $layout /p $msix /o
if ($LASTEXITCODE -ne 0) { throw "makeappx failed" }
if ($Certificate) {
  $signArgs = @("sign", "/fd", "SHA256", "/f", $Certificate)
  if ($CertificatePassword) {
    # signtool takes it as text: unwrapped only here.
    $plain = [System.Net.NetworkCredential]::new("", $CertificatePassword).Password
    $signArgs += @("/p", $plain)
  }
  & (Join-Path $tools "signtool.exe") @signArgs $msix
  if ($LASTEXITCODE -ne 0) { throw "signtool failed" }
}
Write-Host "Packed $msix$(if (-not $Certificate) { ' (unsigned: sign it before installing)' })"
