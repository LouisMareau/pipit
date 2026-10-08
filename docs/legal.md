# Legal notes

Not legal advice — a summary of the lines the project stays inside.

- **Emulators are legal.** The core is written from public documentation (GBATEK,
  ARM7TDMI technical reference) and tested with open-source ROMs. No Nintendo code is
  used.
- **No ROMs are distributed.** The app only opens files the user already has. Nothing
  is uploaded: ROMs, saves and save states stay in the browser's storage on the device.
- **No Nintendo BIOS.** Pipit ships its own replacement (high-level emulation of the
  BIOS functions plus a small boot/IRQ stub). Users may optionally supply a BIOS dump
  they made themselves; it is stored locally and never bundled.
- **No trademarks.** The name, icon, screenshots and store listings avoid "Nintendo",
  "Game Boy" and their logos. The README carries a non-affiliation notice.
- **Homebrew only in demos.** Any bundled demo content must be homebrew with a licence
  that permits redistribution.
