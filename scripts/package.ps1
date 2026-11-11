# Builds a release and collects everything a Windows user needs into dist/weebshot:
# weebshot.exe, libmpv-2.dll, ffmpeg.exe, gifsicle.exe, the Anime4K helper and licenses.
# Then zips it to dist/weebshot-windows-x64.zip.
#
# The pinned FFmpeg shared SDK supplies helper libraries. FFMPEG / GIFSICLE may point at
# local command-line executables.
$ErrorActionPreference = 'Stop'

$FfmpegVersion = '9.0.2'
$FfmpegUrl = "https://github.com/GyanD/codexffmpeg/releases/download/$FfmpegVersion/ffmpeg-$FfmpegVersion-full_build-shared.zip"
$FfmpegSha256 = "8d31e162f1616e37aab3fa2db991b97e1b8dbeb1c8465fd81a81be16ff91b328"
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

    # FFmpeg shared build (GPLv3). The helper links to the included SDK.
    $ffmpegZip = Join-Path $Cache "ffmpeg-$FfmpegVersion-shared.zip"
    if (-not (Test-Path $ffmpegZip)) { Invoke-WebRequest $FfmpegUrl -OutFile $ffmpegZip }
    if ((Get-FileHash $ffmpegZip -Algorithm SHA256).Hash -ne $FfmpegSha256) {
        throw 'FFmpeg SDK checksum mismatch'
    }
    $sdkDir = Join-Path $Cache "ffmpeg-$FfmpegVersion-shared"
    if (Test-Path $sdkDir) { Remove-Item -Recurse -Force $sdkDir }
    Expand-Archive $ffmpegZip $sdkDir
    $sdk = Get-ChildItem $sdkDir -Directory | Select-Object -First 1
    Copy-Item "$($sdk.FullName)\bin\*.dll" $Dist -Force
    $ffmpeg = if ($env:FFMPEG) { $env:FFMPEG } else { "$($sdk.FullName)\bin\ffmpeg.exe" }
    Copy-Item $ffmpeg (Join-Path $Dist 'ffmpeg.exe') -Force
    Copy-Item "$($sdk.FullName)\LICENSE" (Join-Path $Dist 'FFmpeg-LICENSE.txt') -Force

    $helperBuild = Join-Path $Cache 'anime4k-build'
    cmake -S native/anime4k -B $helperBuild -A x64 "-DFFMPEG_SDK=$($sdk.FullName)" -DAC_FETCH_OPENCL=ON -DAC_CORE_WITH_OPENCL=ON
    if ($LASTEXITCODE -ne 0) { throw 'Anime4K helper configuration failed' }
    cmake --build $helperBuild --config Release --parallel
    if ($LASTEXITCODE -ne 0) { throw 'Anime4K helper build failed' }
    $helperStage = Join-Path $Cache 'anime4k'
    cmake --install $helperBuild --config Release --prefix $helperStage
    if ($LASTEXITCODE -ne 0) { throw 'Anime4K helper install failed' }
    Copy-Item "$helperStage\bin\*" $Dist -Force
    Copy-Item "$helperStage\licenses" $Dist -Recurse -Force
    & "$Dist\weebshot-anime4k.exe" probe cpu
    if ($LASTEXITCODE -ne 0) { throw 'Packaged Anime4K helper probe failed' }

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
