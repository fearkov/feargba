# fear-gba

A Rust desktop Game Boy Advance emulator inspired by the subsystem layout of [gdkGBA](https://github.com/gdkchan/gdkGBA).

## Status

This is an intentionally small, playable-core starting point. It currently includes:

- ARM state reset and a focused ARM7TDMI ARM/Thumb interpreter
- GBA memory map with BIOS, EWRAM, IWRAM, I/O, palette, VRAM, OAM, and mirrored cartridge reads
- Mode 3 framebuffer conversion from GBA BGR555 pixels
- Desktop window, frame pacing, and a starter input hook

Graphics registers, DMA, timers, audio, save hardware, and the remaining instruction tables are planned next. The original gdkGBA is C and public domain; this project is an independent Rust implementation guided by its public architecture.

## Run

```sh
cargo run --release -- path/to/game.gba [path/to/gba_bios.bin]
```

To verify the desktop renderer immediately without a ROM:

```sh
cargo run --release -- --demo
```

Press `Escape` to close the emulator.
