# fi-whisper

Pratinjau aplikasi dikte lokal untuk Windows dengan Rust dan egui/eframe. UI Dikte dan Pengaturan tersedia, termasuk alur demo, timer, meter simulasi, serta hasil yang dapat dipilih, disalin, dan dihapus. Backend audio dan transkripsi belum terhubung ke UI.

Gunakan Rust MSVC 1.95 atau lebih baru. Jalankan `cargo run --locked`; buat executable dengan `cargo build --locked --release`. Aktifkan **Demo interaksi** untuk mencoba UI tanpa mengakses mikrofon. Pengaturan dan hasil hanya di RAM; menutup jendela mengakhiri aplikasi.

Untuk uji model, simpan [ggml-large-v3-turbo.bin](https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo.bin?download=true) dan [ggml-silero-v6.2.0.bin](https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v6.2.0.bin?download=true) di folder `models/`. Model, executable, runtime, rekaman, dan dokumen rancangan lokal tidak disertakan dalam Git.

Dengan PowerShell 7, jalankan `./scripts/test-model.ps1 -SelfTest`, lalu `./scripts/test-model.ps1` untuk CUDA atau tambahkan `-Cpu` untuk pembanding CPU. Skrip memeriksa checksum dan mengunduh runtime whisper.cpp resmi sekitar 685 MB serta tiga sampel publik FLEURS bila belum tersedia; hasil berada di `.local/runs/`. Setelah cache lengkap, uji dapat berjalan offline. Gunakan `-ModelDirectory` untuk lokasi model lain.

Uji awal tiga klip menghasilkan WER 7,32%; CUDA pengulangan 1,67–1,77 detik, CPU 8,67–11,33 detik per klip, dengan lonjakan CUDA pertama 25,54 detik. Setiap klip memuat model baru; ini belum mewakili kualitas umum atau latensi worker model persisten. Lihat [uji model](docs/MODEL-TEST.md), [validasi UI dan MCP egui](docs/VALIDATION.md), serta [atribusi](docs/CREDITS.md). Lisensi source: [MIT](LICENSE).
