fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
        && std::env::var_os("CARGO_FEATURE_CUDA").is_some()
    {
        // cuBLAS saat launch dapat mengarahkan grafis Optimus ke GPU diskret, termasuk untuk Chirp.
        println!("cargo:rustc-link-lib=delayimp");
        println!("cargo:rustc-link-arg=/DELAYLOAD:cublas64_13.dll");
    }
}
