#!/usr/bin/env bash
# System libraries GPUI needs to build on Ubuntu (CI runners).
set -euo pipefail
# GitHub's x86_64 runners point apt at azure.archive.ubuntu.com through a mirror list. When that
# name doesn't resolve (as on 2026-11-11), every download fails after ~25 minutes; put Ubuntu's own
# archive first so apt falls back to it, and retry flaky downloads.
if [ -f /etc/apt/apt-mirrors.txt ]; then
  { echo "http://archive.ubuntu.com/ubuntu/	priority:1"; cat /etc/apt/apt-mirrors.txt; } \
    | sudo tee /etc/apt/apt-mirrors.txt.new >/dev/null
  sudo mv /etc/apt/apt-mirrors.txt.new /etc/apt/apt-mirrors.txt
fi
echo 'Acquire::Retries "3"; Acquire::http::Timeout "30";' | sudo tee /etc/apt/apt.conf.d/80-kantor-ci >/dev/null
sudo apt-get update -qq
sudo apt-get install -y -qq --no-install-recommends \
  pkg-config clang cmake \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-xcb-dev libxcb1-dev libvulkan-dev \
  libfontconfig1-dev libfreetype-dev \
  libasound2-dev libssl-dev libzstd-dev libdbus-1-dev
