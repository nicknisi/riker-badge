#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTUP_HOME="$PWD/.tools/rustup"
SDK_PYTHON_BIN="$(dirname "$(uv python find 3.12)")"
export PATH="$SDK_PYTHON_BIN:$PWD/.tools/rustup/toolchains/esp/bin:$PWD/.tools/bin:$PWD/.tools/python/bin:$PATH"
source .tools/export-esp.sh
# Reuse the installed GCC + espup's verified libclang rather than downloading
# a second LLVM distribution. SDK Python and CMake/Ninja remain project-local.
SDK="$PWD/.tools/espressif"
GCC="$SDK/tools/xtensa-esp-elf/esp-14.2.0_20250730/xtensa-esp-elf/bin"
if [ -x "$GCC/xtensa-esp32s3-elf-gcc" ] && [ -x "$SDK/python_env/idf5.4_py3.12_env/bin/python" ]; then
  export ESP_IDF_TOOLS_INSTALL_DIR=fromenv
  export IDF_PATH="$SDK/esp-idf/v5.4.3"
  export IDF_TOOLS_PATH="$SDK"
  export IDF_PYTHON_ENV_PATH="$SDK/python_env/idf5.4_py3.12_env"
  export ESP_ROM_ELF_DIR="$SDK/tools/esp-rom-elfs/20241011"
  export PATH="$IDF_PATH/tools:$GCC:$SDK/tools/esp32ulp-elf/2.38_20240113/esp32ulp-elf/bin:$IDF_PYTHON_ENV_PATH/bin:$PATH"
fi
# esp-idf-sys doesn't watch local extra-component sources. Invalidate its
# documented sdkconfig input only when those sources change, so C++/FFI edits really rebuild.
python3 - <<'PY'
from pathlib import Path
import hashlib
files=sorted(p for p in Path('components/badge_display').rglob('*') if p.is_file())
h=hashlib.sha256()
for p in files:h.update(str(p).encode());h.update(p.read_bytes())
key=Path('.tools/native-sources.sha');value=h.hexdigest()
if not key.exists() or key.read_text()!=value:
 Path('sdkconfig.defaults').touch();key.write_text(value)
PY
# Limit the native glob to its one input; walking the whole workspace follows
# package-manager symlink graphs and can exceed filesystem path limits.
mkdir -p .tools/native-input
cmp -s partitions.csv .tools/native-input/partitions.csv || cp partitions.csv .tools/native-input/partitions.csv
export ESP_IDF_GLOB_BASE="$PWD/.tools/native-input"
export ESP_IDF_GLOB_FILES="partitions.csv"
exec cargo build --locked --release --features firmware --target xtensa-esp32s3-espidf -Z build-std=std,panic_abort
