#!/bin/bash
cd "$(dirname "$0")/.." || exit 1
uv run --with pyserial python tools/provision.py --reset
printf '\nPress Enter to close this window.'
read -r
