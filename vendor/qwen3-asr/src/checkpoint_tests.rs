use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "qwen-checkpoint-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn save(path: &Path, name: &str, dtype: DType) {
    let tensor = Tensor::new(&[[1.0f32, -2.0], [3.0, 4.0]], &Device::Cpu)
        .unwrap()
        .to_dtype(dtype)
        .unwrap();
    candle_core::safetensors::save(&HashMap::from([(name, tensor)]), path).unwrap();
}

#[test]
fn checkpoint_values_survive_unmapping_and_file_removal() {
    let dir = TestDir::new();
    let path = dir.0.join("model.safetensors");
    for dtype in [DType::F32, DType::F16, DType::BF16] {
        save(&path, "w", dtype);
        let weights = load_weights(&dir.0, &Device::Cpu).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(weights["w"].dtype(), dtype);
        assert_eq!(weights["w"].dims(), &[2, 2]);
        assert_eq!(
            weights["w"]
                .to_dtype(DType::F32)
                .unwrap()
                .to_vec2::<f32>()
                .unwrap(),
            vec![vec![1.0, -2.0], vec![3.0, 4.0]]
        );
    }
}

#[test]
fn checkpoint_shards_and_invalid_files() {
    let dir = TestDir::new();
    let first = dir.0.join("first.safetensors");
    let second = dir.0.join("second.safetensors");
    save(&first, "a", DType::F32);
    save(&second, "b", DType::BF16);
    std::fs::write(
        dir.0.join("model.safetensors.index.json"),
        r#"{"weight_map":{"a":"first.safetensors","b":"second.safetensors"}}"#,
    )
    .unwrap();
    let weights = load_weights(&dir.0, &Device::Cpu).unwrap();
    assert_eq!(weights.len(), 2);
    assert_eq!(weights["a"].dtype(), DType::F32);
    assert_eq!(weights["b"].dtype(), DType::BF16);
    let bytes = std::fs::read(&second).unwrap();
    for bad in [&b""[..], &b"invalid"[..], &bytes[..bytes.len() - 1]] {
        std::fs::write(&second, bad).unwrap();
        assert!(load_weights(&dir.0, &Device::Cpu).is_err());
    }
    std::fs::remove_file(&second).unwrap();
    assert!(load_weights(&dir.0, &Device::Cpu).is_err());
}

#[cfg(windows)]
#[test]
fn checkpoint_rejects_existing_writer_and_writable_mapping() {
    let dir = TestDir::new();
    let path = dir.0.join("model.safetensors");
    save(&path, "w", DType::F32);
    let writer = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    assert!(load_checkpoint(&path, &Device::Cpu).is_err());
    // SAFETY: this test owns the file and does not mutate it while the view exists.
    let mapping = unsafe { memmap2::MmapMut::map_mut(&writer).unwrap() };
    drop(writer);
    assert!(load_checkpoint(&path, &Device::Cpu).is_err());
    drop(mapping);
    assert!(load_checkpoint(&path, &Device::Cpu).is_ok());
}
