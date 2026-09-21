//! Memory map, backup memory, timers, DMA and the picture unit.

use fear_gba::cart::SaveKind;
use fear_gba::ppu::{HEIGHT, WIDTH};
use fear_gba::{Bus, Gba};

fn bus() -> Bus {
    Gba::new(vec![0; 0x200], None).bus
}

/// Drives a flash command sequence, which always starts with the same unlock.
fn flash_command(bus: &mut Bus, command: u8) {
    bus.write8(0x0e00_5555, 0xaa);
    bus.write8(0x0e00_2aaa, 0x55);
    bus.write8(0x0e00_5555, command);
}

#[test]
fn internal_memory_mirrors() {
    let mut bus = bus();
    bus.write32(0x0200_0000, 0x1234_5678);
    assert_eq!(bus.read32(0x0204_0000), 0x1234_5678, "ewram repeats every 256K");
    bus.write32(0x0300_0000, 0xabcd_ef01);
    assert_eq!(bus.read32(0x0300_8000), 0xabcd_ef01, "iwram repeats every 32K");
    assert_eq!(bus.read32(0x03ff_fff8), bus.read32(0x0300_7ff8));
}

#[test]
fn vram_upper_block_mirrors_the_object_area() {
    let mut bus = bus();
    bus.write16(0x0601_0000, 0x7fff);
    assert_eq!(bus.read16(0x0601_8000), 0x7fff);
    assert_eq!(bus.read16(0x0602_0000), bus.read16(0x0600_0000));
}

#[test]
fn byte_writes_to_palette_fill_the_halfword() {
    let mut bus = bus();
    bus.write8(0x0500_0000, 0x3c);
    assert_eq!(bus.read16(0x0500_0000), 0x3c3c);
}

#[test]
fn byte_writes_to_object_vram_and_oam_are_dropped() {
    let mut bus = bus();
    bus.write8(0x0601_0000, 0x5a);
    assert_eq!(bus.read8(0x0601_0000), 0, "object vram ignores byte writes");
    bus.write8(0x0700_0000, 0x5a);
    assert_eq!(bus.read8(0x0700_0000), 0, "oam ignores byte writes");
}

#[test]
fn backup_memory_is_byte_wide() {
    let mut bus = bus();
    bus.cart.kind = SaveKind::Sram;
    bus.cart.save = vec![0xff; 0x8000];
    bus.write8(0x0e00_0000, 0x42);
    assert_eq!(bus.read16(0x0e00_0000), 0x4242, "reads repeat the byte");
    assert_eq!(bus.read32(0x0e00_0000), 0x4242_4242);
    // A halfword store picks its byte using the low address bit.
    bus.write16(0x0e00_0011, 0xaabb);
    assert_eq!(bus.read8(0x0e00_0011), 0xaa);
    bus.write16(0x0e00_0010, 0xaabb);
    assert_eq!(bus.read8(0x0e00_0010), 0xbb);
}

#[test]
fn flash_identifies_as_a_one_megabit_device() {
    let mut bus = bus();
    bus.cart.kind = SaveKind::Flash128;
    bus.cart.save = vec![0xff; 0x20000];
    flash_command(&mut bus, 0x90);
    assert_eq!(bus.read8(0x0e00_0000), 0x62, "manufacturer");
    assert_eq!(bus.read8(0x0e00_0001), 0x13, "device");
    flash_command(&mut bus, 0xf0);
    assert_eq!(bus.read8(0x0e00_0000), 0xff, "back to reading data");
}

#[test]
fn flash_programs_erases_and_switches_banks() {
    let mut bus = bus();
    bus.cart.kind = SaveKind::Flash128;
    bus.cart.save = vec![0xff; 0x20000];

    flash_command(&mut bus, 0xa0);
    bus.write8(0x0e00_0100, 0x12);
    assert_eq!(bus.read8(0x0e00_0100), 0x12);

    // The second bank is a separate 64K window.
    flash_command(&mut bus, 0xb0);
    bus.write8(0x0e00_0000, 1);
    assert_eq!(bus.read8(0x0e00_0100), 0xff, "bank one starts erased");
    flash_command(&mut bus, 0xa0);
    bus.write8(0x0e00_0100, 0x34);
    flash_command(&mut bus, 0xb0);
    bus.write8(0x0e00_0000, 0);
    assert_eq!(bus.read8(0x0e00_0100), 0x12, "bank zero kept its data");

    // Erasing a sector restores it to all ones.
    flash_command(&mut bus, 0x80);
    bus.write8(0x0e00_5555, 0xaa);
    bus.write8(0x0e00_2aaa, 0x55);
    bus.write8(0x0e00_0000, 0x30);
    assert_eq!(bus.read8(0x0e00_0100), 0xff);
}

