//! The high level BIOS calls games rely on.

use fear_gba::{Bus, Cpu, Gba};

fn system() -> (Cpu, Bus) {
    let gba = Gba::new(vec![0; 0x200], None);
    (gba.cpu, gba.bus)
}

/// Writes bytes into work ram so a BIOS call can read them.
fn load(bus: &mut Bus, address: u32, data: &[u8]) {
    for (index, byte) in data.iter().enumerate() {
        bus.write8(address + index as u32, *byte);
    }
}

#[test]
fn division_returns_quotient_remainder_and_magnitude() {
    let (mut cpu, mut bus) = system();
    cpu.r[0] = (-7i32) as u32;
    cpu.r[1] = 2;
    cpu.software_interrupt(&mut bus, 0x06);
    assert_eq!(cpu.r[0] as i32, -3);
    assert_eq!(cpu.r[1] as i32, -1);
    assert_eq!(cpu.r[3], 3);
}

#[test]
fn division_by_zero_leaves_the_registers_alone() {
    let (mut cpu, mut bus) = system();
    cpu.r[0] = 5;
    cpu.r[1] = 0;
    cpu.software_interrupt(&mut bus, 0x06);
    assert_eq!(cpu.r[0], 5);
}

#[test]
fn square_root_truncates() {
    let (mut cpu, mut bus) = system();
    cpu.r[0] = 1000;
    cpu.software_interrupt(&mut bus, 0x08);
    assert_eq!(cpu.r[0], 31);
}

#[test]
fn cpu_set_copies_and_fills() {
    let (mut cpu, mut bus) = system();
    bus.write32(0x0200_0000, 0xdead_beef);
    cpu.r[0] = 0x0200_0000;
    cpu.r[1] = 0x0300_0000;
    cpu.r[2] = (1 << 26) | 2; // word mode, two units
    bus.write32(0x0200_0004, 0x1234_5678);
    cpu.software_interrupt(&mut bus, 0x0b);
    assert_eq!(bus.read32(0x0300_0000), 0xdead_beef);
    assert_eq!(bus.read32(0x0300_0004), 0x1234_5678);

    cpu.r[0] = 0x0200_0000;
    cpu.r[1] = 0x0300_0100;
    cpu.r[2] = (1 << 24) | (1 << 26) | 3; // fill three words
    cpu.software_interrupt(&mut bus, 0x0b);
    for offset in 0..3 {
        assert_eq!(bus.read32(0x0300_0100 + offset * 4), 0xdead_beef);
    }
}

#[test]
fn cpu_fast_set_fills_in_blocks() {
    let (mut cpu, mut bus) = system();
    bus.write32(0x0200_0000, 0xaabb_ccdd);
    cpu.r[0] = 0x0200_0000;
    cpu.r[1] = 0x0300_0000;
    cpu.r[2] = (1 << 24) | 8;
    cpu.software_interrupt(&mut bus, 0x0c);
    for offset in 0..8 {
        assert_eq!(bus.read32(0x0300_0000 + offset * 4), 0xaabb_ccdd);
    }
}

#[test]
fn lz77_expands_literals_and_back_references() {
    let (mut cpu, mut bus) = system();
    // Header: type 1, eight bytes out. One literal block then a back reference
    // copying four bytes from one byte back.
    load(
        &mut bus,
        0x0200_0000,
        &[0x10, 8, 0, 0, 0b0000_1000, b'A', b'B', b'C', b'D', 0x10, 0x00],
    );
    cpu.r[0] = 0x0200_0000;
    cpu.r[1] = 0x0300_0000;
    cpu.software_interrupt(&mut bus, 0x11);
    let mut output = Vec::new();
    for index in 0..8 {
        output.push(bus.read8(0x0300_0000 + index));
    }
    assert_eq!(&output, b"ABCDDDDD");
}

