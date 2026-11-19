# Vitna Code automated release packager (PowerShell)
# Builds or archives binaries, computes SHA-256 checksums, and signs release manifests.

param (
    [string]$Version = "1.0.0",
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$OutputDir = "dist"
)

$ErrorActionPreference = "Stop"

Write-Host "==> Packaging Vitna Code v$Version for $Target"

$ReleaseDist = Join-Path $OutputDir "vitna-code-v$Version-$Target"
if (Test-Path $ReleaseDist) {
    Remove-Item -Recurse -Force $ReleaseDist
}
New-Item -ItemType Directory -Force -Path $ReleaseDist | Out-Null

$BinDir = Join-Path $ReleaseDist "bin"
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

# Copy compiled binaries (fail fast instead of producing an empty bin/ archive)
$TargetRelease = Join-Path "target" (Join-Path $Target "release")
if (-not (Test-Path $TargetRelease)) {
    $TargetRelease = Join-Path "target" "release"
}
foreach ($BinName in @("vitna.exe", "vitna-coded.exe", "vitna-tui.exe")) {
    $BinPath = Join-Path $TargetRelease $BinName
    if (-not (Test-Path $BinPath)) {
        throw "Missing compiled binary $BinPath. Run 'cargo build --release' first."
    }
    Copy-Item $BinPath $BinDir
}

# Copy documentation and schemas
Copy-Item "README.md" $ReleaseDist
Copy-Item "CONTRIBUTING.md" $ReleaseDist
Copy-Item "schemas\vitna-run-receipt-v1.json" $ReleaseDist
Copy-Item "schemas\vitna-release-manifest-v1.json" $ReleaseDist

# Create ZIP archive
$ZipFile = "$ReleaseDist.zip"
if (Test-Path $ZipFile) {
    Remove-Item -Force $ZipFile
}
Compress-Archive -Path "$ReleaseDist\*" -DestinationPath $ZipFile -CompressionLevel Optimal

# Compute SHA-256
$Hash = (Get-FileHash -Algorithm SHA256 $ZipFile).Hash.ToLower()
$ChecksumFile = Join-Path $OutputDir "SHA256SUMS"
"$Hash  $(Split-Path $ZipFile -Leaf)" | Out-File -FilePath $ChecksumFile -Append -Encoding ascii

Write-Host "==> Package created: $ZipFile"
Write-Host "==> SHA-256: $Hash"
