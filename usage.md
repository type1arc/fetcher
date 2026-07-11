# fetcher

A configurable package fetcher — downloads from system repositories (APT, Pacman, APK),
builds from source (auto-detects Cargo, Go, CMake, Meson, Make, Autotools, Python),
clones Git repos, or downloads from arbitrary URLs.

## Installation

```bash
# Build from source
cargo build --release
# binary at target/release/fetcher
```

Arch Linux (AUR): `fetcher-cli` (e.g. `yay -S fetcher-cli`).

**Runtime dependencies:** `git` for `--git` clone operations; `sha256sum` for checksum
verification; `strip` for binary stripping; `sh` for pre/post-install scripts.

## Usage

```
fetcher fetch [PKG] [OPTIONS]
fetcher fetch              # manifest mode (reads package.toml)
fetcher source <src>       # download source and build (auto-detect or --build)
fetcher clean              # remove cached .fetcher/ directory
fetcher unfetch [PKG]      # unfetching (uninstalling) a package
```

### Global options

| Flag | Description |
|------|-------------|
| `--github <owner/repo>` | Download from GitHub releases archive |
| `--url <URL>` | Download from an arbitrary URL (supports `{name}`/`{version}`) |
| `--git <URL>` | Clone a Git repository (requires `git` CLI) |
| `--rev <ref>` | Branch, tag, or commit for `--git` sources |
| `--build <cmd>` | Build from source using a shell command (`INSTALL_DIR`, `PKG_NAME`, `PKG_VERSION` env vars available) |
| `--from-source` | Build from source instead of searching system repos (auto-detects build system) |

The package name may include a version with `@`:

```bash
fetcher fetch neovim@0.10.0
```

When a version is omitted, `latest` is used.

## Fetch a single package

### From system repositories

Searches APT, Pacman, and Alpine APK repositories, downloads the first match,
and extracts to `~/.local/bin/`:

```bash
fetcher fetch neovim
```

After searching, a status line shows how many matches were found:
```
found 3 package(s) in 2 repo(s)
```

### From source (auto-detect build system)

```bash
# Auto-detect Cargo.toml, go.mod, CMakeLists.txt, etc.
fetcher fetch ripgrep --from-source --github BurntSushi/ripgrep

# Or with an explicit build command
fetcher fetch myapp --from-source --url "https://example.com/src/{name}-{version}.tar.gz"
```

### Source command (standalone build and install)

```bash
# Download HEAD.zip from GitHub, auto-detect build system, install to ~/.local/bin
fetcher source BurntSushi/ripgrep

# With a specific version tag
fetcher source BurntSushi/ripgrep --version 14.1.0

# From a URL
fetcher source "https://example.com/src/pkg-1.0.tar.gz"

# With an explicit build command instead of auto-detection
fetcher source BurntSushi/ripgrep --build "cargo build --release && cp target/release/rg \$INSTALL_DIR/"
```

Build environment variables:

| Variable | Description |
|----------|-------------|
| `INSTALL_DIR` | Target install directory (`~/.local/bin`) |
| `PKG_NAME` | Package name |
| `PKG_VERSION` | Package version |

### From GitHub releases

```bash
fetcher fetch tokio --github tokio-rs/tokio
# downloads github.com/tokio-rs/tokio/archive/refs/tags/v{version}.zip
```

### From a URL

```bash
fetcher fetch mylib --url "https://example.com/downloads/mylib-{version}.zip"
```

Placeholders `{name}` and `{version}` are substituted automatically.

### From a Git repository

```bash
fetcher fetch mylib --git "https://github.com/myorg/mylib.git" --rev main
```

Clones into `~/.local/bin/{name}-{version}/`.

### Fallback (no source flag)

Without `--github` / `--url` / `--git` / `--from-source`:

1. Search system repos (APT → Pacman → APK) for an exact package name match.
2. If not found, attempt to build a URL from the `FETCHER_GITHUB_ORG` env variable.
3. If no source resolves, print an error.

## Manifest mode (package.toml)

Run without a package name to process `package.toml`:

```bash
fetcher fetch
```