#[test]
fn save_type_is_detected_from_the_rom() {
    let mut rom = vec![0u8; 0x400];
    rom[0x200..0x209].copy_from_slice(b"FLASH1M_V");
    assert_eq!(Gba::new(rom, None).bus.cart.kind, SaveKind::Flash128);
    let mut rom = vec![0u8; 0x400];
    rom[0x200..0x206].copy_from_slice(b"SRAM_V");
    assert_eq!(Gba::new(rom, None).bus.cart.kind, SaveKind::Sram);
}

#[test]
fn timer_overflow_reloads_and_raises_its_interrupt() {
    let mut bus = bus();
    bus.write16(0x0400_0100, 0xffff); // reload
    bus.write16(0x0400_0102, 0x00c0); // enable with interrupt, no prescaler
    bus.run_cycles(1);
    assert_eq!(bus.read16(0x0400_0100), 0xffff, "reloaded, not wrapped to zero");
    assert_ne!(bus.interrupt_flags & 1 << 3, 0);
}

#[test]
fn timer_prescaler_divides_the_clock() {
    let mut bus = bus();
    bus.write16(0x0400_0100, 0);
    bus.write16(0x0400_0102, 0x0081); // enabled, divide by 64
    bus.run_cycles(63);
    assert_eq!(bus.read16(0x0400_0100), 0);
    bus.run_cycles(1);
    assert_eq!(bus.read16(0x0400_0100), 1);
}

#[test]
fn cascading_timer_counts_the_previous_overflow() {
    let mut bus = bus();
    bus.write16(0x0400_0100, 0xffff);
    bus.write16(0x0400_0102, 0x0080);
    bus.write16(0x0400_0104, 0);
    bus.write16(0x0400_0106, 0x0084); // enabled, cascade
    bus.run_cycles(1);
    assert_eq!(bus.read16(0x0400_0104), 1);
}

#[test]
fn immediate_dma_copies_words() {
    let mut bus = bus();
    bus.write32(0x0200_0000, 0x1122_3344);
    bus.write32(0x0200_0004, 0x5566_7788);
    bus.write32(0x0400_00b0, 0x0200_0000);
    bus.write32(0x0400_00b4, 0x0300_0000);
    bus.write16(0x0400_00b8, 2);
    bus.write16(0x0400_00ba, 0x8400); // enable, 32 bit, immediate
    assert_eq!(bus.read32(0x0300_0000), 0x1122_3344);
    assert_eq!(bus.read32(0x0300_0004), 0x5566_7788);
    assert_eq!(bus.read16(0x0400_00ba) & 0x8000, 0, "a one shot clears itself");
}

#[test]
fn hblank_dma_repeats_once_per_visible_line() {
    let mut bus = bus();
    // The source keeps advancing between repeats, so seed a word per line.
    for line in 0..4 {
        bus.write32(0x0200_0000 + line * 4, 0x0000_00aa);
    }
    bus.write32(0x0400_00b0, 0x0200_0000);
    bus.write32(0x0400_00b4, 0x0300_0000);
    bus.write16(0x0400_00b8, 1);
    // enable, repeat, 32 bit, hblank timing, destination increment+reload
    bus.write16(0x0400_00ba, 0x8000 | 0x0200 | 0x0400 | 0x2000 | 0x0060);
    for _ in 0..3 {
        bus.run_cycles(1232);
    }
    assert_eq!(bus.read32(0x0300_0000), 0xaa);
    assert_ne!(bus.read16(0x0400_00ba) & 0x8000, 0, "a repeating channel stays on");
}

#[test]
fn scanline_counter_and_blanking_flags_follow_the_dot_clock() {
    let mut bus = bus();
    bus.run_cycles(959);
    assert_eq!(bus.read16(0x0400_0004) & 2, 0, "still drawing");
    bus.run_cycles(1);
    assert_ne!(bus.read16(0x0400_0004) & 2, 0, "hblank");
    bus.run_cycles(1232 * 160);
    assert_eq!(bus.read16(0x0400_0006), 160);
    assert_ne!(bus.read16(0x0400_0004) & 1, 0, "vblank");
    bus.run_cycles(1232 * 68);
    assert_eq!(bus.read16(0x0400_0006), 0);
    assert_eq!(bus.read16(0x0400_0004) & 1, 0);
}

