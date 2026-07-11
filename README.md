<p align="center">
  <img src="assets/fetcher.png" alt="fetcher" />
</p>

# fetcher

A configurable, cross-platform package fetcher written in Rust. Downloads and installs software from system repositories (APT, Pacman, APK), GitHub releases, direct URLs, Git repos, and auto-detected source builds.

## Installation

```bash
cargo build --release
# binary at target/release/fetcher
```

**Arch Linux (AUR):**

```bash
yay -S fetcher-cli
```

**Runtime dependencies:** `git`, `sha256sum`, `strip`, `sh` (POSIX shell)

## Usage

```
fetcher fetch [PKG] [OPTIONS]   # install a package (or manifest mode)
fetcher source <src>            # download source and build
fetcher clean                   # remove cached .fetcher/ directory
fetcher unfetch [PKG]           # uninstall a package
```

### Fetch a single package

```bash
# From system repos (searches APT, Pacman, APK in parallel)
fetcher fetch neovim

# Specific version
fetcher fetch neovim@0.10.0

# From GitHub releases
fetcher fetch tokio --github tokio-rs/tokio

# From a direct URL
fetcher fetch mylib --url "https://example.com/downloads/mylib-{version}.zip"

# From a Git repo
fetcher fetch mylib --git "https://github.com/myorg/mylib.git" --rev main

# Build from source (auto-detects Cargo, Go, CMake, Meson, Make, etc.)
fetcher fetch ripgrep --from-source --github BurntSushi/ripgrep
```

Placeholders `{name}` and `{version}` are substituted automatically in URLs.

### Source command

```bash
fetcher source BurntSushi/ripgrep
fetcher source BurntSushi/ripgrep --version 14.1.0
fetcher source "https://example.com/src/pkg-1.0.tar.gz"
fetcher source myorg/myapp --build "cmake -B build && cmake --build build && cp build/myapp \$INSTALL_DIR/"
```

Build environment variables: `INSTALL_DIR`, `PKG_NAME`, `PKG_VERSION`.

### Global options

| Flag | Description |
|------|-------------|
| `--github <owner/repo>` | Download from GitHub releases archive |
| `--url <URL>` | Download from an arbitrary URL |
| `--git <URL>` | Clone a Git repository |
| `--rev <ref>` | Branch, tag, or commit for `--git` sources |
| `--build <cmd>` | Build from source using a shell command |
| `--from-source` | Skip system repos, build from source instead |

## Manifest mode

Run without a package name to process `package.toml`:

```bash
fetcher fetch
```

### Manifest format

```toml
[package]
name = "my-project"
version = "1.0.0"

[source]
github = "serde-rs"

[deps]
serde = "1.0"
tokio = { version = "1.51", github = "tokio-rs/tokio" }
ripgrep = { version = "14.1.0", github = "BurntSushi/ripgrep", build = "" }
critical-tool = { version = "2.0", sha256 = "abc123..." }

myapp = { version = "1.0", github = "myorg/myapp",
  pre_install = "echo installing $PKG_NAME",
  post_install = "echo done: $PKG_NAME" }
```

### Dependency fields

| Field | Description |
|-------|-------------|
| `version` | Version string (defaults to `"latest"`) |
| `github` | `owner/repo` for GitHub release archives |
| `url` | Direct URL with `{name}`/`{version}` placeholders |
| `git` | Git repository URL |
| `rev` | Branch / tag / commit for Git sources |
| `build` | Build command; `""` triggers auto-detection |
| `sha256` | Expected SHA-256 checksum |
| `pre_install` | Shell command run before installation |
| `post_install` | Shell command run after installation |

### Lockfile

After a successful run, `.fetcher.lock` pins versions, URLs, and checksums. Remove it to re-resolve:

```bash
rm .fetcher.lock
```

## Source builds

Build system auto-detection priority:

1. `Cargo.toml` -- cargo install
2. `go.mod` -- go build
3. `CMakeLists.txt` -- cmake configure/build/install
4. `meson.build` -- meson setup/compile/install
5. `Makefile` -- make && make install
6. `configure` -- autotools
7. `setup.py` / `pyproject.toml` -- python/pip install

After building, all files in `~/.local/bin/` are made executable and ELF binaries are stripped.

## Supported package formats

| Source | Format | Extraction |
|--------|--------|------------|
| APT | `.deb` | data.tar.xz / data.tar.gz |
| Pacman | `.pkg.tar.zst` | Zstandard tar |
| Alpine APK | `.apk` | Gzip tar |
| GitHub / URL | `.zip` / `.tar.gz` / `.tgz` | Full extraction |
| Git | directory | `git clone` |

## Output directories

| Path | Purpose |
|------|---------|
| `.fetcher/` | Cached downloaded archives |
| `.fetcher/builds/{name}-{version}/` | Source build dirs (deleted after build) |
| `~/.local/bin/` | Installed binaries |

## Environment

| Variable | Description |
|----------|-------------|
| `FETCHER_GITHUB_ORG` | Default GitHub org when no source is configured |

## PATH integration

`fetcher` automatically appends `~/.local/bin` to `$PATH` and persists it to your shell RC file (`~/.bashrc`, `~/.zshrc`, `~/.config/fish/config.fish`, or `~/.profile`).

## License

[Apache 2.0](LICENSE)
