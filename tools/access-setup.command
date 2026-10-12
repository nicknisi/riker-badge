#!/bin/bash
cd "$(dirname "$0")/.." || exit 1
python3 tools/save-access-token.py
printf '\nPress Enter to close this window.'
read -r
