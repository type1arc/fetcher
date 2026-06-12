# fetcher

A configurable package fetcher — downloads from system repositories (APT, Pacman, APK),
GitHub releases, arbitrary URLs, or Git repositories.

## Installation

```bash
# Build from source
cargo build --release
# binary at target/release/fetcher
```

Arch Linux (AUR): `fetcher-cli` (e.g. `yay -S fetcher-cli`).

**Runtime dependency:** `git` is required for `--git` clone operations.

## Usage

```
fetcher fetch [PKG] [OPTIONS]
fetcher fetch              # manifest mode (reads package.toml)
```

### Options

| Flag | Description |
|------|-------------|
| `--github <owner/repo>` | Download from GitHub releases archive |
| `--url <URL>` | Download from an arbitrary URL |
| `--git <URL>` | Clone a Git repository (requires `git` CLI) |
| `--rev <ref>` | Branch, tag, or commit for `--git` sources |

The package name may include a version with `@`:

```bash
fetcher fetch neovim@0.10.0
```

When a version is omitted, `latest` is used.

### Version

```bash
fetcher --version
```

## Fetch a single package

### From system repositories

Searches APT, Pacman, and Alpine APK repositories (in that order), lists all
matching versions, downloads the first match, and extracts it into `.fcache/root/`:

```bash
fetcher fetch neovim
```

If more than one repo provides the package, all are listed with indices and the
entry at index `[0]` is downloaded automatically:

```
found 'neovim' in 2 repo(s):
  [0] 0.10.0 (from apt:http://deb.debian.org/debian)
  [1] 0.9.5 (from apt:http://archive.ubuntu.com/ubuntu)
downloading from [0]
```

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

Clones into `.fcache/{name}-{version}/`.

### Fallback (no source flag)

Without `--github` / `--url` / `--git`:

1. Search system repos (APT → Pacman → APK) for an exact package name match.
2. If not found, attempt to build a URL from the `FETCHER_GITHUB_ORG` env variable:
   `https://github.com/{org}/{name}/archive/refs/tags/v{version}.zip`.
3. If no source resolves, print an error.

## Manifest mode (package.toml)

Run without a package name to process `package.toml`:

```bash
fetcher fetch
```

All dependencies are fetched **concurrently**.

### Manifest format

```toml
[pkg_name]
name = "my-project"
version = "1.0.0"

# Default source for deps without explicit overrides
[source]
github = "serde-rs"                     # default GitHub org
# registry = "https://crates.io/api/v1/crates/{name}/{version}/download"
# url = "https://releases.example.com/{name}/v{version}.zip"

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

# Git clone (default branch)
other-internal = { git = "https://github.com/myorg/other.git" }
```

Dependency fields:

| Field | Required | Description |
|-------|----------|-------------|
| `version` | No | Version string (defaults to `"latest"`) |
| `github` | No | `owner/repo` — downloads GitHub release archive |
| `url` | No | Direct URL with `{name}`/`{version}` placeholders |
| `git` | No | Git repository URL |
| `rev` | No | Branch / tag / commit for Git sources |

A dependency can also be written as a plain version string (e.g. `serde = "1.0"`)
when it should use the default `[source]`.

## Environment

| Variable | Description |
|----------|-------------|
| `FETCHER_GITHUB_ORG` | Default GitHub organization used when no source is configured |

## Output directories

| Path | Purpose |
|------|---------|
| `.fcache/` | Downloaded package files |
| `.fcache/root/` | Extracted package contents (system repos) |
| `.fcache/{name}-{version}/` | Git clone destination |

## Supported package formats

| Source | Format | Extraction |
|--------|--------|------------|
| APT | `.deb` (ar archive) | `data.tar.xz` or `data.tar.gz` unpacked |
| Pacman | `.pkg.tar.zst` | Zstandard-compressed tar unpacked |
| Alpine APK | `.apk` | Gzip-compressed tar unpacked |
| GitHub / URL | `.zip` | Downloaded only (no extraction) |
| Git | directory | Cloned via `git clone` |

## Notes

- HTTP client uses a 3-second connect timeout and a 10-second total timeout.
- Pacman mirrors are tried in parallel chunks of 10; the first responsive mirror is used.
- System repo indices are fetched and parsed on every invocation (no local cache).
- Git clone uses the system `git` CLI, not an embedded library.
