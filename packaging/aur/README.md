# AUR: `kantor-bin`

`kantor-bin/` holds the PKGBUILD and `.SRCINFO` published to
<https://aur.archlinux.org/packages/kantor-bin>. It repackages the prebuilt release
tarballs (`kantor-X.Y.Z-{x86_64,aarch64}-linux.tar.gz`) that `.github/workflows/release.yml`
attaches to each GitHub Release, so nothing is compiled on the user's machine.

The checked-in `.SRCINFO` may carry `SKIP` checksums before a release exists;
`update.sh` fills in the real ones. Never publish with `SKIP`.

## Publishing a release

Needs an Arch box with `base-devel` (for `makepkg`) and the maintainer's AUR SSH key
registered at <https://aur.archlinux.org/account/> (`~/.ssh/config`: `Host aur.archlinux.org`,
`User aur`, `IdentityFile ~/.ssh/aur`).

1. Wait for the `Release` workflow of tag `vX.Y.Z` to finish; the GitHub Release must list
   both tarballs and `SHA256SUMS`.
2. Fill in version, checksums and `.SRCINFO`:
   ```sh
   packaging/aur/update.sh X.Y.Z
   ```
3. Test-build in a scratch copy (downloads the real tarball, checks the sum):
   ```sh
   tmp=$(mktemp -d) && cp packaging/aur/kantor-bin/PKGBUILD "$tmp" && (cd "$tmp" && makepkg -f && namcap PKGBUILD *.pkg.tar.zst)
   ```
4. Commit the change here (`chore(aur): kantor-bin X.Y.Z`) and push to `main`.
5. Push to the AUR (first time: the clone of a not-yet-existing package is empty, that's fine):
   ```sh
   git clone ssh://aur@aur.archlinux.org/kantor-bin.git ~/aur/kantor-bin
   cp packaging/aur/kantor-bin/{PKGBUILD,.SRCINFO} ~/aur/kantor-bin/
   cd ~/aur/kantor-bin
   git add PKGBUILD .SRCINFO
   git commit -m "Update to X.Y.Z"
   git push origin master
   ```
   The AUR only accepts the `master` branch and requires `.SRCINFO` in every commit.

A packaging-only fix (same version): bump `pkgrel` in the PKGBUILD, run
`(cd packaging/aur/kantor-bin && makepkg --printsrcinfo > .SRCINFO)`, then steps 4–5.

## Testing locally without a release

`makepkg` uses a source file that already sits next to the PKGBUILD instead of
downloading it, so a locally built tarball can stand in for the release asset:

```sh
cargo build --release -p kantor
v=$(sed -n 's/^pkgver=//p' packaging/aur/kantor-bin/PKGBUILD)
tmp=$(mktemp -d)
scripts/package.sh target/release/kantor "$tmp/kantor-$v-x86_64-linux"
(cd "$tmp" && sha256sum kantor-*.tar.gz > SHA256SUMS)   # aarch64 line: reuse the x86_64 sum
sed -i "p; s/x86_64/aarch64/" "$tmp/SHA256SUMS"
cp packaging/aur/kantor-bin/PKGBUILD "$tmp/" && cp -r packaging/aur/update.sh "$tmp/"
# run update.sh against the copy so the checked-in PKGBUILD keeps its checksums:
mkdir "$tmp/kantor-bin" && mv "$tmp/PKGBUILD" "$tmp/kantor-bin/" && mv "$tmp/kantor-$v-x86_64-linux.tar.gz" "$tmp/kantor-bin/"
"$tmp/update.sh" "$v" "$tmp/SHA256SUMS"
(cd "$tmp/kantor-bin" && makepkg -f && tar -tf kantor-bin-*.pkg.tar.zst)
```

Don't install the result with `pacman -U` unless you mean to.

## Runtime dependencies

`depends` follows what the binary actually loads (re-check when GPUI or wgpu change):

- linked (`readelf -d /usr/bin/kantor | grep NEEDED`): glibc (libc, libm), gcc-libs
  (libgcc_s), libxcb, libxkbcommon, libxkbcommon-x11;
- `dlopen`ed: wayland (libwayland-client, libwayland-egl) and vulkan-icd-loader
  (libvulkan.so.1, loaded by wgpu's ash backend; the name is inlined, so `strings` misses it);
  libglvnd (libEGL) only for the GLES fallback, hence optional;
- read at run time: fontconfig's `/etc/fonts/*.conf` (parsed in Rust, no libfontconfig).

freetype2, alsa-lib, openssl, zstd and dbus are build-time `-dev` packages in
`scripts/ci-deps.sh`, but the release binary links none of them. A Vulkan driver
(`vulkan-driver`) is required to draw anything but stays optional because the right one
depends on the GPU.
