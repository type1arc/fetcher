use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn make_executable(dir: &str) -> u32 {
    let root = Path::new(dir);
    if !root.exists() { return 0; }
    let Ok(entries) = fs::read_dir(root) else { return 0 };
    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let Ok(meta) = fs::metadata(&path) else { continue };
            let mut perm = meta.permissions();
            let mode = perm.mode();
            if mode & 0o111 != 0 { continue; }
            perm.set_mode(mode | 0o111);
            let _ = fs::set_permissions(&path, perm);
            count += 1;
        }
    }
    count
}

pub fn strip_binaries(dir: &str) -> u32 {
    let root = Path::new(dir);
    if !root.exists() { return 0; }
    let Ok(entries) = fs::read_dir(root) else { return 0 };
    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let Ok(data) = fs::read(&path) else { continue };
            if data.len() < 4 { continue; }
            if &data[..4] == b"\x7fELF" {
                let _ = Command::new("strip").arg(&path).output();
                count += 1;
            }
        }
    }
    count
}

fn shell_rc() -> Option<String> {
    let shell = std::env::var("SHELL").ok()?;
    let home = std::env::var("HOME").ok()?;
    let name = Path::new(&shell).file_name()?.to_str()?;
    match name {
        "zsh" => Some(format!("{}/.zshrc", home)),
        "fish" => Some(format!("{}/.config/fish/config.fish", home)),
        "bash" => {
            let p = format!("{}/.bashrc", home);
            if Path::new(&p).exists() { Some(p) }
            else { Some(format!("{}/.bash_profile", home)) }
        }
        _ => Some(format!("{}/.profile", home)),
    }
}

pub fn check_path() {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return,
    };
    let bin = format!("{}/.local/bin", home);
    let already = std::env::var("PATH")
        .map(|p| p.split(':').any(|x| x == bin))
        .unwrap_or(false);

    if already {
        println!("✓ {} is in PATH", bin);
        return;
    }

    // add to current process PATH
    if let Ok(cur) = std::env::var("PATH") {
        let new = format!("{}:{}", cur, bin);
        std::env::set_var("PATH", &new);
    }
    println!("✓ added {} to PATH for this session", bin);

    // persist in shell RC
    if let Some(rc) = shell_rc() {
        let line = format!("\nexport PATH=\"$PATH:{}\"", bin);
        if let Ok(content) = fs::read_to_string(&rc) {
            if content.contains(&line.trim().to_string()) {
                return;
            }
        }
        if let Ok(mut f) = fs::OpenOptions::new().append(true).open(&rc) {
            use std::io::Write;
            let _ = writeln!(f, "{}", line.trim());
            println!("✓ added to {} (restart shell or source it)", rc);
        }
    }
}

pub fn run_script(script: &str, name: &str, install_dir: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("▶ running post-install script for {}...", name);
    let status = Command::new("sh")
        .arg("-c")
        .arg(script)
        .env("INSTALL_DIR", install_dir)
        .env("PKG_NAME", name)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err(format!("script for {} exited with code {:?}", name, status.code()).into());
    }
    println!("✓ post-install script for {} completed", name);
    Ok(())
}

pub fn compute_sha256(path: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let out = Command::new("sha256sum")
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("sha256sum failed: {}", stderr.trim()).into());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let hash = stdout.split_whitespace().next().unwrap_or("").to_string();
    if hash.is_empty() {
        Err("could not parse sha256sum output".into())
    } else {
        Ok(hash)
    }
}

pub fn verify_checksum(path: &str, expected: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("🔍 verifying checksum...");
    let actual = compute_sha256(path)?;
    if actual != expected {
        return Err(format!(
            "checksum mismatch for {}\n  expected: {}\n  actual:   {}",
            path, expected, actual
        ).into());
    }
    println!("✓ checksum verified ({})", &actual[..16]);
    Ok(())
}
