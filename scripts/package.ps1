#requires -Version 7.0
# Membuat folder portable dist/fi-whisper: exe CUDA, DLL cuBLAS yang diimpor, dan lisensi.
# Model tidak disertakan; letakkan di dist/fi-whisper/models atau %LOCALAPPDATA%\fi-whisper\models.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path $PSScriptRoot -Parent
$cuda = Join-Path $root '.local\cuda-13.2.2'
& (Join-Path $PSScriptRoot 'build-cuda.ps1') build --locked --release
if ($LASTEXITCODE -ne 0) { throw 'Build CUDA gagal.' }

$dist = Join-Path $root 'dist\fi-whisper'
if (Test-Path -LiteralPath $dist) { Remove-Item -LiteralPath $dist -Recurse -Force }
New-Item -ItemType Directory -Force (Join-Path $dist 'licenses') | Out-Null
Copy-Item (Join-Path $root 'target\release\fi-whisper.exe') $dist
# fi-whisper.exe mengimpor cublas64_13.dll, yang memuat cublasLt64_13.dll. cudart ditautkan statis.
foreach ($dll in 'cublas64_13.dll', 'cublasLt64_13.dll') { Copy-Item (Join-Path $cuda "bin\x64\$dll") $dist }
Copy-Item (Join-Path $cuda 'LICENSE') (Join-Path $dist 'licenses\NVIDIA-CUDA-EULA.txt')
Copy-Item (Join-Path $root 'LICENSE') (Join-Path $dist 'licenses\fi-whisper-MIT.txt')
Copy-Item (Join-Path $root 'fonts\*.txt') (Join-Path $dist 'licenses')
Copy-Item (Join-Path $root 'README.md'), (Join-Path $root 'docs\CREDITS.md') $dist
Get-ChildItem $dist -Recurse -File | Select-Object @{ n = 'File'; e = { $_.FullName.Substring($dist.Length + 1) } }, @{ n = 'MB'; e = { [Math]::Round($_.Length / 1MB, 1) } }
