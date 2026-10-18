# Atribusi

- Token warna, gaya kartu, spacing, dan pengaturan font diadaptasi dari `fi-shortener/desktop/src/theme.rs`, dibaca sebagai referensi lokal. Logo fi-whisper mengikuti bentuk logo fi-shortener dengan palet amber, dibuat oleh `scripts/make-logo.py`.
- Ikon GitHub (`mark-github-24`) dan beranda (`home-fill-24`): [Primer Octicons](https://github.com/primer/octicons), GitHub Inc., MIT ([lisensi](../assets/icons/OCTICONS-LICENSE.txt)). Logo GitHub hanya dipakai sebagai tautan ke repo.
- Inter Regular, Medium, SemiBold: Rasmus Andersson / Inter contributors, SIL Open Font License 1.1. Lisensi lengkap: [Inter-LICENSE.txt](../fonts/Inter-LICENSE.txt).
- JetBrains Mono Regular: JetBrains, SIL Open Font License 1.1. Lisensi lengkap: [JetBrainsMono-OFL.txt](../fonts/JetBrainsMono-OFL.txt).
- egui/eframe 0.37.0: MIT OR Apache-2.0, https://github.com/emilk/egui.
- rfd 0.17.2: MIT, https://github.com/PolyMeilex/rfd.
- whisper-rs 0.16.0 / whisper-rs-sys 0.15.0 (Unlicense, https://codeberg.org/tazz4843/whisper-rs), membundel whisper.cpp 1.8.3 dan ggml (MIT) yang ditautkan statis ke exe.
- cpal 0.18.2 (Apache-2.0), rubato 5.0.1 (MIT), rtrb 0.4.0 (MIT OR Apache-2.0), global-hotkey 0.9.0 dan tray-icon 0.26.1 (MIT OR Apache-2.0), windows-sys 0.61.2 (MIT OR Apache-2.0), serde/serde_json (MIT OR Apache-2.0).
- qwen3-asr 0.2.2 (MIT, https://github.com/alan890104/qwen3-asr-rs), disalin ke `vendor/qwen3-asr` dengan patch fi-whisper (layout checkpoint `*-hf`, pembatalan, dan validasi `config.json`); memakai candle 0.9.2 (MIT OR Apache-2.0) dan tokenizers 0.22 (Apache-2.0).
- Bobot [Qwen/Qwen3-ASR-0.6B-hf](https://huggingface.co/Qwen/Qwen3-ASR-0.6B-hf) dan [Qwen/Qwen3-ASR-1.7B-hf](https://huggingface.co/Qwen/Qwen3-ASR-1.7B-hf): Alibaba Qwen, Apache-2.0. Tidak dibundel.
- webbrowser 1.2.4 (MIT OR Apache-2.0), lewat fitur `links` eframe untuk membuka tautan header.

Uji model terpisah memakai CLI resmi [whisper.cpp b5454](https://github.com/ggml-org/whisper.cpp/releases/tag/b5454) (MIT), bobot Whisper large-v3-turbo (MIT), serta model [Silero VAD](https://github.com/snakers4/silero-vad) (MIT). Runtime uji dan library CUDA distribusi upstream disimpan lokal di `.local/`, tidak dibundel ke aplikasi UI atau dipublikasikan ulang. Lisensi/distribusi library NVIDIA perlu ditinjau saat membuat paket produk. Sampel FLEURS berlisensi CC BY 4.0: lihat [atribusi fixture](../fixtures/fleurs-id/ATTRIBUTION.md).

Model tidak dibundel; pengguna memilih file Whisper/Silero dan folder Qwen3-ASR sendiri. Paket CUDA menyertakan `cublas64_13.dll`, `cublasLt64_13.dll`, `nvrtc64_130_0.dll`, `nvrtc-builtins64_132.dll`, dan `curand64_10.dll` dari CUDA 13.2.2 (redistributable di bawah NVIDIA CUDA EULA, disalin ke `licenses/NVIDIA-CUDA-EULA.txt`); cudart ditautkan statis. Source proyek memakai [lisensi MIT](../LICENSE), mengikuti lisensi yang sudah ditetapkan pada remote. Font dan dataset mengikuti lisensinya masing-masing.
