# Atribusi

- Token warna, gaya kartu, spacing, dan pengaturan font diadaptasi dari `fi-shortener/desktop/src/theme.rs`, dibaca sebagai referensi lokal. Tidak memakai logo fi-shortener.
- Inter Regular, Medium, SemiBold: Rasmus Andersson / Inter contributors, SIL Open Font License 1.1. Lisensi lengkap: [Inter-LICENSE.txt](../fonts/Inter-LICENSE.txt).
- JetBrains Mono Regular: JetBrains, SIL Open Font License 1.1. Lisensi lengkap: [JetBrainsMono-OFL.txt](../fonts/JetBrainsMono-OFL.txt).
- egui/eframe 0.37.0: MIT OR Apache-2.0, https://github.com/emilk/egui.
- rfd 0.17.2: MIT, https://github.com/PolyMeilex/rfd.

Uji model terpisah memakai CLI resmi [whisper.cpp b5454](https://github.com/ggml-org/whisper.cpp/releases/tag/b5454) (MIT), bobot Whisper large-v3-turbo (MIT), serta model [Silero VAD](https://github.com/snakers4/silero-vad) (MIT). Runtime uji dan library CUDA distribusi upstream disimpan lokal di `.local/`, tidak dibundel ke aplikasi UI atau dipublikasikan ulang. Lisensi/distribusi library NVIDIA perlu ditinjau saat membuat paket produk. Sampel FLEURS berlisensi CC BY 4.0: lihat [atribusi fixture](../fixtures/fleurs-id/ATTRIBUTION.md).

whisper-rs belum diintegrasikan. Source proyek memakai [lisensi MIT](../LICENSE), mengikuti lisensi yang sudah ditetapkan pada remote. Font dan dataset mengikuti lisensinya masing-masing.
