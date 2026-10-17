# fi-whisper

Aplikasi dikte lokal untuk Windows dengan Rust dan egui/eframe. Tekan shortcut global (default `Ctrl+Alt+Space`) di aplikasi lain, bicara, tekan lagi; audio diproses di perangkat (VAD Silero + Whisper large-v3-turbo melalui whisper-rs) lalu teks diketik ke jendela yang tadi fokus memakai Unicode SendInput. Tidak ada koneksi jaringan, telemetry, atau penyimpanan audio/transkrip.

Build: Rust MSVC 1.95+, Visual Studio Build Tools (C++), CMake, dan LLVM untuk bindgen (`$env:LIBCLANG_PATH='C:\Program Files\LLVM\bin'`). `.cargo/config.toml` memaksa flag `/O2` untuk whisper.cpp; tanpa itu inferensi sekitar 10x lebih lambat.

- CPU: `cargo build --locked --release`.
- CUDA (NVIDIA, driver r580+): `./scripts/setup-cuda.ps1` sekali (mengunduh ~460 MB komponen resmi CUDA 13.2.2 ke `.local/`, tanpa installer/admin), lalu `./scripts/build-cuda.ps1 build --locked --release`. Build menargetkan `sm_120` (RTX 50xx); ubah `CMAKE_CUDA_ARCHITECTURES` di skrip untuk GPU lain.
- Paket portable: `./scripts/package.ps1` membuat `dist/fi-whisper/` berisi exe CUDA, `cublas64_13.dll`, `cublasLt64_13.dll`, dan lisensi. Pengguna akhir hanya butuh driver NVIDIA dan Visual C++ Redistributable.

Model: simpan [ggml-large-v3-turbo.bin](https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo.bin?download=true) dan [ggml-silero-v6.2.0.bin](https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v6.2.0.bin?download=true) di `%LOCALAPPDATA%\fi-whisper\models\`, folder `models\` di samping exe, atau `models\` di direktori kerja; file tersebut dimuat otomatis saat start. Lokasi lain dapat dipilih di Pengaturan lalu tekan **Muat model**. Checksum ada di `scripts/test-model.ps1`.

Pemakaian: konfigurasi (model, mikrofon, bahasa, shortcut) tersimpan di `%LOCALAPPDATA%\fi-whisper\config.json`; file rusak tidak ditimpa sampai Anda menekan Reset. Tombol Mulai di jendela hanya untuk uji dan tidak mengetik ke aplikasi lain. Rekaman berhenti otomatis setelah 2 menit. Hasil terakhir hanya ada di RAM; **Salin** menandai clipboard agar dikecualikan dari Clipboard History/cloud. Menutup jendela menyembunyikan aplikasi ke tray; klik ikon tray untuk membuka, pilih **Keluar** untuk berhenti.

Batas: teks tidak dimasukkan bila jendela tujuan berubah, tertutup, atau modifier masih ditekan; hasil tetap bisa disalin. Aplikasi yang berjalan sebagai administrator tidak dapat menerima input. Pemeriksaan jendela tidak menjamin field di dalamnya tidak berubah, dan tidak ada deteksi field password. Lihat [validasi](docs/VALIDATION.md), [uji model](docs/MODEL-TEST.md), dan [atribusi](docs/CREDITS.md). Lisensi source: [MIT](LICENSE).
