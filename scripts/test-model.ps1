#requires -Version 7.0
param(
    [string]$ModelDirectory = (Join-Path $PSScriptRoot '..\models'),
    [switch]$Cpu,
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path $PSScriptRoot -Parent

function Get-WordError([string]$Reference, [string]$Hypothesis) {
    $words = { param($text) @([regex]::Matches($text.ToLowerInvariant(), '[\p{L}\p{N}]+') | ForEach-Object Value) }
    $ref = @(& $words $Reference)
    $hyp = @(& $words $Hypothesis)
    [int[]]$previous = 0..$hyp.Count
    for ($i = 1; $i -le $ref.Count; $i++) {
        [int[]]$current = [int[]]::new($hyp.Count + 1)
        $current[0] = $i
        for ($j = 1; $j -le $hyp.Count; $j++) {
            $cost = [int]($ref[$i - 1] -ne $hyp[$j - 1])
            $current[$j] = [Math]::Min([Math]::Min($previous[$j] + 1, $current[$j - 1] + 1), $previous[$j - 1] + $cost)
        }
        $previous = $current
    }
    [pscustomobject]@{ errors = $previous[$hyp.Count]; words = $ref.Count }
}

if ($SelfTest) {
    foreach ($case in @(
        @('Halo, INDONESIA!', 'halo indonesia', 0, 2),
        @('satu dua', 'satu tiga', 1, 2),
        @('satu dua', 'satu', 1, 2),
        @('satu', 'satu dua', 1, 1),
        @('tim-tim', 'tim tim', 0, 2),
        @('satu', '', 1, 1)
    )) {
        $score = Get-WordError $case[0] $case[1]
        if ($score.errors -ne $case[2] -or $score.words -ne $case[3]) { throw "WER self-test gagal: $($case[0])" }
    }
    Write-Host 'WER self-test: 6 lulus.'
    return
}

function Assert-Hash([string]$Path, [string]$Hash) {
    if (!(Test-Path -LiteralPath $Path -PathType Leaf)) { throw "File tidak ditemukan: $Path" }
    if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -ne $Hash) { throw "SHA-256 tidak cocok: $Path" }
}

function Get-VerifiedFile([string]$Url, [string]$Path, [string]$Hash) {
    if (!(Test-Path -LiteralPath $Path)) {
        Write-Host "Mengunduh $Url"
        Invoke-WebRequest $Url -OutFile "$Path.part"
        Assert-Hash "$Path.part" $Hash
        Move-Item -LiteralPath "$Path.part" -Destination $Path
    }
    Assert-Hash $Path $Hash
}

$asr = Join-Path $ModelDirectory 'ggml-large-v3-turbo.bin'
$vad = Join-Path $ModelDirectory 'ggml-silero-v6.2.0.bin'
Assert-Hash $asr '1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69'
Assert-Hash $vad '2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987'

$cache = Join-Path $root '.local'
$runtime = Join-Path $cache 'whisper-b5454'
$downloads = Join-Path $cache 'downloads'
$fixtures = Join-Path $root 'fixtures\fleurs-id'
$backend = if ($Cpu) { 'cpu' } else { 'cuda' }
$output = Join-Path $cache ('runs\{0}-{1}' -f (Get-Date -Format 'yyyyMMdd-HHmmss-fff'), $backend)
New-Item -ItemType Directory -Force $downloads, $output | Out-Null

# Official CUDA build also supports -ng for an explicit CPU comparison.
$archive = Join-Path $downloads 'whisper-cuda-b5454.zip'
Get-VerifiedFile 'https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-win-cuda-12.4.0-x64.zip' $archive 'afef0b881c500958921c3f5523b50e59ee2ec9b6f5cbd25b324c51ed308a957a'
if (!(Test-Path -LiteralPath $runtime)) { Expand-Archive -LiteralPath $archive -DestinationPath $runtime }
$cli = @(Get-ChildItem -LiteralPath $runtime -Filter whisper-cli.exe -Recurse)
if ($cli.Count -ne 1) { throw 'Tidak ditemukan tepat satu whisper-cli.exe dalam runtime.' }
$manifest = Get-Content (Join-Path $fixtures 'manifest.json') -Raw -Encoding utf8 | ConvertFrom-Json

if (!$Cpu) {
    & nvidia-smi --query-gpu=name,driver_version,compute_cap --format=csv | Set-Content (Join-Path $output 'device.txt')
    if ($LASTEXITCODE -ne 0) { throw 'nvidia-smi gagal. Gunakan -Cpu hanya bila memang ingin menguji CPU.' }
}

