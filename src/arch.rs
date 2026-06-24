use std::process::Command;

pub fn native_arch() -> String {
    let out = Command::new("uname").arg("-m").output()
        .ok().and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default();
    match out.trim() {
        "x86_64" => "amd64".to_string(),
        "aarch64" => "arm64".to_string(),
        "i686" | "i386" => "i386".to_string(),
        a => a.to_string(),
    }
}

pub fn pacman_arch() -> String {
    Command::new("uname").arg("-m").output()
        .ok().and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "x86_64".to_string())
}
