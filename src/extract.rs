use flate2::read::GzDecoder;
use std::fs;
use std::io::Read;
use std::path::Path;

pub fn extract_pkg(path: &str, dest: &str, _name: &str, _version: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let data = fs::read(path)?;
    if path.ends_with(".pkg.tar.zst") || path.ends_with(".zst") {
        let tar_bytes = zstd::stream::decode_all(&data[..])?;
        let root = Path::new(dest);
        fs::create_dir_all(root)?;
        let mut archive = tar::Archive::new(&tar_bytes[..]);
        archive.unpack(root)?;
    } else if path.ends_with(".apk") {
        let mut tar_bytes = Vec::new();
        GzDecoder::new(&data[..]).read_to_end(&mut tar_bytes)?;
        let root = Path::new(dest);
        fs::create_dir_all(root)?;
        let mut archive = tar::Archive::new(&tar_bytes[..]);
        archive.unpack(root)?;
    } else if path.ends_with(".deb") {
        extract_deb(&data, Path::new(dest))?;
    } else if path.ends_with(".zip") || path.ends_with(".tar.gz") || path.ends_with(".tgz") {
        if path.ends_with(".zip") {
            extract_zip(&data, Path::new(dest))?;
        } else {
            let mut tar_bytes = Vec::new();
            GzDecoder::new(&data[..]).read_to_end(&mut tar_bytes)?;
            let root = Path::new(dest);
            fs::create_dir_all(root)?;
            let mut archive = tar::Archive::new(&tar_bytes[..]);
            archive.unpack(root)?;
        }
    } else {
        return Err(format!("unknown package format: {}", path).into());
    }
    Ok(())
}

fn extract_zip(data: &[u8], dest: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data))?;
    fs::create_dir_all(dest)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let path = dest.join(entry.mangled_name());
        if entry.is_dir() {
            fs::create_dir_all(&path)?;
        } else {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = fs::File::create(&path)?;
            std::io::copy(&mut entry, &mut out)?;
        }
    }
    Ok(())
}

fn extract_deb(data: &[u8], dest: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut pos = 0;
    let magic = b"!<arch>\n";
    if data.get(..8) != Some(magic) {
        return Err("not a valid ar archive".into());
    }
    pos += 8;

    while pos + 60 <= data.len() {
        let header = &data[pos..pos+60];
        pos += 60;
        let name = String::from_utf8_lossy(&header[..16]).trim().to_string();
        let size_str = String::from_utf8_lossy(&header[48..58]).trim().to_string();
        let size: usize = size_str.parse().unwrap_or(0);
        let padded = size + (size % 2);
        if pos + padded > data.len() { break; }
        let content = &data[pos..pos+size];
        pos += padded;

        if name.starts_with("data.tar.") {
            if name.ends_with(".xz") {
                let mut tar_bytes = Vec::new();
                let mut decoder = xz2::read::XzDecoder::new(content);
                decoder.read_to_end(&mut tar_bytes)?;
                fs::create_dir_all(dest)?;
                let mut archive = tar::Archive::new(&tar_bytes[..]);
                archive.unpack(dest)?;
                return Ok(());
            } else if name.ends_with(".gz") {
                let mut tar_bytes = Vec::new();
                GzDecoder::new(content).read_to_end(&mut tar_bytes)?;
                fs::create_dir_all(dest)?;
                let mut archive = tar::Archive::new(&tar_bytes[..]);
                archive.unpack(dest)?;
                return Ok(());
            }
        }
    }
    Err("no data.tar found in .deb".into())
}