$results = foreach ($sample in $manifest.samples) {
    $wav = Join-Path $fixtures $sample.file
    $url = "https://huggingface.co/datasets/$($manifest.mirror)/resolve/$($manifest.revision)/id_id/$($sample.file)"
    Get-VerifiedFile $url $wav $sample.sha256
    $prefix = Join-Path $output $sample.id
    $arguments = @('-m', $asr, '-vm', $vad, '--vad', '-l', 'id', '-bs', '1', '-bo', '1', '-t', '8', '-nt', '-otxt', '-of', $prefix, '-f', $wav)
    if ($Cpu) { $arguments += '-ng' }
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $cli[0].FullName -ArgumentList ($arguments | ForEach-Object { '"{0}"' -f $_ }) -WindowStyle Hidden -PassThru -RedirectStandardOutput "$prefix.stdout.log" -RedirectStandardError "$prefix.stderr.log"
    $telemetry = [Collections.Generic.List[string]]::new()
    try {
        while (!$process.HasExited) {
            if (!$Cpu) {
                $reading = & nvidia-smi --query-gpu=timestamp,utilization.gpu,memory.used --format=csv,noheader,nounits
                if ($LASTEXITCODE -ne 0) { throw 'Pembacaan telemetry GPU gagal.' }
                $telemetry.Add([string]$reading)
            }
            if ($timer.Elapsed.TotalSeconds -gt 180) { throw 'Inferensi melampaui batas 180 detik.' }
            Start-Sleep -Milliseconds 150
            $process.Refresh()
        }
        $process.WaitForExit()
    } finally {
        if (!$process.HasExited) { $process.Kill(); $process.WaitForExit() }
        $telemetry | Set-Content "$prefix.gpu.csv" -Encoding utf8
    }
    $timer.Stop()
    $log = Get-Content "$prefix.stderr.log" -Raw -Encoding utf8
    if ($process.ExitCode -ne 0) { throw "whisper-cli exit $($process.ExitCode). Baca $prefix.stderr.log" }
    if (!$Cpu -and $log -notmatch 'using CUDA\d+ backend') { throw "CUDA tidak terbukti aktif. Baca $prefix.stderr.log" }
    if ($Cpu -and ($log -notmatch 'use gpu\s*=\s*0' -or $log -match 'using CUDA\d+ backend')) { throw "Mode CPU tidak terkonfirmasi. Baca $prefix.stderr.log" }
    $hypothesis = (Get-Content "$prefix.txt" -Raw -Encoding utf8).Trim()
    if ([string]::IsNullOrWhiteSpace($hypothesis)) { throw "Hasil ucapan kosong: $($sample.id)" }
    $score = Get-WordError $sample.reference $hypothesis
    $gpuRows = @($telemetry | ConvertFrom-Csv -Header time, utilization, memory)
    $peakUtilization = ($gpuRows | ForEach-Object { [int]$_.utilization } | Measure-Object -Maximum).Maximum
    $peakMemory = ($gpuRows | ForEach-Object { [int]$_.memory } | Measure-Object -Maximum).Maximum
    if (!$Cpu -and !($peakUtilization -gt 0)) { throw "Aktivitas GPU belum teramati: $($sample.id)" }
    $audio = [IO.File]::ReadAllBytes($wav)
    # shortcut: fixtures pinned berupa PCM16 mono 16 kHz dengan header WAV 44 byte; gunakan parser WAV saat menerima file lain.
    if ([Text.Encoding]::ASCII.GetString($audio, 0, 4) -ne 'RIFF' -or [BitConverter]::ToInt32($audio, 24) -ne 16000 -or [BitConverter]::ToInt16($audio, 22) -ne 1 -or [BitConverter]::ToInt16($audio, 34) -ne 16 -or [Text.Encoding]::ASCII.GetString($audio, 36, 4) -ne 'data') { throw "Format fixture berubah: $wav" }
    [pscustomobject]@{
        sample = $sample.id; backend = $backend; audio_seconds = [Math]::Round(($audio.Length - 44) / 32000.0, 3)
        wall_seconds = [Math]::Round($timer.Elapsed.TotalSeconds, 3)
        word_errors = $score.errors; reference_words = $score.words
        wer_percent = [Math]::Round(100.0 * $score.errors / $score.words, 2)
        process_id = $process.Id; peak_gpu_percent = $peakUtilization; peak_device_memory_mib = $peakMemory
        arguments = $arguments
        reference = $sample.reference; hypothesis = $hypothesis
    }
    Write-Host "Selesai $($sample.id): $($score.errors)/$($score.words) word errors."
}
$results | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $output 'results.json') -Encoding utf8
$results | Format-Table sample, backend, audio_seconds, wall_seconds, wer_percent
Write-Host "Hasil, log backend, dan telemetry: $output"
