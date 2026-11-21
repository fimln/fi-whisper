# fi-whisper

Aplikasi dikte Windows (Rust + egui 0.37) dengan mesin lokal Whisper/Qwen3-ASR serta layanan cloud Google Chirp 3 dan DeepSeek ASR. Shortcut global merekam suara; mesin lokal memakai VAD Silero dan GPU. Chirp 3 mengalirkan PCM saat merekam, sedangkan DeepSeek mengunggah WAV setelah rekaman selesai. Hasil final diketik ke jendela tujuan lewat SendInput. Repo publik: `fimln/fi-whisper`, lisensi MIT.

## Prinsip
- Mesin lokal tidak membuka koneksi jaringan. Koneksi ke penyedia cloud hanya dibuka untuk sesi dikte dengan mesin tersebut. Tidak ada centang izin tambahan, telemetry, preflight startup, atau fallback otomatis. Token, audio, dan hasil cloud hanya di RAM.
- Jangan simpan audio, transkrip, judul jendela, atau isi clipboard ke disk atau log. Hasil terakhir hanya di RAM.
- Tokio, tonic, dan reqwest diizinkan hanya untuk worker cloud. Komponen lokal tetap memakai thread/channel standar. Jangan tambah framework, ONNX, Python, downloader model, overlay, atau dependensi lain tanpa diminta.
- Jangan pernah mengetik ke jendela yang berubah, ke fi-whisper sendiri, atau mengirim Enter otomatis. Kalau ragu, simpan hasil untuk Salin manual.
- CPU fallback harus terlihat di UI, tidak boleh mengaku GPU.

## Struktur
- `src/main.rs`: UI (header, kartu Dikte, Hasil terakhir, Pengaturan, Model).
- `src/controller.rs`: state rekaman di thread sendiri (hotkey tetap jalan saat jendela tersembunyi), sesi, penyisipan teks.
- `src/transcribe.rs`: worker model (VAD, Whisper, Qwen3-ASR), abort per sesi, uji fixture `#[ignore]`.
- `src/cloud.rs`: ADC `authorized_user`, unggahan WAV DeepSeek, streaming gRPC Chirp 3 (region `us`/`eu`), cancellation, batas penggunaan bulanan UTC.
- `src/audio.rs`: CPAL + rtrb + rubato ke 16 kHz mono, batas 120 detik; cloud memakai resampling inkremental dan buffer terbatas tanpa membuang audio diam-diam.
- `src/focus/`: generator white, pink, dan brown noise dari fi-focus, serta kontrol media Windows (SMTC) pada HWND utama. Generator mengadaptasi `5mdt_noise/noise` (MIT, lihat `assets/LICENSE-5mdt.txt`) dan memakai koefisien Paul Kellet untuk pink noise.
- `src/windows.rs`: single instance, hotkey, tray, foreground, SendInput Unicode, clipboard tanpa History.
- `src/config.rs`: `Engine`, `Language`, `GoogleConfig`, `DeepSeekConfig`, config JSON atomik di `%LOCALAPPDATA%\fi-whisper\config.json`. Field lama `google.enabled` diabaikan.
- `src/theme.rs`: palet, font, logo, ikon, helper tampilan.
- `vendor/qwen3-asr`: salinan crate qwen3-asr 0.2.2 dengan patch (layout `*-hf`, pembatalan). Ubah seperlunya, catat di `docs/CREDITS.md`.
- `assets/`: logo (dibuat `scripts/make-logo.py`), ikon Octicons, screenshot README.