#[test]
fn mode_three_bitmap_reaches_the_screen() {
    let mut bus = bus();
    bus.write16(0x0400_0000, 0x0403); // mode 3, background 2 on
    bus.write16(0x0600_0000, 0x001f); // pure red
    bus.run_cycles(1232);
    assert_eq!(bus.ppu.framebuffer[0], 0x00ff_0000);
}

#[test]
fn mode_four_uses_the_palette_and_page_flip() {
    let mut bus = bus();
    bus.write16(0x0400_0000, 0x0404);
    bus.write16(0x0500_0002, 0x03e0); // colour 1 is green
    bus.write8(0x0600_0000, 1);
    bus.run_cycles(1232);
    assert_eq!(bus.ppu.framebuffer[0], 0x0000_ff00);

    bus.write16(0x0400_0000, 0x0414); // second page
    bus.write8(0x0600_a000, 1);
    bus.run_cycles(1232 * 228);
    assert_eq!(bus.ppu.framebuffer[0], 0x0000_ff00);
}

#[test]
fn forced_blank_shows_white() {
    let mut bus = bus();
    bus.write16(0x0400_0000, 0x0483);
    bus.write16(0x0600_0000, 0x001f);
    bus.run_cycles(1232);
    assert_eq!(bus.ppu.framebuffer[0], 0x00ff_ffff);
}

#[test]
fn text_background_draws_a_tile_with_flipping() {
    let mut bus = bus();
    bus.write16(0x0400_0000, 0x0100); // mode 0, background 0 on
    bus.write16(0x0400_0008, 0x0080); // 256 colour, char base 0, map base 0
    bus.write16(0x0500_0002, 0x001f); // colour 1 is red
    // Tile 1 has its top left pixel set; the map puts it at the origin.
    bus.write16(0x0600_0040, 0x0001);
    bus.write16(0x0600_0000, 1);
    bus.run_cycles(1232);
    assert_eq!(bus.ppu.framebuffer[0], 0x00ff_0000);
    assert_eq!(bus.ppu.framebuffer[1], 0, "the rest of the tile is clear");

    // Flipping horizontally moves it to the far side of the tile.
    bus.write16(0x0600_0000, 1 | 0x0400);
    bus.run_cycles(1232 * 228);
    assert_eq!(bus.ppu.framebuffer[7], 0x00ff_0000);
}

#[test]
fn sprites_draw_above_backgrounds() {
    let mut bus = bus();
    bus.write16(0x0400_0000, 0x1040); // objects on, one dimensional mapping
    bus.write16(0x0500_0202, 0x001f); // object palette colour 1 is red
    bus.write16(0x0601_0000, 0x0001); // first pixel of object tile 0
    bus.write16(0x0700_0000, 0x2000); // y = 0, 256 colour
    bus.write16(0x0700_0002, 0x0000); // x = 0
    bus.write16(0x0700_0004, 0x0000); // tile 0
    bus.run_cycles(1232);
    assert_eq!(bus.ppu.framebuffer[0], 0x00ff_0000);
}

#[test]
fn brightness_effects_scale_towards_white_and_black() {
    let mut bus = bus();
    bus.write16(0x0400_0000, 0x0403);
    bus.write16(0x0600_0000, 0x7fff); // white
    bus.write16(0x0400_0050, 0x00c4); // darken, background 2 is the target
    bus.write16(0x0400_0054, 16); // full strength
    bus.run_cycles(1232);
    assert_eq!(bus.ppu.framebuffer[0], 0, "darkened all the way to black");

    bus.write16(0x0600_0000, 0x0000); // black
    bus.write16(0x0400_0050, 0x0084); // brighten
    bus.run_cycles(1232 * 228);
    assert_eq!(bus.ppu.framebuffer[0], 0x00ff_ffff);
}

#[test]
fn a_window_can_hide_a_background() {
    let mut bus = bus();
    bus.write16(0x0400_0000, 0x2403); // mode 3, background 2 on, window 0 on
    bus.write16(0x0600_0000, 0x001f);
    for pixel in 0..WIDTH {
        bus.write16(0x0600_0000 + pixel as u32 * 2, 0x001f);
    }
    bus.write16(0x0400_0040, 0x0010); // window 0 spans x 0..16
    bus.write16(0x0400_0044, 0x0001); // and y 0..1
    bus.write16(0x0400_0048, 0x0004); // inside: only background 2
    bus.write16(0x0400_004a, 0x0000); // outside: nothing
    bus.run_cycles(1232);
    assert_eq!(bus.ppu.framebuffer[0], 0x00ff_0000, "inside the window");
    assert_eq!(bus.ppu.framebuffer[20], 0, "outside it the backdrop shows");
}

