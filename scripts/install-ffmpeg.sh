#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "$0")/.." && pwd)"
task_tools="${FFMPEG_INSTALL_ROOT:-$task_root/.local/tools}"
mkdir -p "$task_tools/source"
task_archive="$task_tools/source/ffmpeg-9.0.2.tar.xz"
curl --fail --location --proto '=https' --tlsv1.2 https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz -o "$task_archive"
printf '%s  %s\n' '8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e' "$task_archive" | sha256sum -c -
tar -xf "$task_archive" -C "$task_tools/source"
cd "$task_tools/source/ffmpeg-9.0.2"
./configure --prefix="$task_tools" --disable-doc --disable-debug --disable-ffplay --enable-gpl --enable-libx264
make -j2
make install
