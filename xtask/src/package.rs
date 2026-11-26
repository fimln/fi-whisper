use std::fs;
use std::path::{Path, PathBuf};

use crate::cuda::{self, CUDA_RELEASE, CUDA_RUNTIME_DLLS};

pub fn run(root: &Path) -> Result<(), String> {
    let build_args = ["--locked".to_string(), "--release".to_string()];
    let code = cuda::run_cargo(root, "build", &build_args)?;
    if code != 0 {
        return Err(format!("Build CUDA gagal dengan kode {code}."));
    }

    let cuda = cuda::cuda_dir(root, CUDA_RELEASE);
    let dist = root.join("dist").join("fi-whisper");
    if dist.exists() {
        fs::remove_dir_all(&dist)
            .map_err(|e| format!("gagal membersihkan {}: {e}", dist.display()))?;
    }
    let licenses = dist.join("licenses");
    fs::create_dir_all(&licenses)
        .map_err(|e| format!("gagal membuat {}: {e}", licenses.display()))?;

    copy_file(
        &root.join("target").join("release").join("fi-whisper.exe"),
        &dist.join("fi-whisper.exe"),
    )?;
    copy_file(
        &root.join("assets").join("logo.ico"),
        &dist.join("logo.ico"),
    )?;

    let cuda_bin = cuda.join("bin").join("x64");
    for &dll in CUDA_RUNTIME_DLLS {
        copy_file(&cuda_bin.join(dll), &dist.join(dll))?;
    }

    copy_file(
        &cuda.join("LICENSE"),
        &licenses.join("NVIDIA-CUDA-EULA.txt"),
    )?;
    copy_file(&root.join("LICENSE"), &licenses.join("fi-whisper-MIT.txt"))?;
    copy_file(
        &root.join("assets").join("LICENSE-5mdt.txt"),
        &licenses.join("LICENSE-5mdt.txt"),
    )?;

    let fonts_dir = root.join("fonts");
    for entry in fs::read_dir(&fonts_dir)
        .map_err(|e| format!("gagal membaca {}: {e}", fonts_dir.display()))?
    {
        let entry = entry.map_err(|e| format!("gagal membaca entri fonts: {e}"))?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
            && let Some(name) = path.file_name()
        {
            copy_file(&path, &licenses.join(name))?;
        }
    }

    copy_file(&root.join("README.md"), &dist.join("README.md"))?;
    copy_file(
        &root.join("docs").join("CREDITS.md"),
        &dist.join("CREDITS.md"),
    )?;

    let mut files = Vec::new();
    collect_files(&dist, &dist, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    println!("{:<40} {:>8}", "File", "MB");
    println!("{:-<40} {:-<8}", "", "");
    for (rel, len) in files {
        let mb = len as f64 / (1024.0 * 1024.0);
        println!("{:<40} {:>8.1}", rel, mb);
    }
    Ok(())
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), String> {
    fs::copy(src, dst)
        .map(|_| ())
        .map_err(|e| format!("gagal menyalin {} ke {}: {e}", src.display(), dst.display()))
}

fn collect_files(base: &Path, dir: &Path, out: &mut Vec<(String, u64)>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("gagal membaca {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("gagal membaca entri: {e}"))?;
        let path: PathBuf = entry.path();
        if path.is_dir() {
            collect_files(base, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let len = entry
                .metadata()
                .map_err(|e| format!("gagal membaca metadata {}: {e}", path.display()))?
                .len();
            out.push((rel, len));
        }
    }
    Ok(())
}
