#!/bin/sh
# Render the store artwork from assets/src with headless Chromium in Docker.
# Output: assets/icon.png (640x640, transparent) and assets/cover.png (1600x1000).
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
docker build -q -t audiobridge-art:local "$ROOT/assets/src" >/dev/null
render() {
  # $1 page, $2 width, $3 height, $4 output file name in assets/
  MSYS_NO_PATHCONV=1 docker run --rm -v "$ROOT/assets:/assets" audiobridge-art:local \
    chromium --headless=new --no-sandbox --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
      --default-background-color=00000000 --window-size="$2,$3" \
      --screenshot="/assets/$4" "file:///assets/src/$1" >/dev/null 2>&1
}
render icon.svg 640 640 icon.png
render cover.html 1600 1000 cover.png
echo "Wrote assets/icon.png and assets/cover.png"
