#!/bin/bash
cd "$(dirname "$0")/.." || exit 1
uv run --with pyserial python tools/mic-test.py
printf '\nPress Enter to close this window.'
read -r