## Model
- Disimpan di `models\<folder>\`, tidak di-commit. Whisper `whisper-large-v3-turbo\ggml-large-v3-turbo.bin`, VAD `silero-vad-v6.2.0\ggml-silero-v6.2.0.bin`, Qwen `Qwen3-ASR-0.6B-hf\` dan `Qwen3-ASR-1.7B-hf\`.
- README menautkan revisi model. Catatan verifikasi hash ada di `docs/MODEL-TEST.md`. Model baru wajib dipin revision dan diverifikasi hash.
- Serde `"qwen3-asr"` berarti 0.6B demi kompatibilitas config lama. Jangan diubah.
- Serde `"google-chirp-3"` dan `"deepseek-asr"` memilih mesin cloud, tanpa model ASR/VAD lokal. Jangan memuat model lokal saat startup cloud.

## Cloud
- DeepSeek membaca `DEEPSEEK_API_KEY` atau `deepseek.api_key` dari konfigurasi. Jangan mencatat kunci API.
- ADC Google dibaca dari `GOOGLE_APPLICATION_CREDENTIALS` atau lokasi standar gcloud Windows; hanya `authorized_user`. Jangan log token atau detail respons server yang bisa memuat data request.
- `cloud-usage.json` di direktori konfigurasi hanya menyimpan durasi per bulan UTC untuk setiap project Google dan untuk DeepSeek (`deepseek:api`). Default batas 600 menit; cadangkan 120 detik sebelum sesi lalu sesuaikan menurut durasi PCM yang diserahkan ke transport Google atau disiapkan untuk unggahan DeepSeek, dibulatkan ke detik. Reservasi tetap dihitung jika proses crash. Ini penghitung lokal, bukan tagihan Google.
- Streaming Google mengikuti jadwal durasi audio yang tetap agar keterlambatan timer tidak terakumulasi. Buffer penuh, koneksi gagal, atau pembatalan harus menghentikan sesi tanpa menyisipkan hasil parsial/gagal. Hasil final tetap melalui pemeriksaan sesi dan jendela tujuan yang sama dengan mesin lokal.

## UI
- Centang `Aktifkan fi-focus` di Pengaturan menyimpan `focus_enabled` (default mati). Jika aktif, startup menyiapkan sesi media dalam keadaan berhenti. Memilih jenis audio langsung memutarnya. Putar melanjutkan pilihan terakhir dalam sesi, sedangkan Berikutnya/Sebelumnya mengganti jenis audio secara siklik.
- Pemutaran mengikuti proses dan tray fi-whisper. Worker fi-focus memiliki stream output WASAPI shared sendiri. Dikte tetap memakai stream input mikrofon yang dipilih, termasuk saat noise diputar, diganti, atau dijeda. Saat memverifikasi perubahan, uji rekam dan putar bersamaan serta kontrol media saat jendela tersembunyi.
- Palet amber di `theme.rs`. Teks di atas amber selalu `ON_ACCENT` (gelap), tidak pernah putih.
- Tanpa dot status, hint mesin, atau tagline. Sediakan teks singkat dalam bahasa Indonesia dan Inggris sesuai pengaturan bahasa.
- Header Chirp 3 saat idle: badge `Chirp 3`, lalu status `Siap` dengan warna siap biasa. Kartu Model langsung menampilkan pengaturan project, tanpa logo Google Cloud atau teks pemeriksaan ADC; bantuan tetap di `Setup ADC`.
- Hindari elemen yang muncul atau hilang di baris tombol (mis. spinner), karena menggeser layout.
- Di layout `right_to_left`, tambahkan anak dengan urutan terbalik. `Id::new` dan `WidgetType` deprecated di egui 0.37.
- Cek visual lewat MCP egui: build `--features inspection`, jalankan dengan `EGUI_INSPECTION=1`. Fitur ini tidak boleh masuk build rilis.

## Perintah
```powershell
./scripts/setup-cuda.ps1                                   # CUDA redistributable ke .local/, sekali saja
./scripts/build-cuda.ps1 build --locked --release          # selalu memakai fitur cuda,qwen-cuda
./scripts/build-cuda.ps1 clippy --locked --all-targets '--' -D warnings
./scripts/build-cuda.ps1 test --locked --release
./scripts/build-cuda.ps1 test --locked --release '--' --ignored fixture --nocapture --test-threads=1
# Opt-in tes cloud berbayar dengan ADC; jalankan terpisah dan serial:
$env:FI_WHISPER_TEST_PROJECT = 'PROJECT_ID'
./scripts/build-cuda.ps1 test --locked --release cloud::tests::cloud_ '--' --ignored --nocapture --test-threads=1
./scripts/package.ps1                                      # dist/fi-whisper, tutup aplikasi dulu
```
Peringatan `vswhere.exe` dan LNK4098 (LIBCMT) sudah ada sejak awal dan tidak perlu diperbaiki.

## Verifikasi
- Setelah mengubah kode: `cargo fmt`, clippy, unit test. Kalau menyentuh worker atau model, jalankan juga uji fixture.
- Perubahan transport/audio cloud perlu tes fixture cloud, hening, pembatalan, batas penggunaan, dan streaming 120 detik. Tes ini memakai kuota Google dan diabaikan secara default; jangan memasukkannya ke tes lokal/CI tanpa opt-in. Jangan simpan transkrip ke disk; laporkan metrik/status saja.
- Klaim GPU butuh bukti dari log backend (`GPU CUDA0`) atau nvidia-smi. Flag konfigurasi saja tidak cukup.
- Sebutkan apa yang diamati langsung dan apa yang belum diuji (mis. injection ke aplikasi tertentu).

## Privasi repo
- Jangan commit `models/`, `dist/`, `target/`, `.local/`, `docs/model-test/`, WAV fixture, log, token, atau key. Semuanya sudah di `.gitignore`.
- Jangan tulis path pribadi (`C:\Users\<nama>`), nama komputer, atau transkrip suara pengguna di file yang di-commit. Fixture uji hanya FLEURS publik.
- Catatan kerja lokal yang lebih rinci (jika ada) tetap di luar Git.

## Workflow
- Commit dan push hanya jika diminta. Pesan commit singkat, bahasa Inggris.
- Tulis komentar dalam bahasa Indonesia untuk menjelaskan alasan yang sulit disimpulkan dari kode.
- Balas dalam bahasa Indonesia, langsung ke inti, tanpa trope AI.