Dependencies are installed with lockfile pinning, checksum verification,
optional pre/post-install scripts, and recursive nested dependency resolution
(if a dep's install dir contains a `package.toml`, its sub-deps are installed too).

### Manifest format

```toml
[pkg_name]
name = "my-project"
version = "1.0.0"

# Default source for deps without explicit overrides
[source]
github = "serde-rs"                     # default GitHub org (used when dep is "1.0")
# registry = "https://crates.io/api/v1/crates/{name}/{version}/download"
# url = "https://releases.example.com/{name}/v{version}.zip"

# Global hooks (run before/after all deps)
pre_install = "echo starting"
post_install = "echo done"

[deps]
# Uses the default [source]
serde = "1.0"
serde_json = "1.0"

# Explicit GitHub repo (different org)
tokio = { version = "1.51", github = "tokio-rs/tokio" }

# Direct URL download
my-custom-lib = { version = "2.0", url = "https://example.com/downloads/{name}-{version}.zip" }

# Git clone
my-internal-lib = { git = "https://github.com/myorg/my-internal-lib.git", rev = "main" }

# Build from source (auto-detect build system)
ripgrep = { version = "14.1.0", github = "BurntSushi/ripgrep", build = "" }

# Build from source with explicit command
myapp = { version = "1.0", github = "myorg/myapp", build = "cmake -B build && cmake --build build && cp build/myapp \"$INSTALL_DIR/\"" }

# Build from source via --from-source flag
# (no build field, but fetcher fetch --from-source will skip repos and build)
mylib = { version = "1.0", github = "myorg/mylib" }

# Checksum verification
critical-tool = { version = "2.0", sha256 = "abc123..." }
```

### Dependency fields

| Field | Required | Description |
|-------|----------|-------------|
| `version` | No | Version string (defaults to `"latest"`) |
| `github` | No | `owner/repo` — downloads GitHub release archive |
| `url` | No | Direct URL with `{name}`/`{version}` placeholders |
| `git` | No | Git repository URL |
| `rev` | No | Branch / tag / commit for Git sources |
| `build` | No | Build command; empty string `""` triggers auto-detection |
| `sha256` | No | Expected SHA-256 checksum for verification |
| `pre_install` | No | Shell command run before installation |
| `post_install` | No | Shell command run after successful installation |

A dependency can also be written as a plain version string (e.g. `serde = "1.0"`)
when it should use the default `[source]`.

### Pre/Post-install scripts

```toml
[deps]
myapp = { version = "1.0", github = "myorg/myapp",
  pre_install = "echo installing $PKG_NAME",
  post_install = "echo done: $PKG_NAME" }
```

Scripts run via `sh -c` with `INSTALL_DIR`, `PKG_NAME` env vars and real-time
stdout/stderr output.

### Lockfile

After a successful run, `.fetcher.lock` is written with pinned versions,
download URLs, repo names, and checksums. On subsequent runs, locked deps
are reused without re-resolving.

```bash
# Remove lockfile to re-resolve all deps
rm .fetcher.lock
```

## Source builds

Build system auto-detection priority:

1. `Cargo.toml` → `cargo install --path . --root "$INSTALL_DIR/.."`
2. `go.mod` → `go build -o "$INSTALL_DIR/{name}" .`
3. `CMakeLists.txt` → cmake configure/build/install
4. `meson.build` → meson setup/compile/install
5. `Makefile` / `makefile` → `make && make install PREFIX="$INSTALL_DIR"`
6. `configure` → `./configure --prefix="$INSTALL_DIR" && make && make install`
7. `setup.py` → `python setup.py install --prefix="$INSTALL_DIR"`
8. `pyproject.toml` → `pip install --prefix="$INSTALL_DIR" .`

After every build:
- All files in `~/.local/bin/` are made executable (`u+rx`)
- ELF binaries are stripped
- Build directory is deleted

## Environment

| Variable | Description |
|----------|-------------|
| `FETCHER_GITHUB_ORG` | Default GitHub organization when no source is configured |

## PATH integration

`fetcher` automatically appends `~/.local/bin` to `$PATH` for the current session
and persists it to your shell RC file (`~/.bashrc`, `~/.zshrc`, `~/.config/fish/config.fish`,
or `~/.profile`).

## Output directories

| Path | Purpose |
|------|---------|
| `.fetcher/` | Cached downloaded archives |
| `.fetcher/builds/{name}-{version}/` | Source build directories (deleted after build) |
| `~/.local/bin/` | Extracted / installed binaries |

## Supported package formats

| Source | Format | Extraction |
|--------|--------|------------|
| APT | `.deb` (ar archive) | `data.tar.xz` or `data.tar.gz` unpacked |
| Pacman | `.pkg.tar.zst` | Zstandard-compressed tar unpacked |
| Alpine APK | `.apk` | Gzip-compressed tar unpacked |
| GitHub / URL | `.zip` / `.tar.gz` / `.tgz` | Full extraction |
| Git | directory | Cloned via `git clone` |

## Clean cache

```bash
fetcher clean
```

Removes the `.fetcher/` directory (download cache and build artifacts).

# Uninstall Package
```bash
fetcher unfetch [PKG]

```

## Notes

- Default mode is **binary installation** from system repos. Source builds require
  `--from-source`, `source` subcommand, or `build` field in `package.toml`.
- Lockfile (`.fetcher.lock`) is read/written in the current working directory.
- Archived files are deleted after successful extraction.
- HTTP client uses a 10-second connect timeout and a 120-second total timeout.
- Pacman mirrors are tried in parallel chunks of 10; the first responsive mirror is used.
- System repo indices are fetched and parsed on every invocation (no local cache).
- Git clone uses the system `git` CLI, not an embedded library.
