use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::util::{download_verified, extract_zip, http_client};

/// Versi default CUDA redistributable yang resmi mendukung MSVC 14.5x (VS 2026)
/// dan berjalan pada driver r580+ (minor version compatibility).
pub const CUDA_RELEASE: &str = "13.2.2";

/// cuBLAS dimuat saat dipakai (cublas64_13 memuat cublasLt64_13); cudart ditautkan statis.
/// Qwen3-ASR (candle) memuat NVRTC (+builtins) dan cuRAND secara dinamis saat runtime.
pub const CUDA_RUNTIME_DLLS: &[&str] = &[
    "cublas64_13.dll",
    "cublasLt64_13.dll",
    "nvrtc64_130_0.dll",
    "nvrtc-builtins64_132.dll",
    "curand64_10.dll",
];

// cuda_crt dan libnvvm terpisah dari cuda_nvcc sejak CUDA 13.
// cuda_nvrtc dan libcurand dibutuhkan candle (fitur `qwen-cuda`).
const CUDA_COMPONENTS: &[&str] = &[
    "cuda_nvcc",
    "cuda_crt",
    "libnvvm",
    "cuda_cudart",
    "cuda_cccl",
    "cuda_profiler_api",
    "libcublas",
    "cuda_nvrtc",
    "libcurand",
];

const CUDA_BASE_URL: &str = "https://developer.download.nvidia.com/compute/cuda/redist";

pub fn cuda_dir(root: &Path, release: &str) -> PathBuf {
    root.join(".local").join(format!("cuda-{release}"))
}

pub fn setup(root: &Path, args: &[String]) -> Result<(), String> {
    let release = parse_release_arg(args)?;
    let target = cuda_dir(root, &release);
    let downloads = root.join(".local").join("downloads");
    fs::create_dir_all(&downloads)
        .map_err(|e| format!("gagal membuat {}: {e}", downloads.display()))?;
    fs::create_dir_all(&target).map_err(|e| format!("gagal membuat {}: {e}", target.display()))?;

    let client = http_client()?;
    let manifest_url = format!("{CUDA_BASE_URL}/redistrib_{release}.json");
    let manifest: Value = client
        .get(&manifest_url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("gagal mengambil manifest CUDA {manifest_url}: {e}"))?
        .json()
        .map_err(|e| format!("manifest CUDA tidak valid: {e}"))?;

    for &name in CUDA_COMPONENTS {
        let Some(component) = manifest.get(name) else {
            continue;
        };
        let entry = component
            .get("windows-x86_64")
            .ok_or_else(|| format!("entri windows-x86_64 tidak ada untuk {name}"))?;
        let rel_path = entry["relative_path"]
            .as_str()
            .ok_or_else(|| format!("relative_path hilang untuk {name}"))?;
        let sha256 = entry["sha256"]
            .as_str()
            .ok_or_else(|| format!("sha256 hilang untuk {name}"))?;
        let version = component["version"].as_str().unwrap_or("");
        let file_name = rel_path
            .rsplit('/')
            .next()
            .ok_or_else(|| format!("nama file tidak valid: {rel_path}"))?;
        let zip_path = downloads.join(file_name);
        let url = format!("{CUDA_BASE_URL}/{rel_path}");
        let label = format!("{name} {version}");

        download_verified(&client, &url, &zip_path, sha256, Some(&label))?;
        // Arsip berisi satu folder <name>-windows-x86_64-<versi>-archive; gabungkan isinya.
        extract_zip(&zip_path, &target, true)?;
        println!("{name} {version} {sha256}");
    }

    let nvcc = target.join("bin").join("nvcc.exe");
    let status = Command::new(&nvcc)
        .arg("--version")
        .status()
        .map_err(|e| format!("gagal menjalankan {}: {e}", nvcc.display()))?;
    if !status.success() {
        return Err(format!("nvcc --version gagal dengan kode {status}"));
    }
    println!("CUDA {release} siap di {}", target.display());
    Ok(())
}

