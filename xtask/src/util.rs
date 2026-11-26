use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path};
use std::time::Duration;

use reqwest::blocking::Client;
use ring::digest::{Context, SHA256};
use zip::ZipArchive;

pub fn http_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("gagal membuat klien HTTP: {e}"))
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|e| format!("gagal membuka {}: {e}", path.display()))?;
    let mut ctx = Context::new(&SHA256);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("gagal membaca {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    let digest = ctx.finish();
    let mut hex = String::with_capacity(digest.as_ref().len() * 2);
    for b in digest.as_ref() {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{b:02x}");
    }
    Ok(hex)
}

pub fn assert_sha256(path: &Path, expected: &str) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("File tidak ditemukan: {}", path.display()));
    }
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!("SHA-256 tidak cocok: {}", path.display()));
    }
    Ok(())
}

pub fn download_verified(
    client: &Client,
    url: &str,
    dest: &Path,
    expected_sha256: &str,
    label: Option<&str>,
) -> Result<(), String> {
    if dest.is_file() && sha256_file(dest)?.eq_ignore_ascii_case(expected_sha256) {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("gagal membuat {}: {e}", parent.display()))?;
    }
    match label {
        Some(text) => println!("Mengunduh {text}"),
        None => println!("Mengunduh {url}"),
    }

    let part = dest.with_extension(match dest.extension().and_then(|s| s.to_str()) {
        Some(ext) => format!("{ext}.part"),
        None => "part".to_string(),
    });

    let mut resp = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("gagal mengunduh {url}: {e}"))?;

    let write_res = (|| -> Result<(), String> {
        let mut out =
            File::create(&part).map_err(|e| format!("gagal membuat {}: {e}", part.display()))?;
        io::copy(&mut resp, &mut out)
            .map_err(|e| format!("gagal menulis {}: {e}", part.display()))?;
        out.flush()
            .map_err(|e| format!("gagal flush {}: {e}", part.display()))?;
        Ok(())
    })();
    if let Err(err) = write_res {
        let _ = fs::remove_file(&part);
        return Err(err);
    }

    if let Err(err) = assert_sha256(&part, expected_sha256) {
        let _ = fs::remove_file(&part);
        return Err(err);
    }

    // Di Windows, fs::rename gagal jika file tujuan sudah ada.
    if dest.exists() {
        fs::remove_file(dest).map_err(|e| format!("gagal menimpa {}: {e}", dest.display()))?;
    }
    fs::rename(&part, dest).map_err(|e| {
        format!(
            "gagal memindahkan {} ke {}: {e}",
            part.display(),
            dest.display()
        )
    })?;
    Ok(())
}

pub fn extract_zip(archive_path: &Path, dest: &Path, strip_top_dir: bool) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|e| format!("gagal membuka arsip {}: {e}", archive_path.display()))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|e| format!("arsip ZIP tidak valid {}: {e}", archive_path.display()))?;
    fs::create_dir_all(dest).map_err(|e| format!("gagal membuat {}: {e}", dest.display()))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("gagal membaca entri ZIP #{i}: {e}"))?;
        let Some(enclosed) = entry.enclosed_name() else {
            return Err(format!("path tidak aman di arsip: {}", entry.name()));
        };
        let rel = if strip_top_dir {
            let mut comps = enclosed.components();
            match comps.next() {
                Some(Component::Normal(_)) => comps.as_path(),
                _ => continue,
            }
        } else {
            enclosed.as_path()
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&target)
                .map_err(|e| format!("gagal membuat {}: {e}", target.display()))?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("gagal membuat {}: {e}", parent.display()))?;
            }
            let mut out = File::create(&target)
                .map_err(|e| format!("gagal membuat {}: {e}", target.display()))?;
            io::copy(&mut entry, &mut out)
                .map_err(|e| format!("gagal mengekstrak {}: {e}", target.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    #[test]
    fn sha256_matches_known_vector() {
        let dir = std::env::temp_dir().join(format!("xtask-sha-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.txt");
        fs::write(&file, b"abc").unwrap();
        let hash = sha256_file(&file).unwrap();
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(assert_sha256(&file, &hash.to_uppercase()).is_ok());
        assert!(assert_sha256(&file, "0000").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_zip_strips_top_dir() {
        let dir = std::env::temp_dir().join(format!("xtask-zip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("sample.zip");
        {
            let f = File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            zw.start_file("outer-1.0/bin/tool.txt", SimpleFileOptions::default())
                .unwrap();
            zw.write_all(b"ok").unwrap();
            zw.finish().unwrap();
        }
        let out = dir.join("out");
        extract_zip(&zip_path, &out, true).unwrap();
        assert_eq!(fs::read_to_string(out.join("bin/tool.txt")).unwrap(), "ok");
        let _ = fs::remove_dir_all(&dir);
    }
}