#[test]
fn keypad_register_is_active_low() {
    let mut gba = Gba::new(vec![0; 0x200], None);
    gba.set_keys(fear_gba::KEY_A | fear_gba::KEY_START);
    assert_eq!(gba.bus.read16(0x0400_0130), 0x03ff & !(1 | 1 << 3));
}

#[test]
fn a_frame_is_the_expected_length() {
    let mut gba = Gba::new(vec![0; 0x200], None);
    gba.run_frame();
    assert_eq!(gba.framebuffer().len(), WIDTH * HEIGHT);
    assert_eq!(gba.bus.ppu.vcount, 0, "a frame ends back at the first line");
}

#[test]
fn a_write_to_haltcnt_stops_the_processor() {
    let mut gba = Gba::new(vec![0; 0x200], None);
    gba.bus.write8(0x0400_0301, 0x00);
    gba.cpu.step(&mut gba.bus);
    assert!(gba.cpu.halted);
}

#[test]
fn green_swap_exchanges_green_between_neighbours() {
    let mut bus = bus();
    bus.write16(0x0400_0000, 0x0403); // mode 3, background 2 on
    bus.write16(0x0600_0000, 0x001f); // red
    bus.write16(0x0600_0002, 0x03e0); // green
    bus.write16(0x0400_0002, 1);
    bus.run_cycles(1232);
    assert_eq!(bus.ppu.framebuffer[0], 0x00ff_ff00, "red took its neighbour's green");
    assert_eq!(bus.ppu.framebuffer[1], 0x0000_0000, "and gave its own away");
}

#[test]
fn video_capture_runs_from_line_two_until_line_162() {
    let mut bus = bus();
    for word in 0..200u32 {
        bus.write32(0x0200_0000 + word * 4, 0xaa);
    }
    bus.write32(0x0400_00d4, 0x0200_0000);
    bus.write32(0x0400_00d8, 0x0300_0000);
    bus.write16(0x0400_00dc, 1);
    // enable, repeat, 32 bit, special timing
    bus.write16(0x0400_00de, 0x8000 | 0x0200 | 0x0400 | 0x3000);

    bus.run_cycles(1232); // line 0
    bus.run_cycles(1232); // line 1
    assert_eq!(bus.read32(0x0300_0000), 0, "nothing before line two");
    bus.run_cycles(1232); // line 2
    assert_eq!(bus.read32(0x0300_0000), 0xaa);

    for _ in 0..200 {
        bus.run_cycles(1232);
    }
    assert_eq!(
        bus.read16(0x0400_00de) & 0x8000,
        0,
        "the channel switches itself off at line 162"
    );
}

#[test]
fn a_serial_transfer_completes_instead_of_hanging() {
    let mut bus = bus();
    assert_eq!(bus.read16(0x0400_0120), 0xffff, "no partner is connected");
    bus.write16(0x0400_0128, 0x4080); // start, with the interrupt enabled
    assert_eq!(bus.read16(0x0400_0128) & 0x0080, 0, "the busy bit cleared");
    assert_ne!(bus.interrupt_flags & 1 << 7, 0);
}

#[test]
fn cartridge_reads_are_cheaper_when_they_follow_on() {
    let mut bus = bus();
    bus.access_cycles = 0;
    bus.read16(0x0800_0000);
    let first = bus.access_cycles;

    bus.access_cycles = 0;
    bus.read16(0x0800_0002);
    let sequential = bus.access_cycles;

    bus.access_cycles = 0;
    bus.read16(0x0800_1000);
    let jumped = bus.access_cycles;

    assert!(sequential < first, "{sequential} should beat {first}");
    assert_eq!(jumped, first, "a jump pays the full price again");
}

#[test]
fn the_prefetch_unit_speeds_up_straight_line_code() {
    let mut bus = bus();
    bus.write16(0x0400_0204, 0x4000); // WAITCNT with prefetch enabled
    bus.read16_code(0x0800_0000);
    bus.access_cycles = 0;
    bus.read16_code(0x0800_0002);
    assert_eq!(bus.access_cycles, 1, "the buffer already holds it");

    bus.write16(0x0400_0204, 0x0000); // prefetch off
    bus.read16_code(0x0800_0000);
    bus.access_cycles = 0;
    bus.read16_code(0x0800_0002);
    assert!(bus.access_cycles > 1);
}
