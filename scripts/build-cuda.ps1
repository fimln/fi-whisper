#requires -Version 7.0
# Menjalankan cargo dengan fitur `cuda` memakai CUDA dari scripts/setup-cuda.ps1.
# Environment hanya berlaku untuk proses ini. Contoh:
#   ./scripts/build-cuda.ps1 build --locked --release
#   ./scripts/build-cuda.ps1 test --release '--' --ignored fixture --nocapture  (kutip '--' agar tidak dibuang PowerShell)
# Tanpa blok param agar argumen seperti `--` diteruskan apa adanya ke cargo.
$CargoArgs = if ($args.Count) { [string[]]$args } else { @('build', '--locked', '--release') }
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path $PSScriptRoot -Parent
$cuda = Join-Path $root '.local\cuda-13.2.2'
if (!(Test-Path -LiteralPath (Join-Path $cuda 'bin\nvcc.exe'))) { throw 'CUDA belum ada. Jalankan scripts/setup-cuda.ps1.' }

# Muat environment MSVC x64 (cl.exe, link.exe, SDK) untuk generator Ninja.
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (!$vs) { throw 'Visual Studio C++ Build Tools tidak ditemukan.' }
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
cmd /c "`"$vcvars`" >nul && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] }
}
$ninja = Join-Path $vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja'

$env:CUDA_PATH = $cuda
$env:PATH = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer;$cuda\bin;$cuda\bin\x64;$ninja;$env:PATH"
$env:LIBCLANG_PATH = if ($env:LIBCLANG_PATH) { $env:LIBCLANG_PATH } else { 'C:\Program Files\LLVM\bin' }
# Generator Ninja tidak membutuhkan integrasi MSBuild CUDA di instalasi Visual Studio.
$env:CMAKE_GENERATOR = 'Ninja'
$env:CMAKE_CUDA_COMPILER = (Join-Path $cuda 'bin\nvcc.exe') -replace '\\', '/'
$env:CMAKE_CUDA_ARCHITECTURES = '120'
Set-Location $root
# --features diletakkan sebelum argumen lain agar tidak jatuh setelah `--`.
& cargo $CargoArgs[0] --features cuda @($CargoArgs | Select-Object -Skip 1)
$code = $LASTEXITCODE
if ($code -eq 0) {
    # Exe CUDA mengimpor cuBLAS secara dinamis; salin DLL ke samping exe agar bisa dijalankan langsung
    # dari target\<profil> tanpa CUDA di PATH. Hanya disalin jika belum ada atau berbeda ukuran/waktu.
    $buildProfile = if ($CargoArgs -contains '--release' -or $CargoArgs -contains '-r') { 'release' } else { 'debug' }
    $outDir = Join-Path $root "target\$buildProfile"
    if (Test-Path -LiteralPath $outDir) {
        foreach ($dll in 'cublas64_13.dll', 'cublasLt64_13.dll') {
            $src = Get-Item -LiteralPath (Join-Path $cuda "bin\x64\$dll")
            $dst = Join-Path $outDir $dll
            $old = Get-Item -LiteralPath $dst -ErrorAction SilentlyContinue
            if (!$old -or $old.Length -ne $src.Length -or $old.LastWriteTimeUtc -ne $src.LastWriteTimeUtc) {
                Copy-Item -LiteralPath $src.FullName -Destination $dst -Force
            }
        }
    }
}
exit $code
