#!/usr/bin/env bash
# macOS Apple Silicon prototype toolchain; stays local to this workspace.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v uv >/dev/null
command -v cargo >/dev/null
mkdir -p .tools/bin .tools/cargo .tools/rustup components
export RUSTUP_HOME="$PWD/.tools/rustup"
if [ ! -x .tools/cargo/bin/rustup ]; then
  curl -fL --retry 2 https://static.rust-lang.org/rustup/dist/aarch64-apple-darwin/rustup-init -o .tools/bin/rustup-init
  chmod +x .tools/bin/rustup-init
  CARGO_HOME="$PWD/.tools/cargo" RUSTUP_INIT_SKIP_PATH_CHECK=yes .tools/bin/rustup-init -y --no-modify-path --default-toolchain none --profile minimal
fi
if [ ! -f .tools/export-esp.sh ]; then
  curl -fL --retry 2 https://github.com/esp-rs/espup/releases/download/v0.17.1/espup-aarch64-apple-darwin -o .tools/bin/espup
  chmod +x .tools/bin/espup
  CARGO_HOME="$PWD/.tools/cargo" PATH="$PWD/.tools/cargo/bin:$PATH" .tools/bin/espup install --targets esp32s3 --std --toolchain-version 1.97.0.0 --export-file "$PWD/.tools/export-esp.sh"
fi
if [ ! -x .tools/bin/ldproxy ]; then cargo install ldproxy --version 0.3.5 --locked --root "$PWD/.tools"; fi
if [ ! -d .tools/python ]; then uv venv --python 3.12 .tools/python; fi
uv pip install --python .tools/python/bin/python 'cmake<4' ninja setuptools
for entry in M5Unified:fd40d58b8405ad1e7ed7afab548dab5e7cec5bbb M5GFX:c5a3fefad0b38a52cc750e2c4761e339a69a7208; do
  name=${entry%%:*}; revision=${entry#*:}
  if [ ! -d "components/$name" ]; then
    git init "components/$name"
    git -C "components/$name" remote add origin "https://github.com/m5stack/$name.git"
    git -C "components/$name" fetch --depth 1 origin "$revision"
    git -C "components/$name" checkout --detach FETCH_HEAD
  fi
  test "$(git -C "components/$name" rev-parse HEAD)" = "$revision" || { echo "Unexpected $name revision; stop and review"; exit 1; }
done
printf 'Setup ready. Run bash tools/build.sh. No shell profile was modified.\n'
