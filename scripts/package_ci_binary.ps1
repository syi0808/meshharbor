param(
    [Parameter(Mandatory)]
    [string] $Target
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$packageName = "meshharbor-$Target"
$binaryName = $IsWindows ? 'meshharbor.exe' : 'meshharbor'
$extension = $IsWindows ? 'zip' : 'tar.gz'
$archiveName = "$packageName.$extension"
$output = New-Item -ItemType Directory -Path 'target/ci-packages' -Force
$archive = Join-Path $output.FullName $archiveName
$scratch = [System.IO.Directory]::CreateTempSubdirectory('meshharbor-ci-').FullName

try {
    $packageRoot = Join-Path $scratch $packageName
    New-Item -ItemType Directory -Path $packageRoot | Out-Null
    Copy-Item -LiteralPath "target/$Target/release/$binaryName" -Destination $packageRoot

    $notices = @(
        'LICENSE'
        'THIRD-PARTY-NOTICES.md'
        'apps/studio/THIRD-PARTY-LICENSES.txt'
        'apps/studio/THIRD-PARTY-NOTICES.md'
        'crates/core/src/trellis_text_mlx/THIRD-PARTY-LICENSES'
        'crates/xatlas-sys/THIRD_PARTY_NOTICES.md'
        'crates/xatlas-sys/vendor/xatlas/LICENSE'
        'runtimes/pixal3d/THIRD-PARTY-LICENSES'
        'runtimes/pixal3d/THIRD-PARTY-NOTICES.md'
        'runtimes/pixal3d/cuda/upstream/LICENSE'
        'runtimes/pixal3d/mlx/upstream/LICENSE'
    )
    foreach ($notice in $notices) {
        $destination = Join-Path $packageRoot $notice
        New-Item -ItemType Directory -Path (Split-Path $destination) -Force | Out-Null
        Copy-Item -LiteralPath $notice -Destination $destination -Recurse
    }

    $extracted = Join-Path $scratch 'extracted'
    New-Item -ItemType Directory -Path $extracted | Out-Null
    if ($IsWindows) {
        Compress-Archive -LiteralPath $packageRoot -DestinationPath $archive -Force
        Expand-Archive -LiteralPath $archive -DestinationPath $extracted
    } else {
        tar -czf $archive -C $scratch $packageName
        tar -xzf $archive -C $extracted
    }
    node scripts/smoke_embedded_studio.mjs (Join-Path $extracted "$packageName/$binaryName")

    $checksum = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    "$checksum  $archiveName" | Set-Content -LiteralPath "$archive.sha256" -Encoding ascii
} finally {
    Remove-Item -LiteralPath $scratch -Recurse -Force
}
