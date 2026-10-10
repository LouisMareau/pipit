#!/usr/bin/env sh
# Clones the open-source test ROM suites into tests/roms (gitignored).
# Run from anywhere: `sh scripts/fetch-test-roms.sh`
set -eu
cd "$(dirname "$0")/../tests/roms" 2>/dev/null || { mkdir -p "$(dirname "$0")/../tests/roms"; cd "$(dirname "$0")/../tests/roms"; }

clone() { [ -d "$2" ] || git clone -q --depth 1 "$1" "$2"; }

clone https://github.com/jsmolka/gba-tests.git gba-tests                       # CPU, memory, PPU, saves (MIT)
clone https://github.com/DenSinH/FuzzARM.git FuzzARM                           # randomized ARM/Thumb fuzz tests
clone https://github.com/destoer/armwrestler-gba-fixed.git armwrestler-gba-fixed # classic CPU test



# Game Boy / Game Boy Color
clone https://github.com/retrio/gb-test-roms.git gb-test-roms                  # Blargg's CPU, timing and sound tests
fetch() { [ -f "$2" ] || curl -fsSL "$1" -o "$2"; }
mkdir -p acid2
fetch https://github.com/mattcurrie/dmg-acid2/releases/download/v1.0/dmg-acid2.gb acid2/dmg-acid2.gb   # DMG PPU (MIT)
fetch https://github.com/mattcurrie/cgb-acid2/releases/download/v1.1/cgb-acid2.gbc acid2/cgb-acid2.gbc # CGB PPU (MIT)
MTS=mts-20260714-0944-31510e1                                                   # Gekkio's mooneye test suite (MIT)
if [ ! -d mooneye ]; then
  fetch "https://gekkio.fi/files/mooneye-test-suite/$MTS/$MTS.zip" mooneye.zip
  unzip -q mooneye.zip && mv "$MTS" mooneye && rm mooneye.zip
fi
echo "Test ROMs ready in $(pwd)"