pub fn run_cargo(root: &Path, subcmd: &str, args: &[String]) -> Result<i32, String> {
    let cuda = cuda_dir(root, CUDA_RELEASE);
    let nvcc = cuda.join("bin").join("nvcc.exe");
    if !nvcc.is_file() {
        return Err("CUDA belum ada. Jalankan `cargo xtask setup-cuda`.".to_string());
    }

    let pf86 =
        env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".to_string());
    let vs_installer = PathBuf::from(&pf86)
        .join("Microsoft Visual Studio")
        .join("Installer");
    let vswhere = vs_installer.join("vswhere.exe");
    let vs_out = Command::new(&vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .map_err(|e| format!("gagal menjalankan {}: {e}", vswhere.display()))?;
    if !vs_out.status.success() {
        return Err("vswhere.exe gagal mendeteksi Visual Studio.".to_string());
    }
    let vs_path = String::from_utf8_lossy(&vs_out.stdout).trim().to_string();
    if vs_path.is_empty() {
        return Err("Visual Studio C++ Build Tools tidak ditemukan.".to_string());
    }

    // Muat environment MSVC x64 (cl.exe, link.exe, SDK) untuk generator Ninja.
    // cmd.exe tidak memakai aturan escape backslash seperti CRT, jadi pakai raw_arg.
    let vcvars = PathBuf::from(&vs_path)
        .join("VC")
        .join("Auxiliary")
        .join("Build")
        .join("vcvars64.bat");
    let vcvars_cmd = format!("\"\"{}\" >nul && set\"", vcvars.display());
    let env_out = Command::new("cmd")
        .arg("/c")
        .raw_arg(&vcvars_cmd)
        .output()
        .map_err(|e| format!("gagal memuat vcvars64.bat: {e}"))?;
    if !env_out.status.success() {
        return Err(format!("vcvars64.bat gagal dengan kode {}", env_out.status));
    }

    let mut env_map = parse_env_block(&String::from_utf8_lossy(&env_out.stdout));
    let ninja = PathBuf::from(&vs_path)
        .join("Common7")
        .join("IDE")
        .join("CommonExtensions")
        .join("Microsoft")
        .join("CMake")
        .join("Ninja");
    let existing_path = env_map
        .get("PATH")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let new_path = format!(
        "{};{};{};{};{}",
        vs_installer.display(),
        cuda.join("bin").display(),
        cuda.join("bin").join("x64").display(),
        ninja.display(),
        existing_path
    );
    set_env(&mut env_map, "PATH", new_path);
    set_env(&mut env_map, "CUDA_PATH", cuda.display().to_string());

    let libclang = env::var("LIBCLANG_PATH")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| r"C:\Program Files\LLVM\bin".to_string());
    set_env(&mut env_map, "LIBCLANG_PATH", libclang);
    // Generator Ninja tidak membutuhkan integrasi MSBuild CUDA di instalasi Visual Studio.
    set_env(&mut env_map, "CMAKE_GENERATOR", "Ninja".to_string());
    set_env(
        &mut env_map,
        "CMAKE_CUDA_COMPILER",
        nvcc.display().to_string().replace('\\', "/"),
    );
    set_env(&mut env_map, "CMAKE_CUDA_ARCHITECTURES", "120".to_string());
    // Arsitektur kernel candle (fitur `qwen-cuda`), disamakan dengan whisper.cpp.
    set_env(&mut env_map, "CUDA_COMPUTE_CAP", "120".to_string());
    // CCCL CUDA 13 menolak preprocessor tradisional MSVC; bindgen_cuda (candle) tidak
    // menambahkan flag ini sendiri. nvcc membaca NVCC_APPEND_FLAGS untuk setiap pemanggilan.
    set_env(
        &mut env_map,
        "NVCC_APPEND_FLAGS",
        "-Xcompiler=/Zc:preprocessor".to_string(),
    );

    // --features diletakkan sebelum argumen lain agar tidak jatuh setelah `--`.
    let mut cargo = Command::new("cargo");
    cargo
        .current_dir(root)
        .env_clear()
        .envs(env_map.values().map(|(k, v)| (k.as_str(), v.as_str())))
        .arg(subcmd)
        .args(["--features", "cuda,qwen-cuda"])
        .args(args);

    let status = cargo
        .status()
        .map_err(|e| format!("gagal menjalankan cargo {subcmd}: {e}"))?;
    let code = status.code().unwrap_or(1);

    if code == 0 {
        let profile = detect_profile(args);
        let out_dir = root.join("target").join(profile);
        if out_dir.is_dir() {
            sync_cuda_dlls(&cuda, &out_dir)?;
        }
    }

    Ok(code)
}

