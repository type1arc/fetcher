use crate::extract::extract_pkg;
use crate::postinstall::{self, make_executable, strip_binaries};
use crate::repo::FoundPkg;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::fs;
use std::path::Path;
use std::process::Command;

fn install_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".fetcher".to_string());
    format!("{}/.local/bin", home)
}

fn cache_dir() -> String {
    ".fetcher".to_string()
}

pub async fn download_pkg(
    client: &Client,
    pkg: &FoundPkg,
    expected_sha256: Option<&str>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let dl_sty = ProgressStyle::default_bar()
        .template("[{bar:20}] {bytes}/{total_bytes} {msg}")
        .unwrap()
        .progress_chars("##-");
    println!("→ downloading {} from {}", pkg.name, pkg.repo);
    let response = client.get(&pkg.download_url).header("user-agent", "fetcher").send().await?;
    if !response.status().is_success() {
        return Err(format!("HTTP {} downloading {}", response.status(), pkg.download_url).into());
    }

    let total = response.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total);
    pb.set_style(dl_sty);
    pb.set_message(format!("downloading {}", pkg.name));

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes.extend_from_slice(&chunk);
        pb.inc(chunk.len() as u64);
    }

    pb.finish_with_message(format!("✓ downloaded {} ({} bytes)", pkg.name, bytes.len()));

    let cache = cache_dir();
    fs::create_dir_all(&cache)?;
    let ext = pkg.download_url.rsplit_once('.').map(|(_, e)| e).unwrap_or("pkg");
    let path = format!("{}/{}-{}.{}", cache, pkg.name, pkg.version, ext);
    fs::write(&path, &bytes)?;

    // checksum verification
    if let Some(expected) = expected_sha256 {
        postinstall::verify_checksum(&path, expected)?;
    }

    let dest = install_dir();
    fs::create_dir_all(&dest)?;
    println!("→ extracting {} to {}", pkg.name, dest);
    match extract_pkg(&path, &dest, &pkg.name, &pkg.version) {
        Ok(_) => {
            println!("✓ extracted {}", pkg.name);
            let _ = fs::remove_file(&path);
            let n = make_executable(&dest);
            if n > 0 { println!("  made {} file(s) executable", n); }
            let m = strip_binaries(&dest);
            if m > 0 { println!("  stripped {} binary(ies)", m); }
            Ok(dest)
        }
        Err(e) => {
            eprintln!("✗ extraction failed for {}: {}", pkg.name, e);
            Err(e)
        }
    }
}

pub async fn fetch_pkg(
    client: &Client,
    name: &str,
    version: &str,
    url: &str,
    expected_sha256: Option<&str>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let dl_sty = ProgressStyle::default_bar()
        .template("[{bar:20}] {bytes}/{total_bytes} {msg}")
        .unwrap()
        .progress_chars("##-");

    println!("→ downloading {}@{} from {}", name, version, url);
    let response = client.get(url).header("user-agent", "fetcher").send().await?;
    if !response.status().is_success() {
        return Err(format!("failed to fetch {}@{} ({}): HTTP {}", name, version, url, response.status()).into());
    }

    let total = response.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total);
    pb.set_style(dl_sty);
    pb.set_message(format!("downloading {}@{}", name, version));

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes.extend_from_slice(&chunk);
        pb.inc(chunk.len() as u64);
    }
    pb.finish_with_message(format!("✓ downloaded {}@{} ({} bytes)", name, version, bytes.len()));

    let cache = cache_dir();
    fs::create_dir_all(&cache)?;
    let ext = if url.ends_with(".tar.gz") { "tar.gz" }
              else { url.rsplit_once('.').map(|(_, e)| e).unwrap_or("zip") };
    let path = format!("{}/{}-{}.{}", cache, name, version, ext);
    fs::write(&path, &bytes)?;

    // checksum verification
    if let Some(expected) = expected_sha256 {
        postinstall::verify_checksum(&path, expected)?;
    }

    let dest = install_dir();
    fs::create_dir_all(&dest)?;
    println!("→ extracting {} to {}", name, dest);
    match extract_pkg(&path, &dest, name, version) {
        Ok(_) => {
            println!("✓ extracted {}", name);
            let _ = fs::remove_file(&path);
            let n = make_executable(&dest);
            if n > 0 { println!("  made {} file(s) executable", n); }
            let m = strip_binaries(&dest);
            if m > 0 { println!("  stripped {} binary(ies)", m); }
            Ok(dest)
        }
        Err(e) => {
            eprintln!("✗ extraction failed for {}: {}", name, e);
            Err(e)
        }
    }
}

fn source_root(build_dir: &str, _name: &str, _version: &str) -> String {
    // GitHub archives wrap in a top-level dir like `repo-name-version/`
    let root = Path::new(build_dir);
    let entries: Vec<_> = std::fs::read_dir(root).ok()
        .into_iter().flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    if entries.len() == 1 {
        entries[0].path().to_string_lossy().to_string()
    } else {
        build_dir.to_string()
    }
}

