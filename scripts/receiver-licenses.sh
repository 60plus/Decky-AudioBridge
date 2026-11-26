#!/bin/sh
# Regenerate receiver/THIRD_PARTY_LICENSES.txt with cargo-about, then add the
# font licenses egui ships, which cargo-about does not pick up by itself.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
docker build -q -t audiobridge-rust:local -f "$ROOT/receiver/Dockerfile.build" "$ROOT/receiver" >/dev/null
MSYS_NO_PATHCONV=1 docker run --rm \
  -v "$ROOT/receiver:/work" \
  -v audiobridge-cargo:/usr/local/cargo/registry \
  -v audiobridge-cargobin:/opt/cargobin \
  -v audiobridge-target:/target \
  -e CARGO_TARGET_DIR=/target -w /work audiobridge-rust:local sh -c '
    set -e
    export PATH=/opt/cargobin/bin:$PATH
    command -v cargo-about >/dev/null || cargo install --locked --features cli --root /opt/cargobin cargo-about
    cargo about generate about.hbs -o THIRD_PARTY_LICENSES.txt 2>/dev/null
    fonts=$(ls -d /usr/local/cargo/registry/src/*/epaint_default_fonts-*/fonts | head -1)
    for f in UFL.txt Hack-Regular.txt emoji-icon-font-mit-license.txt; do
      printf "\n================================================================================\n" >> THIRD_PARTY_LICENSES.txt
      printf "Font license from epaint_default_fonts: %s\n" "$f" >> THIRD_PARTY_LICENSES.txt
      printf -- "--------------------------------------------------------------------------------\n" >> THIRD_PARTY_LICENSES.txt
      cat "$fonts/$f" >> THIRD_PARTY_LICENSES.txt
    done'
echo "Wrote receiver/THIRD_PARTY_LICENSES.txt"
