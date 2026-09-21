# fear-gba

Game Boy Advance emulator written in Rust. The way the subsystems are split up
follows [gdkGBA](https://github.com/gdkchan/gdkGBA).

Pokemon Emerald boots, plays and saves. No BIOS file is required; the SWI calls
are implemented in the emulator and the interrupt dispatcher is assembled into
a small synthetic BIOS image. Pass a real BIOS with `--bios` and that is used
instead.

## Running

```
cargo run --release -- "path/to/game.gba"
```

Flags:

```
--bios bios.bin    use a real BIOS image
--scale 1|2|4      window scale, default 2
--mute             don't open an audio device
--save file.sav    backup memory file, defaults to the ROM path with .sav
```

Backup memory is written out a second after the game stops touching it, and
again when the window closes, so in-game saves survive quitting.

Keys:

```
arrows                d-pad
X / Z                 A / B
A / S                 L / R
enter / backspace     start / select
escape                quit
```

## Headless mode

The same binary runs without a window. This is how the emulator is tested.

```
cargo run --release -- game.gba --headless 4300 \
    --screenshot shot.png --every 500 --from 3000 \
    --input "4300:START:20,4500:A:20"
```

`--input` takes a comma separated list of `frame:KEY[+KEY][:frames_held]`.
`--every` writes a numbered PNG every N frames, `--from` delays the first one,
and `--registers` prints the processor state at the end.

## What works

CPU: full ARM and Thumb instruction sets, all six register banks, the barrel
shifter edge cases, exceptions, and the three stage prefetch pipeline, so code
that overwrites the instructions behind itself runs the old ones like hardware
does.

Memory: the whole map with region mirroring, VRAM's odd upper block, byte
writes duplicating into palette and background VRAM, wait states from WAITCNT,
and BIOS reads from outside the BIOS returning the open bus latch.

Video: scanline renderer for every mode. Four text layers, affine layers, the
three bitmap modes, 128 sprites (regular and affine, 4bpp and 8bpp, both
mapping layouts), both windows and the object window, mosaic, and the alpha,
brighten and darken effects.

DMA: four channels, immediate/vblank/hblank/sound-FIFO timing, address control
including increment-and-reload, and repeat.

Timers: four channels with prescalers, cascade and interrupts.

Audio: both DirectSound FIFOs and all four PSG channels, mixed to 32 kHz
stereo and played through cpal.

Cartridge: save type detection, SRAM, 64K and 128K flash with bank switching
and sector erase, EEPROM, and the GPIO real time clock.

Not done: link cable, the prefetch buffer's timing effects, DMA3 video capture.

## Tests

```
cargo test
```

Two tests are opt-in and skip themselves unless pointed at real files:

```
FEAR_GBA_TEST_SUITE=path/to/gba-tests \
FEAR_GBA_TEST_ROM="path/to/game.gba" cargo test --release
```

`FEAR_GBA_TEST_SUITE` wants a checkout of
[jsmolka/gba-tests](https://github.com/jsmolka/gba-tests). The arm, thumb,
memory, bios, nes, unsafe and all four save ROMs pass.

## Debugging

```
cargo run --release --example trace -- game.gba            # stops where execution derails
cargo run --release --example layers -- game.gba 600 out/  # renders each layer on its own
cargo run --release --example audiostat -- game.gba        # reports what the APU produced
```

`FEAR_GBA_TRACE_MODES=1` makes trace log processor mode changes.
`FEAR_GBA_TRACE_FLASH=1` logs the backup memory command stream.
