# Builds a release and collects everything a Windows user needs into dist/weebshot:
# weebshot.exe, libmpv-2.dll, ffmpeg.exe, gifsicle.exe, licenses.
# Then zips it to dist/weebshot-windows-x64.zip.
#
# ffmpeg and gifsicle are downloaded (pinned versions below) unless FFMPEG / GIFSICLE point at
# local executables.
$ErrorActionPreference = 'Stop'

$FfmpegVersion = '9.0.2'
$FfmpegUrl = "https://github.com/GyanD/codexffmpeg/releases/download/$FfmpegVersion/ffmpeg-$FfmpegVersion-essentials_build.zip"
$GifsicleVersion = '1.95'
$GifsicleUrl = "https://eternallybored.org/misc/gifsicle/releases/gifsicle-$GifsicleVersion-win64.zip"

$Root = Split-Path -Parent $PSScriptRoot
$Dist = Join-Path $Root 'dist\weebshot'
$Cache = Join-Path $Root 'dist\cache'
Push-Location $Root
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    if (Test-Path $Dist) { Remove-Item -Recurse -Force $Dist }
    New-Item -ItemType Directory -Force $Dist, $Cache | Out-Null
    Copy-Item target\release\weebshot.exe, vendor\mpv\libmpv-2.dll, LICENSE $Dist -Force

    # ffmpeg (GPLv3 build by gyan.dev)
    if ($env:FFMPEG) {
        Copy-Item $env:FFMPEG (Join-Path $Dist 'ffmpeg.exe') -Force
    } else {
        $zip = Join-Path $Cache "ffmpeg-$FfmpegVersion.zip"
        if (-not (Test-Path $zip)) { Invoke-WebRequest $FfmpegUrl -OutFile $zip }
        $out = Join-Path $Cache 'ffmpeg'
        if (Test-Path $out) { Remove-Item -Recurse -Force $out }
        & "$env:SystemRoot\System32\tar.exe" -xf $zip -C (New-Item -ItemType Directory -Force $out)
        $exe = Get-ChildItem $out -Recurse -Filter ffmpeg.exe | Select-Object -First 1
        Copy-Item $exe.FullName (Join-Path $Dist 'ffmpeg.exe') -Force
    }

    # gifsicle (GPLv2, build by eternallybored.org)
    if ($env:GIFSICLE) {
        Copy-Item $env:GIFSICLE (Join-Path $Dist 'gifsicle.exe') -Force
    } else {
        $zip = Join-Path $Cache "gifsicle-$GifsicleVersion.zip"
        if (-not (Test-Path $zip)) { Invoke-WebRequest $GifsicleUrl -OutFile $zip }
        $out = Join-Path $Cache 'gifsicle'
        if (Test-Path $out) { Remove-Item -Recurse -Force $out }
        & "$env:SystemRoot\System32\tar.exe" -xf $zip -C (New-Item -ItemType Directory -Force $out)
        $exe = Get-ChildItem $out -Recurse -Filter gifsicle.exe | Select-Object -First 1
        Copy-Item $exe.FullName (Join-Path $Dist 'gifsicle.exe') -Force
    }

    Copy-Item THIRD-PARTY.md $Dist -Force

    $Zip = Join-Path $Root 'dist\weebshot-windows-x64.zip'
    if (Test-Path $Zip) { Remove-Item $Zip }
    Compress-Archive -Path "$Dist\*" -DestinationPath $Zip
    Write-Host "Packaged $Zip"
} finally {
    Pop-Location
}
