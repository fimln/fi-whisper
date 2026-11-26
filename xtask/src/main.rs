mod cuda;
mod logo;
mod package;
mod test_model;
mod util;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = repo_root();
    match dispatch(&root, &args) {
        Ok(code) => ExitCode::from((code & 0xFF) as u8),
        Err(err) => {
            eprintln!("Galat: {err}");
            ExitCode::FAILURE
        }
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask berada di bawah root repositori")
        .to_path_buf()
}

fn dispatch(root: &Path, args: &[String]) -> Result<i32, String> {
    let Some((cmd, rest)) = args.split_first() else {
        print_usage();
        return Ok(0);
    };
    match cmd.as_str() {
        "setup-cuda" => {
            cuda::setup(root, rest)?;
            Ok(0)
        }
        "build" | "clippy" | "test" | "check" | "run" => cuda::run_cargo(root, cmd, rest),
        "cuda" => {
            if let Some((subcmd, subrest)) = rest.split_first() {
                cuda::run_cargo(root, subcmd, subrest)
            } else {
                let default_args = ["--locked".to_string(), "--release".to_string()];
                cuda::run_cargo(root, "build", &default_args)
            }
        }
        "package" => {
            package::run(root)?;
            Ok(0)
        }
        "test-model" => {
            test_model::run(root, rest)?;
            Ok(0)
        }
        "make-logo" => {
            logo::run(root)?;
            Ok(0)
        }
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(0)
        }
        other => Err(format!(
            "perintah tidak dikenal: `{other}`. Jalankan `cargo xtask --help`."
        )),
    }
}

fn print_usage() {
    println!(
        "\
Pemakaian: cargo xtask <perintah> [argumen...]

Perintah:
  setup-cuda [--release <versi>]    Unduh redistributable CUDA ke .local/cuda-<versi>
  build|clippy|test|check|run ...   Jalankan cargo dengan fitur cuda,qwen-cuda
  cuda [<subcmd> ...]               Jalankan subcommand cargo lain di lingkungan CUDA
  package                           Bangun rilis CUDA dan kemas ke dist/fi-whisper
  test-model [--model-dir <dir>] [--cpu] [--self-test]
                                    Uji model Whisper + Silero lewat whisper-cli b5454
  make-logo                         Render ulang assets/logo.* dan ikon Octicons RGBA"
    );
}
