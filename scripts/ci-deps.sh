#!/usr/bin/env bash
# System libraries GPUI needs to build on Ubuntu (CI runners).
set -euo pipefail
sudo apt-get update -qq
sudo apt-get install -y -qq --no-install-recommends \
  pkg-config clang cmake \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-xcb-dev libxcb1-dev libvulkan-dev \
  libfontconfig1-dev libfreetype-dev \
  libasound2-dev libssl-dev libzstd-dev libdbus-1-dev
