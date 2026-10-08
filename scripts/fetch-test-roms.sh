#!/usr/bin/env sh
# Clones the open-source test ROM suites into tests/roms (gitignored).
# Run from anywhere: `sh scripts/fetch-test-roms.sh`
set -eu
cd "$(dirname "$0")/../tests/roms" 2>/dev/null || { mkdir -p "$(dirname "$0")/../tests/roms"; cd "$(dirname "$0")/../tests/roms"; }

clone() { [ -d "$2" ] || git clone -q --depth 1 "$1" "$2"; }

clone https://github.com/jsmolka/gba-tests.git gba-tests                       # CPU, memory, PPU, saves (MIT)
clone https://github.com/DenSinH/FuzzARM.git FuzzARM                           # randomized ARM/Thumb fuzz tests
clone https://github.com/destoer/armwrestler-gba-fixed.git armwrestler-gba-fixed # classic CPU test

echo "Test ROMs ready in $(pwd)"
