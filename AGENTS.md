# fi-whisper

Aplikasi dikte lokal untuk Windows (Rust + egui 0.37). Shortcut global merekam suara, VAD Silero memotong ucapan, lalu Whisper atau Qwen3-ASR di GPU mengubahnya jadi teks yang diketik ke jendela tujuan lewat SendInput. Repo publik: `fimln/fi-whisper`, lisensi MIT.

## Prinsip
- Semua inferensi lokal. Runtime tidak boleh membuka koneksi jaringan, telemetry, atau cloud fallback. Satu-satunya akses jaringan adalah tautan header yang dibuka pengguna lewat browser.
- Jangan simpan audio, transkrip, judul jendela, atau isi clipboard ke disk atau log. Hasil terakhir hanya di RAM.
- Minimal: jangan tambah Tokio, framework, ONNX, Python, downloader model, overlay, atau dependensi baru tanpa diminta. Pakai thread dan channel standar.
- Jangan pernah mengetik ke jendela yang berubah, ke fi-whisper sendiri, atau mengirim Enter otomatis. Kalau ragu, simpan hasil untuk Salin manual.
- CPU fallback harus terlihat di UI, tidak boleh mengaku GPU.

## Struktur
- `src/main.rs`: UI (header, kartu Dikte, Hasil terakhir, Pengaturan, Model).
- `src/controller.rs`: state rekaman di thread sendiri (hotkey tetap jalan saat jendela tersembunyi), sesi, penyisipan teks.
- `src/transcribe.rs`: worker model (VAD, Whisper, Qwen3-ASR), abort per sesi, uji fixture `#[ignore]`.
- `src/audio.rs`: CPAL + rtrb + rubato ke 16 kHz mono, batas 120 detik.
- `src/windows.rs`: single instance, hotkey, tray, foreground, SendInput Unicode, clipboard tanpa History.
- `src/config.rs`: `Engine`, `Language`, config JSON atomik di `%LOCALAPPDATA%\fi-whisper\config.json`.
- `src/theme.rs`: palet, font, logo, ikon, helper tampilan.
- `vendor/qwen3-asr`: salinan crate qwen3-asr 0.2.2 dengan patch (layout `*-hf`, pembatalan). Ubah seperlunya, catat di `docs/CREDITS.md`.
- `assets/`: logo (dibuat `scripts/make-logo.py`), ikon Octicons, screenshot README.

## Model
- Disimpan di `models\<folder>\`, tidak di-commit. Whisper `whisper-large-v3-turbo\ggml-large-v3-turbo.bin`, VAD `silero-vad-v6.2.0\ggml-silero-v6.2.0.bin`, Qwen `Qwen3-ASR-0.6B-hf\` dan `Qwen3-ASR-1.7B-hf\`.
- Sumber, revision, dan SHA-256 ada di README. Model baru wajib dipin revision dan diverifikasi hash.
- Serde `"qwen3-asr"` berarti 0.6B demi kompatibilitas config lama. Jangan diubah.

## UI
- Palet amber di `theme.rs`. Teks di atas amber selalu `ON_ACCENT` (gelap), tidak pernah putih.
- Tanpa dot status, hint mesin, atau tagline. Teks UI singkat, bahasa Indonesia.
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
./scripts/package.ps1                                      # dist/fi-whisper, tutup aplikasi dulu
```
Peringatan `vswhere.exe` dan LNK4098 (LIBCMT) sudah ada sejak awal dan tidak perlu diperbaiki.

## Verifikasi
- Setelah mengubah kode: `cargo fmt`, clippy, unit test. Kalau menyentuh worker atau model, jalankan juga uji fixture.
- Klaim GPU butuh bukti dari log backend (`GPU CUDA0`) atau nvidia-smi. Flag konfigurasi saja tidak cukup.
- Sebutkan apa yang diamati langsung dan apa yang belum diuji (mis. injection ke aplikasi tertentu).

## Privasi repo
- Jangan commit `models/`, `dist/`, `target/`, `.local/`, `docs/model-test/`, WAV fixture, log, token, atau key. Semuanya sudah di `.gitignore`.
- Jangan tulis path pribadi (`C:\Users\<nama>`), nama komputer, atau transkrip suara pengguna di file yang di-commit. Fixture uji hanya FLEURS publik.
- Catatan kerja lokal yang lebih rinci (jika ada) tetap di luar Git.

## Workflow
- Commit dan push hanya jika diminta. Pesan commit singkat, bahasa Inggris.
- Gaya kode singkat, komentar hanya untuk alasan yang tidak jelas dari kode.
- Balas dalam bahasa Indonesia, langsung ke inti, tanpa trope AI.