fn parse_release_arg(args: &[String]) -> Result<String, String> {
    match args {
        [] => Ok(CUDA_RELEASE.to_string()),
        [flag, value] if flag == "--release" => Ok(value.clone()),
        [value] if !value.starts_with('-') => Ok(value.clone()),
        _ => Err("argumen setup-cuda: [--release <versi>]".to_string()),
    }
}

fn parse_env_block(text: &str) -> BTreeMap<String, (String, String)> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some((k, v)) = line.split_once('=')
            && !k.is_empty()
        {
            map.insert(k.to_ascii_uppercase(), (k.to_string(), v.to_string()));
        }
    }
    map
}

fn set_env(map: &mut BTreeMap<String, (String, String)>, key: &str, value: String) {
    let upper = key.to_ascii_uppercase();
    let original_key = map
        .get(&upper)
        .map(|(k, _)| k.clone())
        .unwrap_or_else(|| key.to_string());
    map.insert(upper, (original_key, value));
}

fn detect_profile(args: &[String]) -> &'static str {
    for arg in args {
        if arg == "--" {
            break;
        }
        if arg == "--release" || arg == "-r" {
            return "release";
        }
    }
    "debug"
}

fn sync_cuda_dlls(cuda: &Path, out_dir: &Path) -> Result<(), String> {
    // Exe CUDA mengimpor cuBLAS secara dinamis; salin DLL ke samping exe agar bisa dijalankan
    // langsung dari target\<profil> tanpa CUDA di PATH. Hanya salin bila ukuran/waktu berubah.
    let bin_dir = cuda.join("bin").join("x64");
    for &dll in CUDA_RUNTIME_DLLS {
        let src = bin_dir.join(dll);
        let dst = out_dir.join(dll);
        let src_meta =
            fs::metadata(&src).map_err(|e| format!("gagal membaca {}: {e}", src.display()))?;
        let unchanged = fs::metadata(&dst)
            .map(|old| {
                old.len() == src_meta.len() && old.modified().ok() == src_meta.modified().ok()
            })
            .unwrap_or(false);
        if !unchanged {
            fs::copy(&src, &dst).map_err(|e| {
                format!("gagal menyalin {} ke {}: {e}", src.display(), dst.display())
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_ignores_flags_after_separator() {
        let args = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(detect_profile(&args(&["--locked", "--release"])), "release");
        assert_eq!(detect_profile(&args(&["-r"])), "release");
        assert_eq!(detect_profile(&args(&["--", "--release"])), "debug");
        assert_eq!(detect_profile(&args(&["--locked"])), "debug");
    }

    #[test]
    fn env_block_deduplicates_case_insensitively() {
        let mut map = parse_env_block("Path=C:\\Windows\r\nTEMP=C:\\Temp\r\n");
        set_env(&mut map, "PATH", "C:\\Cuda;C:\\Windows".to_string());
        assert_eq!(map.len(), 2);
        assert_eq!(map["PATH"].1, "C:\\Cuda;C:\\Windows");
    }
}
