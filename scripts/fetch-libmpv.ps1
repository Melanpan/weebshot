# Downloads a pinned libmpv build (shinchiro/mpv-winbuild-cmake) into vendor/mpv
# and generates the MSVC import library mpv.lib from vendor/mpv/mpv.def.
$ErrorActionPreference = 'Stop'

$Release = '20261009'
$Asset = 'mpv-dev-x86_64-20261009-git-36abaa32d0.7z'
$Url = "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/$Release/$Asset"

$Root = Split-Path -Parent $PSScriptRoot
$Vendor = Join-Path $Root 'vendor\mpv'
$Tmp = Join-Path ([IO.Path]::GetTempPath()) "weebshot-libmpv-$Release"

New-Item -ItemType Directory -Force $Tmp, $Vendor | Out-Null
$Archive = Join-Path $Tmp $Asset
if (-not (Test-Path $Archive)) {
    Write-Host "Downloading $Url"
    Invoke-WebRequest $Url -OutFile $Archive
}
# Windows' bundled bsdtar can extract .7z
& "$env:SystemRoot\System32\tar.exe" -xf $Archive -C $Tmp
Copy-Item (Join-Path $Tmp 'libmpv-2.dll') $Vendor -Force

$VsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$Lib = & $VsWhere -latest -products * -find 'VC\Tools\MSVC\*\bin\Hostx64\x64\lib.exe' | Select-Object -First 1
if (-not $Lib) { throw 'lib.exe not found - install the MSVC build tools' }
& $Lib /nologo /def:"$Vendor\mpv.def" /machine:x64 /out:"$Vendor\mpv.lib"
Remove-Item "$Vendor\mpv.exp" -ErrorAction SilentlyContinue
Write-Host "libmpv ready in $Vendor"