#[test]
fn run_length_decoding_expands_runs_and_literals() {
    let (mut cpu, mut bus) = system();
    // A run of five 'B' then two literals.
    load(
        &mut bus,
        0x0200_0000,
        &[0x30, 7, 0, 0, 0x82, b'B', 0x01, b'X', b'Y'],
    );
    cpu.r[0] = 0x0200_0000;
    cpu.r[1] = 0x0300_0000;
    cpu.software_interrupt(&mut bus, 0x14);
    let mut output = Vec::new();
    for index in 0..7 {
        output.push(bus.read8(0x0300_0000 + index));
    }
    assert_eq!(&output, b"BBBBBXY");
}

#[test]
fn eight_bit_unfilter_accumulates_differences() {
    let (mut cpu, mut bus) = system();
    load(&mut bus, 0x0200_0000, &[0x81, 4, 0, 0, 1, 1, 2, 3]);
    cpu.r[0] = 0x0200_0000;
    cpu.r[1] = 0x0300_0000;
    cpu.software_interrupt(&mut bus, 0x16);
    assert_eq!(bus.read8(0x0300_0000), 1);
    assert_eq!(bus.read8(0x0300_0001), 2);
    assert_eq!(bus.read8(0x0300_0002), 4);
    assert_eq!(bus.read8(0x0300_0003), 7);
}

#[test]
fn object_affine_set_builds_a_rotation_matrix() {
    let (mut cpu, mut bus) = system();
    bus.write16(0x0200_0000, 0x0100); // scale x = 1.0
    bus.write16(0x0200_0002, 0x0100); // scale y = 1.0
    bus.write16(0x0200_0004, 0x0000); // no rotation
    cpu.r[0] = 0x0200_0000;
    cpu.r[1] = 0x0300_0000;
    cpu.r[2] = 1;
    cpu.r[3] = 2;
    cpu.software_interrupt(&mut bus, 0x0f);
    assert_eq!(bus.read16(0x0300_0000), 0x0100, "pa");
    assert_eq!(bus.read16(0x0300_0002), 0x0000, "pb");
    assert_eq!(bus.read16(0x0300_0004), 0x0000, "pc");
    assert_eq!(bus.read16(0x0300_0006), 0x0100, "pd");
}

#[test]
fn register_ram_reset_clears_the_selected_regions() {
    let (mut cpu, mut bus) = system();
    bus.write8(0x0200_0000, 1);
    bus.write8(0x0300_0000, 2);
    bus.write16(0x0600_0000, 3);
    bus.write16(0x0700_0000, 4);
    cpu.r[0] = 0x1f;
    cpu.software_interrupt(&mut bus, 0x01);
    assert_eq!(bus.read8(0x0200_0000), 0);
    assert_eq!(bus.read8(0x0300_0000), 0);
    assert_eq!(bus.read16(0x0600_0000), 0);
    assert_eq!(bus.read16(0x0700_0000), 0);
}

#[test]
fn vblank_interrupt_wait_sleeps_until_the_flag_is_set() {
    let mut gba = Gba::new(vec![0; 0x200], None);
    gba.cpu.software_interrupt(&mut gba.bus, 0x05);
    assert!(gba.cpu.halted, "the call parks the processor");

    // Nothing wakes it while the BIOS flag stays clear.
    for _ in 0..100 {
        let cycles = gba.cpu.step(&mut gba.bus);
        gba.bus.run_cycles(cycles);
    }
    assert!(gba.cpu.halted);

    // A handler setting the vblank bit in the BIOS mirror releases it.
    gba.bus.set_bios_irq_flags(1);
    let cycles = gba.cpu.step(&mut gba.bus);
    gba.bus.run_cycles(cycles);
    assert!(!gba.cpu.halted);
    assert_eq!(gba.bus.bios_irq_flags(), 0, "the flag is consumed");
}

#[test]
fn the_bios_region_is_protected_from_outside_reads() {
    let mut gba = Gba::new(vec![0; 0x200], None);
    // Running from the cartridge, the BIOS reads back as the last word it
    // prefetched rather than its real contents.
    let value = gba.bus.read32(0x0000_0000);
    assert_eq!(value, 0xe129_f000);
}
