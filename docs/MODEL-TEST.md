# Uji model lokal sebelum integrasi backend

18 Oktober 2026. Pengujian terpisah dari UI melalui [scripts/test-model.ps1](../scripts/test-model.ps1). Tidak mengubah UI atau menghubungkan backend produk.

## Hasil

Whisper large-v3-turbo GGML dan Silero VAD 6.2.0 berhasil dimuat. Transkripsi Indonesia berjalan pada CUDA RTX 5060 Laptop GPU dan pada CPU eksplisit. Kedua jalur menghasilkan teks yang sama pada ketiga sampel.

| Sampel | Audio (detik) | CUDA pengulangan, wall (detik) | CPU, wall (detik) | Kesalahan kata | WER |
| --- | ---: | ---: | ---: | ---: | ---: |
| id_id_0000 | 8,58 | 1,754 | 11,327 | 0 / 15 | 0,00% |
| id_id_0001 | 7,68 | 1,674 | 8,668 | 1 / 9 | 11,11% |
| id_id_0002 | 9,48 | 1,767 | 8,738 | 2 / 17 | 11,76% |

WER gabungan berbobot jumlah kata: **3 / 41 = 7,32%**, bukan rata-rata tiga persentase. Normalisasi: lowercase Unicode, token huruf/angka; tanda baca dan pemisah tanda hubung diabaikan. Self-test metrik: 6 kasus lulus (identik, substitusi, penghapusan, penambahan, tanda hubung, hipotesis kosong).

Parser PowerShell lulus tanpa error. Uji model palsu tiga byte ditolak oleh verifikasi SHA-256 sebelum runtime dijalankan. Mode CPU pada hasil pembanding dikonfirmasi kembali dari log, bukan disimpulkan dari nama folder.

Perbedaan yang terlihat pada keluaran: `Jerman` menjadi `Cerman`, `sie` menjadi `si`, `huruf` menjadi `hukum`. Prediksi dan referensi lengkap tersedia di JSON hasil. Sampel ini hanya 25,74 detik audio formal; tidak membuktikan kualitas umum untuk ucapan spontan, campur bahasa, suara pelan, atau nama pengguna.

**Proses CUDA paling pertama memerlukan 25,543 detik.** Dua klip berikutnya pada putaran itu 1,798 dan 1,760 detik. Setelah diulang, ketiganya 1,674–1,767 detik. Penurunan konsisten dengan pemanasan cache/JIT, tetapi penyebab belum dipisahkan secara eksperimental. Jangan menjanjikan latency proses pertama sama dengan pengulangan.

Setiap klip memakai proses dan model baru. Wall time mencakup startup, model load, VAD, ASR, dan polling proses 150 ms. Contoh log CUDA pengulangan sampel 0000: load 1033,92 ms; encode 294,62 ms; decode 64,18 ms; batchd 50,66 ms; total internal 1499,49 ms. Ini **bukan** pengukuran worker dengan model menetap. Belum ada median/p95 yang representatif.

## Bukti perangkat dan runtime

- CPU: Intel Core i9-14900HX; 8 thread inferensi pada uji ini.
- GPU: NVIDIA GeForce RTX 5060 Laptop GPU, compute capability 12.0, VRAM sekitar 8151 MiB, driver 582.05.
- Runtime: build resmi whisper.cpp `b5454`, source commit `d1be6fde11ac6e0407606b4e42fe72d34add8037`, paket `whisper-bin-win-cuda-12.4.0-x64.zip`.
- SHA-256 paket: `afef0b881c500958921c3f5523b50e59ee2ec9b6f5cbd25b324c51ed308a957a`, cocok dengan GitHub release asset digest.
- Log mengidentifikasi GPU compute 12.0, memuat `ggml-cuda.dll`, memilih `using CUDA0 backend`, serta menempatkan bobot 1623,92 MB pada CUDA0. `nvidia-smi --query-compute-apps` juga menampilkan PID whisper-cli saat uji pertama.
- Telemetry mencapai GPU 100%. Penggunaan memori **seluruh device** teramati maksimum 2016 MiB; ini bukan peak VRAM per proses. WDDM melaporkan memori per proses N/A. Sebelum dan setelah uji, device menunjukkan 0 MiB. Sampling 150 ms ditambah waktu pemanggilan nvidia-smi tidak menjamin menangkap puncak sesaat.
- CPU pembanding memakai `-ng`; log `use gpu = 0`, tanpa pemilihan backend CUDA. Library CUDA dapat tetap terenumerasi pada startup paket yang sama; enumerasi bukan pemakaian GPU untuk inferensi.
- Paket ini melaporkan CUDA ARCHS sampai 900, bukan build native sm_120. Berhasil pada pengujian lokal ini tidak menetapkan kombinasi toolkit/whisper-rs untuk backend Rust. Tidak memasang atau mengubah driver, CUDA Toolkit, atau environment global.
- Silero 6.2.0 termuat dan mendeteksi ucapan. VAD berjalan pada CPU sesuai log, sementara model Whisper menggunakan GPU pada mode CUDA.