fn detect_build_system(dir: &str, name: &str) -> Option<String> {
    let check = |p: &str| Path::new(p).exists();
    let src = source_root(dir, name, "");

    // Cargo
    let cargo = format!("{}/Cargo.toml", src);
    if check(&cargo) {
        return Some(format!(
            "cargo install --path . --root \"$INSTALL_DIR/..\""
        ));
    }

    // Go
    if check(&format!("{}/go.mod", src)) {
        return Some(format!("go build -o \"$INSTALL_DIR/{}\" .", name));
    }

    // CMake
    if check(&format!("{}/CMakeLists.txt", src)) {
        return Some(format!(
            "cmake -B build -DCMAKE_INSTALL_PREFIX=\"$INSTALL_DIR\" && \
             cmake --build build && cmake --install build"
        ));
    }

    // Meson
    if check(&format!("{}/meson.build", src)) {
        return Some(format!(
            "meson setup build --prefix=\"$INSTALL_DIR\" && \
             meson compile -C build && meson install -C build"
        ));
    }

    // Make (checked after CMake/Meson so those take priority)
    if check(&format!("{}/Makefile", src)) || check(&format!("{}/makefile", src)) {
        return Some(format!("make && make install PREFIX=\"$INSTALL_DIR\""));
    }

    // Autotools
    if check(&format!("{}/configure", src)) {
        return Some(format!(
            "./configure --prefix=\"$INSTALL_DIR\" && make && make install"
        ));
    }

    // Python setup.py
    if check(&format!("{}/setup.py", src)) {
        return Some(format!("python setup.py install --prefix=\"$INSTALL_DIR\""));
    }

    // Python pyproject.toml
    if check(&format!("{}/pyproject.toml", src)) {
        return Some(format!("pip install --prefix=\"$INSTALL_DIR\" ."));
    }

    None
}

/// Download source archive and build it, installing results into ~/.local/bin.
/// When `build_cmd` is empty, auto-detect the build system.
pub async fn build_from_source(
    client: &Client,
    name: &str,
    version: &str,
    url: &str,
    build_cmd: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let dl_sty = ProgressStyle::default_bar()
        .template("[{bar:20}] {bytes}/{total_bytes} {msg}")
        .unwrap()
        .progress_chars("##-");

    println!("→ downloading source {}@{} from {}", name, version, url);
    let response = client.get(url).header("user-agent", "fetcher").send().await?;
    if !response.status().is_success() {
        return Err(format!("HTTP {} fetching source {}", response.status(), url).into());
    }

    let total = response.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total);
    pb.set_style(dl_sty);
    pb.set_message(format!("downloading source {}", name));

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes.extend_from_slice(&chunk);
        pb.inc(chunk.len() as u64);
    }
    pb.finish_with_message(format!("✓ downloaded source {} ({} bytes)", name, bytes.len()));

    let cache = cache_dir();
    fs::create_dir_all(&cache)?;
    let ext = if url.ends_with(".tar.gz") { "tar.gz" }
              else { url.rsplit_once('.').map(|(_, e)| e).unwrap_or("zip") };
    let archive_path = format!("{}/{}-{}.{}", cache, name, version, ext);
    fs::write(&archive_path, &bytes)?;

    let build_dir = format!("{}/builds/{}-{}", cache, name, version);
    let _ = fs::remove_dir_all(&build_dir);
    fs::create_dir_all(&build_dir)?;

    println!("→ extracting source to {}", build_dir);
    if let Err(e) = extract_pkg(&archive_path, &build_dir, name, version) {
        let _ = fs::remove_file(&archive_path);
        return Err(format!("failed to extract source: {}", e).into());
    }
    let _ = fs::remove_file(&archive_path);

    // resolve source root (handle GitHub's nested top-level dir)
    let src_root = source_root(&build_dir, name, version);

    // auto-detect build system if no explicit command given
    let cmd = if build_cmd.is_empty() {
        match detect_build_system(&build_dir, name) {
            Some(c) => {
                println!("→ detected build system: {}", c.split_whitespace().next().unwrap_or("?"));
                c
            }
            None => return Err("no build command specified and no build system detected (try --build)".into()),
        }
    } else {
        build_cmd.to_string()
    };

    let install = install_dir();
    fs::create_dir_all(&install)?;
    println!("→ building {}...", name);
    let status = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .current_dir(&src_root)
        .env("INSTALL_DIR", &install)
        .env("PKG_NAME", name)
        .env("PKG_VERSION", version)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()?;
    if !status.success() {
        let _ = fs::remove_dir_all(&build_dir);
        return Err(format!("build for {} failed with exit code {:?}", name, status.code()).into());
    }

    println!("✓ built {}", name);
    let _ = fs::remove_dir_all(&build_dir);
    let n = make_executable(&install);
    if n > 0 { println!("  made {} file(s) executable", n); }
    let m = strip_binaries(&install);
    if m > 0 { println!("  stripped {} binary(ies)", m); }
    Ok(install)
}

pub async fn git_clone(name: &str, version: &str, git_url: &str, rev: Option<&str>) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let dest = install_dir();
    let cache_dir = Path::new(&dest).join(format!("{name}-{version}"));
    if cache_dir.exists() { fs::remove_dir_all(&cache_dir)?; }
    println!("→ cloning {}@{} from {}...", name, version, git_url);
    let mut cmd = Command::new("git");
    cmd.arg("clone");
    if let Some(r) = rev { cmd.arg("--branch").arg(r); }
    cmd.arg(git_url).arg(&cache_dir);
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());
    if !cmd.status()?.success() { return Err(format!("git clone failed for {name}").into()); }
    println!("✓ cloned {}@{} -> {:?}", name, version, cache_dir);
    Ok(dest)
}

pub fn clean_cache() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cache = cache_dir();
    if Path::new(&cache).exists() {
        fs::remove_dir_all(&cache)?;
        println!("✓ removed {}/", cache);
    } else {
        println!("nothing to clean");
    }
    Ok(())
}
