# AUR Packaging

Two packages on the AUR:

- `ghpending-bin` — installs the prebuilt Linux x86_64 binary from GitHub Releases. Fast install, no toolchain needed.
- `ghpending` — builds from the GitHub source tag with the local Rust toolchain. Supports x86_64 and aarch64.

The PKGBUILD files in this directory are the source of truth and are pushed to the AUR repos **automatically** by `.github/workflows/release.yml` on every `v*` tag. You should not need to push to AUR by hand.

## User install

```sh
yay -S ghpending-bin    # prebuilt binary, fastest on x86_64
yay -S ghpending        # builds from source, supports x86_64/aarch64
```

Or manually:

```sh
git clone https://aur.archlinux.org/ghpending-bin.git
cd ghpending-bin
makepkg -si
```

## How the automation works

The `aur` job in `release.yml` runs on each `v*` tag, after the GitHub Release is published. For each package it:

1. Resolves the version from the tag and downloads the build artifacts.
2. Computes fresh `sha256sums` for the source tarball / binary tarball / LICENSE / README.
3. Rewrites a copy of the PKGBUILD with the new `pkgver` and checksums.
4. Hands the updated PKGBUILD to [`KSXGitHub/github-actions-deploy-aur`](https://github.com/KSXGitHub/github-actions-deploy-aur), which regenerates `.SRCINFO`, commits, and pushes to AUR over SSH.

The SSH key registered with the AUR account is stored in the `AUR_SSH_KEY` repo secret.

## Local testing

The committed PKGBUILDs always carry the values for the **last released** version, so they're usable for `makepkg` without changes:

```sh
cd packaging/aur
makepkg -p PKGBUILD-bin --verifysource     # checks sources + sha256sums
makepkg -p PKGBUILD-bin -Ccf               # full build in a clean chroot
```

After bumping `pkgver` locally, refresh the checksums quickly:

```sh
version=X.Y.Z

# Source tarball (PKGBUILD)
curl -fsSL "https://github.com/akitaonrails/ghpending/archive/refs/tags/v${version}.tar.gz" | sha256sum

# Prebuilt binary (PKGBUILD-bin)
curl -fsSL "https://github.com/akitaonrails/ghpending/releases/download/v${version}/ghpending-linux-x86_64.tar.gz.sha256"

# LICENSE + README (PKGBUILD-bin)
curl -fsSL "https://raw.githubusercontent.com/akitaonrails/ghpending/v${version}/LICENSE" | sha256sum
curl -fsSL "https://raw.githubusercontent.com/akitaonrails/ghpending/v${version}/README.md" | sha256sum
```

## Manual override

If the automation breaks and you need to push by hand:

```sh
git clone ssh://aur@aur.archlinux.org/ghpending-bin.git
cd ghpending-bin
cp ../path/to/ghpending/packaging/aur/PKGBUILD-bin ./PKGBUILD
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO
git commit -m "Update to vX.Y.Z"
git push
```

Same steps for `ghpending` (source package), pointing at `ghpending.git` on AUR.
