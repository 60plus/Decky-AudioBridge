#!/bin/sh
# Test the receiver on Linux and cross-compile the Windows program with Docker.
# Output: out/AudioBridgeReceiver.exe
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
docker build -q -t audiobridge-rust:local -f "$ROOT/receiver/Dockerfile.build" "$ROOT/receiver" >/dev/null
MSYS_NO_PATHCONV=1 docker run --rm \
  -v "$ROOT/receiver:/work" \
  -v audiobridge-cargo:/usr/local/cargo/registry \
  -v audiobridge-target:/target \
  -e CARGO_TARGET_DIR=/target -w /work audiobridge-rust:local sh -c '
    set -e
    cargo fmt --check
    cargo test --release
    cargo clippy --release --target x86_64-pc-windows-gnu -- -D warnings
    cargo build --release --target x86_64-pc-windows-gnu
    mkdir -p /work/target-out
    cp /target/x86_64-pc-windows-gnu/release/AudioBridgeReceiver.exe /work/target-out/'
mkdir -p "$ROOT/out"
mv "$ROOT/receiver/target-out/AudioBridgeReceiver.exe" "$ROOT/out/AudioBridgeReceiver.exe"
rmdir "$ROOT/receiver/target-out"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/receiver/Cargo.toml" | head -1)
MSYS_NO_PATHCONV=1 docker run --rm -v "$ROOT:/src" -w /src -e PYTHONDONTWRITEBYTECODE=1 python:3.11-slim python -I -c "
import hashlib, sys, zipfile
version = sys.argv[1]
name = 'out/AudioBridgeReceiver-%s-windows.zip' % version
files = [('out/AudioBridgeReceiver.exe', 'AudioBridgeReceiver.exe'), ('LICENSE', 'LICENSE.txt'),
         ('receiver/THIRD_PARTY_LICENSES.txt', 'THIRD_PARTY_LICENSES.txt')]
with zipfile.ZipFile(name, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for src, dst in files:
        info = zipfile.ZipInfo(dst, (2026, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, open(src, 'rb').read())
for path in (name, 'out/AudioBridgeReceiver.exe'):
    digest = hashlib.sha256(open(path, 'rb').read()).hexdigest()
    open(path + '.sha256', 'w').write('%s  %s\n' % (digest, path.split('/')[-1]))
    print(digest, path)
" "$VERSION"
echo "Built out/AudioBridgeReceiver.exe and out/AudioBridgeReceiver-$VERSION-windows.zip"
