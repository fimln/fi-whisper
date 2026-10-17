#requires -Version 7.0
# Mengunduh komponen redistributable CUDA resmi NVIDIA ke .local/cuda-<versi>
# tanpa installer, hak administrator, atau perubahan environment global.
# Dipakai hanya untuk build fitur `cuda`; pengguna akhir cukup memiliki driver NVIDIA.
# Default 13.2.2: versi pertama yang resmi mendukung MSVC 14.5x (VS 2026), dan
# CUDA 13.x berjalan pada driver r580+ (minor version compatibility).
param([string]$Release = '13.2.2')
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path $PSScriptRoot -Parent
$base = 'https://developer.download.nvidia.com/compute/cuda/redist'
$target = Join-Path $root ".local\cuda-$Release"
$downloads = Join-Path $root '.local\downloads'
New-Item -ItemType Directory -Force $downloads, $target | Out-Null

# Manifest resmi memuat path dan SHA-256 tiap arsip.
$manifest = Invoke-RestMethod "$base/redistrib_$Release.json"
# cuda_crt dan libnvvm terpisah dari cuda_nvcc sejak CUDA 13.
foreach ($name in 'cuda_nvcc', 'cuda_crt', 'libnvvm', 'cuda_cudart', 'cuda_cccl', 'cuda_profiler_api', 'libcublas') {
    if (!$manifest.PSObject.Properties[$name]) { continue }
    $entry = $manifest.$name.'windows-x86_64'
    $zip = Join-Path $downloads (Split-Path $entry.relative_path -Leaf)
    if (!(Test-Path -LiteralPath $zip) -or (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ne $entry.sha256.ToUpperInvariant()) {
        Write-Host "Mengunduh $name $($manifest.$name.version)"
        Invoke-WebRequest "$base/$($entry.relative_path)" -OutFile "$zip.part"
        if ((Get-FileHash -LiteralPath "$zip.part" -Algorithm SHA256).Hash -ne $entry.sha256.ToUpperInvariant()) {
            Remove-Item -LiteralPath "$zip.part"
            throw "SHA-256 tidak cocok: $name"
        }
        Move-Item -LiteralPath "$zip.part" -Destination $zip -Force
    }
    $stage = Join-Path $downloads "extract-$name"
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    Expand-Archive -LiteralPath $zip -DestinationPath $stage
    # Arsip berisi satu folder <name>-windows-x86_64-<versi>-archive; gabungkan isinya.
    $inner = @(Get-ChildItem -LiteralPath $stage -Directory)[0].FullName
    Copy-Item -Path (Join-Path $inner '*') -Destination $target -Recurse -Force
    Remove-Item -LiteralPath $stage -Recurse -Force
    "{0} {1} {2}" -f $name, $manifest.$name.version, $entry.sha256
}
& (Join-Path $target 'bin\nvcc.exe') --version
Write-Host "CUDA $Release siap di $target"
