# Validasi tahap UI

Tanggal: 18 Oktober 2026, Windows x64. Cakupan: UI Rust egui, tanpa backend dikte.

Uji model terpisah dilakukan setelah tahap UI: lihat [MODEL-TEST.md](MODEL-TEST.md). Pernyataan belum diuji di bawah merupakan batas sesi UI, bukan pembatalan hasil uji CLI tersebut.

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