## Model dan sumber fixture

Model lokal berada di `models/`, sesuai lokasi file yang diunduh pengguna. SHA-256 diverifikasi sebelum setiap uji:

| Model | SHA-256 |
| --- | --- |
| ggml-large-v3-turbo.bin | `1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69` |
| ggml-silero-v6.2.0.bin | `2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987` |

Sumber bobot: [Whisper GGML, revision tetap](https://huggingface.co/ggerganov/whisper.cpp/tree/5359861c739e955e79d9a303bcbc70fb988958b1), [Silero GGML, revision tetap](https://huggingface.co/ggml-org/whisper-vad/tree/9ffd54a1e1ee413ddf265af9913beaf518d1639b).

Audio: tiga WAV pertama Indonesia dari [Google FLEURS](https://huggingface.co/datasets/google/fleurs), didistribusikan sebagai file individual melalui [FluidInference/fleurs-full](https://huggingface.co/datasets/FluidInference/fleurs-full/tree/1cca811bb8ea4d370345f108f00518167040282c/id_id). Lisensi CC BY 4.0, audio tidak diubah. Detail atribusi dan SHA-256 per file di [fixtures/fleurs-id](../fixtures/fleurs-id/ATTRIBUTION.md) dan [manifest](../fixtures/fleurs-id/manifest.json). Format terverifikasi: WAV PCM16, mono, 16 kHz.

## Reproduksi dan bukti tersimpan

Jalankan dari root proyek dengan PowerShell 7:

```powershell
.\scripts\test-model.ps1 -SelfTest
.\scripts\test-model.ps1
.\scripts\test-model.ps1 -Cpu
```

CUDA adalah default dan tidak fallback ke CPU diam-diam. Skrip memeriksa exit code, log pemilihan backend, aktivitas GPU, hasil nonkosong, checksum runtime/model/fixture, serta timeout 180 detik per proses. Model tidak diunduh ulang oleh skrip. Runtime dan fixture yang hilang diunduh melalui HTTPS ke `.part`, diperiksa, lalu di-rename. Setelah cache lengkap, inference dan checks tidak membutuhkan jaringan.

Snapshot bukti lokal ada di `docs/model-test/`: `first-cuda.json`, `repeat-cuda.json`, `cpu.json`, log sampel 0000, dan telemetry CUDA pertama. Log lengkap ada di `.local/runs/20261018-003944-084-cuda`, `20261018-004041-839-cuda`, dan `20261018-004055-926-cpu`. Folder hasil/log tersebut tidak disertakan dalam Git; skrip dapat membuat hasil baru dari fixture ber-checksum yang sama.

## Batas untuk sesi backend

Ini membuktikan **file model + whisper.cpp CLI** pada mesin ini, belum membuktikan binding whisper-rs. Bahasa `id`, transcribe, greedy best-of 1, beam-size 1, dan 8 thread menjadi baseline yang dapat dibandingkan saat memilih versi binding.

CLI `--vad` memadatkan segmen ucapan dengan padding default. Backend rancangan memakai rentang ucapan pertama sampai terakhir agar jeda tengah dipertahankan; hasil/latensi dapat berbeda. Jangan menyalin pemadatan CLI tanpa meninjau kebutuhan tersebut. Pada sampel 0000 VAD mendeteksi dua segmen dan mereduksi audio sekitar 24,6%.

Belum diuji: audio silence/noise-only, capture mikrofon, resampling/tail, rekaman 120 detik, cancel/stale result worker, hotkey, tray, injection, model menetap di RAM, atau pengujian akurasi 20 klip pengguna. Tidak melakukan tuning untuk memperbaiki tiga sampel ini, dan tidak menghubungkan UI. Tidak commit/push/publikasi.
