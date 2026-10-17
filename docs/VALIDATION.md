# Validasi

## CUDA (18 Oktober 2026)

CUDA 12.9.1 gagal: nvcc menolak MSVC 14.51 (VS 2026), dan dengan `-allow-unsupported-compiler` `cudafe++` crash (ACCESS_VIOLATION). Diganti CUDA 13.2.2, versi pertama yang resmi mendukung VS 2026 menurut [panduan instalasi CUDA Windows](https://docs.nvidia.com/cuda/cuda-installation-guide-microsoft-windows/index.html); berjalan pada driver 582.05 (CUDA 13.0) melalui minor version compatibility. Komponen redistributable diunduh dengan SHA-256 dari manifest NVIDIA, dibangun dengan generator Ninja dan `CMAKE_CUDA_ARCHITECTURES=120` (CMake memakai `120a`, SASS native, tanpa JIT PTX).

- `./scripts/build-cuda.ps1 test --locked --release '--' --ignored fixture --nocapture`: backend `GPU CUDA0`, log `Device 0: NVIDIA GeForce RTX 5060 Laptop GPU, compute capability 12.0`; load 1,0-3,3 dtk; ASR 0,33-0,41 dtk per klip FLEURS (CPU 8,9-12,0 dtk); teks identik dengan CPU/CLI. Tidak ada lonjakan pemanasan 25 dtk seperti runtime b5454. Utilisasi GPU puncak 97%, memori device 2196 MiB (seluruh device, bukan per proses).
- `./scripts/build-cuda.ps1 clippy --locked --all-targets '--' -D warnings`: lulus. Linker memberi peringatan LNK4098 (LIBCMT dari cudart statis); tidak ada kegagalan teramati.
- Paket `dist/fi-whisper` diluncurkan dari shell tanpa CUDA di PATH: proses memuat `cublas64_13.dll`/`cublasLt64_13.dll` dari folder paket dan `nvcuda.dll` dari driver, tercantum di nvidia-smi, responsif.
- MCP egui (build CUDA + inspection): status Siap, `Backend: GPU CUDA0`, diagnostik backend tampil. Rekaman tombol 1,5 dtk di ruangan sunyi lolos VAD dan Whisper menghasilkan halusinasi "Terima kasih." (ASR 0,51 dtk). Ditambahkan filter segmen dengan `no_speech_probability >= 0,6`; fixture tetap identik dan noise sintetis menghasilkan Tidak ada ucapan, tetapi kasus mikrofon nyata tersebut belum diuji ulang.

## Integrasi backend (18 Oktober 2026)

Build CPU (tanpa fitur `cuda`), whisper-rs 0.16.0 / whisper.cpp 1.8.3, MSVC 14.51, CMake 4.2.0, LLVM libclang. Backend aktual yang dilaporkan app berasal dari log inisialisasi whisper.cpp, bukan flag konfigurasi.

Checks: `cargo fmt -- --check`, `cargo clippy --locked --all-targets -- -D warnings` (dengan dan tanpa `inspection`), `cargo test --locked` (10 lulus, 1 ignored), `cargo build --locked --release` lulus. Unit test mencakup konversi sampel signed/unsigned, downmix, overflow ring buffer, durasi/tail resampling, rentang VAD dengan jeda tengah, penggabungan segmen, ringkasan backend, config atomik/rusak, keputusan insert saat target berubah, dan surrogate pair SendInput.

Uji model eksplisit `cargo test --release -- --ignored fixture --nocapture` (worker persisten, CPU, 8 thread): load 0,9 dtk; silence 3 dtk menghasilkan Tidak ada ucapan; tiga fixture FLEURS menghasilkan teks identik dengan uji CLI, ASR 8,9-12,0 dtk per klip (VAD 11-51 ms). Dua temuan diperbaiki:

- cmake-rs tidak memberi `/O2` pada konfigurasi Release MSVC; whisper.cpp terkompilasi tanpa optimasi dan ASR ~90 dtk per klip. `.cargo/config.toml` menetapkan `CMAKE_C/CXX_FLAGS_RELEASE`.
- `set_abort_callback_safe` di whisper-rs 0.16 membaca user data dengan tipe salah sehingga encode gagal acak (error -6). Diganti callback raw dengan atomic sesi.

MCP egui pada build release `--features inspection`, mikrofon default Realtek Microphone Array:

| Skenario | Hasil |
| --- | --- |
| Start | Model dari `models/` dimuat otomatis, status Siap, backend CPU, shortcut Ctrl+Alt+Space terdaftar |
| Mulai/Selesai via tombol | Mikrofon aktif, timer berjalan; 16,1 dtk audio 16 kHz diterima; VAD tidak menemukan ucapan di ruangan sunyi; tidak ada hasil/insert |
| Batalkan saat merekam | Kembali Siap, audio dibuang |
| Shortcut tidak valid | Error tampil, shortcut lama tetap aktif |
| Tutup jendela pertama kali | Dialog penjelasan tray tampil; Sembunyikan menyembunyikan jendela |
| Hotkey saat tersembunyi | Dua penekanan (SendInput sintetis) merekam 2,3 dtk dan memproses tanpa jendela terlihat |
| Instance kedua | Menampilkan pesan dan tidak memulai app kedua |

Belum diverifikasi: dikte ucapan nyata dari mikrofon dan penyisipan teks ke aplikasi lain (butuh suara pengguna), meter level (selalu kosong di ruangan sunyi; belum diuji dengan suara), menu tray Buka/Batalkan/Keluar via klik nyata (jalur Buka diuji memanggil ShowWindow yang sama), cancel saat inferensi berjalan, sleep/resume, mikrofon dilepas, aplikasi elevated, matriks aplikasi, dan paket portable di mesin lain.

## Tahap UI

Tanggal: 18 Oktober 2026, Windows x64. Cakupan: UI Rust egui, tanpa backend dikte.

Uji model terpisah dilakukan setelah tahap UI: lihat [MODEL-TEST.md](MODEL-TEST.md). Pernyataan belum diuji di bawah merupakan batas sesi UI, bukan pembatalan hasil uji CLI tersebut.

Revisi teks UI berikutnya pada 18 Oktober 2026 menghapus tagline, meringkas petunjuk/status/pesan model, dan mengganti teks contoh dengan kalimat rapat. Dua test Rust, fmt, build release, dan pemeriksaan MCP egui lulus. Screenshot Dikte dan Pengaturan diamati; alur Mulai/Selesai, teks hasil baru, Konfirmasi pilihan, dan Tutup pesan berfungsi. Screenshot revisi disimpan lokal sebagai `docs/ui-copy-dictation.png` dan `docs/ui-copy-settings.png`.

## Build dan pemeriksaan otomatis

- rustc 1.99.1 (58cf8f9c3), cargo 1.99.1 (5f94df478).
- eframe/egui 0.37.0, renderer glow; rfd 0.17.2. Versi dependency lengkap di Cargo.lock.
- `cargo fmt -- --check`: lulus.
- `cargo test --locked`: 2 lulus, 0 gagal.
- `cargo clippy --locked --all-targets --features inspection -- -D warnings`: lulus.
- `cargo build --locked --release`: lulus, build biasa tanpa feature inspection.
- Executable: `target/release/fi-whisper.exe`.
- Smoke test peluncuran release dari PowerShell biasa berhasil: proses hidup dan Responding=True; port inspeksi 5719 tidak listening. Pemeriksaan visual MCP memakai build debug dengan source UI yang sama. Build release dibiarkan terbuka untuk pengguna.

Unit test menguji alur demo dari belum siap sampai hasil, durasi capture, pemeliharaan hasil lama saat cancel, batas 120 detik, serta pematian demo saat pemrosesan agar hasil tertunda tidak muncul. Batas 120 detik diuji memakai waktu sintetis, bukan merekam selama dua menit.

## MCP egui dan pengamatan visual

App debug dijalankan dengan `--features inspection` dan `EGUI_INSPECTION=127.0.0.1:5719`, lalu dihubungkan melalui MCP egui. Penilaian berikut memakai tree aksesibilitas, input MCP, dan screenshot yang benar-benar diamati:

| Skenario | Hasil |
| --- | --- |
| Awal tanpa backend | Belum siap; Mulai, Salin, dan Hapus nonaktif |
| Aktifkan demo | Loading lalu Siap, tombol Mulai aktif |
| Mulai dan Selesai | Timer/meter simulasi tampil, tombol proses nonaktif, lalu teks contoh muncul |
| Batalkan saat merekam | Kembali Siap, hasil kosong tetap kosong |
| Batalkan saat memproses | Kembali Siap; unit test juga memastikan tidak ada hasil tertunda |
| Pengaturan saat merekam | Input shortcut dinonaktifkan; kembali ke Dikte tetap bisa membatalkan |
| Salin | Status berubah; perbandingan `Get-Clipboard -Raw` dengan teks contoh menghasilkan True tanpa mencetak clipboard lain |
| Hapus | Hasil hilang, Salin kembali nonaktif |
| Bahasa | Indonesia dapat diubah ke English, lalu dikembalikan |
| Shortcut | Kolom bisa diedit, dipilih semua dengan command/Ctrl+A, dan dikembalikan ke default |
| Model belum dipilih | Pesan meminta ASR dan VAD muncul; tombol pemulihan kembali ke Belum siap |
| 860 × 820 | Dikte dan ringkasan backend terlihat; teks hasil demo tidak bertumpuk |
| 860 × 1000 | Pengaturan dan pesan kesalahan model terlihat |
| 620 × 540 | Konten menyesuaikan lebar; scrollbar dapat diseret sampai hasil dan ringkasan backend terlihat |

Screenshot diperiksa dan disimpan lokal sebagai `docs/ui-initial.png`, `ui-recording.png`, `ui-result.png`, `ui-settings.png`, `ui-small.png`, serta `ui-small-scrolled.png`. Screenshot tidak disertakan dalam snapshot Git.

Perintah wheel `mcp egui scroll` dua kali menyebabkan koneksi inspeksi berakhir (`early eof` / Windows 10053). Aplikasi tetap hidup dan responsif; disconnect/attach memulihkan koneksi. Verifikasi scroll dilanjutkan dengan drag scrollbar dan berhasil. Wheel melalui MCP belum terverifikasi. Tidak ada perubahan dependency untuk masalah koneksi tool ini.

## Ditunda dan belum diuji

- Dialog pemilih file menggunakan rfd native, tetapi membuka/memilih/membatalkan dialog native belum diuji melalui MCP egui. Path disimpan sebagai PathBuf; file tidak dibaca, dimuat, atau diverifikasi.
- Belum ada backend: capture mikrofon, resampling, VAD, ASR, CUDA/CPU, model download, shortcut global, tray, SendInput, single instance, config JSON, atau packaging DLL.
- Tidak ada pengukuran inferensi/VRAM/latensi aktual. Angka proses UI merupakan waktu simulasi.
- Clipboard eksplisit bekerja; flag pengecualian Windows Clipboard History/cloud belum diimplementasikan.
- Paket portable belum diuji pada mesin lain atau shell tanpa toolchain developer. Build release tahap ini merupakan pratinjau UI, bukan MVP dikte yang selesai.
- Tidak commit, push, atau publikasi. fi-shortener hanya dibaca dan font disalin beserta lisensinya.
