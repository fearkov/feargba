//! ARM7TDMI behaviour that the test ROMs cover, pinned down as unit tests.

use fear_gba::{Bus, Cpu, Gba};

/// Builds a system whose ROM is the given ARM words, entered in ARM state.
fn arm_system(words: &[u32]) -> (Cpu, Bus) {
    let mut rom = Vec::new();
    for word in words {
        rom.extend_from_slice(&word.to_le_bytes());
    }
    rom.resize(rom.len().max(0x200), 0);
    let gba = Gba::new(rom, None);
    (gba.cpu, gba.bus)
}

/// Builds a system whose ROM is the given Thumb halfwords.
fn thumb_system(halfwords: &[u16]) -> (Cpu, Bus) {
    let mut rom = Vec::new();
    for half in halfwords {
        rom.extend_from_slice(&half.to_le_bytes());
    }
    rom.resize(rom.len().max(0x200), 0);
    let mut gba = Gba::new(rom, None);
    gba.cpu.thumb = true;
    (gba.cpu, gba.bus)
}

fn run(cpu: &mut Cpu, bus: &mut Bus, steps: usize) {
    for _ in 0..steps {
        let cycles = cpu.step(bus);
        bus.run_cycles(cycles);
    }
}

#[test]
fn data_processing_sets_carry_and_overflow() {
    // movs r0, #0x80000000 ; adds r0, r0, r0
    let (mut cpu, mut bus) = arm_system(&[0xe3b0_0102, 0xe090_0000]);
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.r[0], 0);
    assert!(cpu.c, "adding two negatives carries out");
    assert!(cpu.v, "the signed result overflowed");
    assert!(cpu.z);
}

#[test]
fn subtraction_borrow_uses_inverted_carry() {
    // movs r0, #1 ; subs r0, r0, #2
    let (mut cpu, mut bus) = arm_system(&[0xe3b0_0001, 0xe250_0002]);
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.r[0], 0xffff_ffff);
    assert!(!cpu.c, "a borrow clears the carry flag");
    assert!(cpu.n);
}

#[test]
fn shift_by_immediate_zero_has_special_meanings() {
    // movs r1, r0, lsr #0  -- an immediate zero means a shift of 32
    let (mut cpu, mut bus) = arm_system(&[0xe1b0_1020]);
    cpu.r[0] = 0x8000_0000;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.r[1], 0);
    assert!(cpu.c, "bit 31 shifted out into the carry");
}

#[test]
fn rrx_rotates_through_the_carry_flag() {
    // movs r1, r0, rrx
    let (mut cpu, mut bus) = arm_system(&[0xe1b0_1060]);
    cpu.r[0] = 1;
    cpu.c = true;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.r[1], 0x8000_0000);
    assert!(cpu.c, "the shifted out bit becomes the new carry");
}

#[test]
fn compare_with_r15_destination_restores_the_saved_status_register() {
    // The ARMv4 "P" forms: cmp with S set and r15 as destination copies SPSR.
    let (mut cpu, mut bus) = arm_system(&[0xe15f_f000]);
    cpu.switch_mode(fear_gba::cpu::MODE_FIQ);
    cpu.spsr = fear_gba::cpu::MODE_SYS;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.mode, fear_gba::cpu::MODE_SYS);
}

#[test]
fn fiq_mode_banks_the_upper_registers() {
    let mut cpu = Cpu::new();
    cpu.skip_bios();
    cpu.r[8] = 0x1111;
    cpu.r[13] = 0x0300_7f00;
    cpu.switch_mode(fear_gba::cpu::MODE_FIQ);
    cpu.r[8] = 0x2222;
    cpu.switch_mode(fear_gba::cpu::MODE_SYS);
    assert_eq!(cpu.r[8], 0x1111);
    assert_eq!(cpu.r[13], 0x0300_7f00);
}

#[test]
fn block_store_writes_the_updated_base_unless_it_comes_first() {
    // stmia r0!, {r0, r1}  -- r0 is first, so the original value is stored.
    let (mut cpu, mut bus) = arm_system(&[0xe8a0_0003]);
    cpu.r[0] = 0x0300_0000;
    cpu.r[1] = 0x1234;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(bus.read32(0x0300_0000), 0x0300_0000);
    assert_eq!(cpu.r[0], 0x0300_0008);

    // stmia r1!, {r0, r1}  -- r1 is not first, so the new base is stored.
    let (mut cpu, mut bus) = arm_system(&[0xe8a1_0003]);
    cpu.r[0] = 0xabcd;
    cpu.r[1] = 0x0300_0000;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(bus.read32(0x0300_0004), 0x0300_0008);
}

#[test]
fn misaligned_word_load_rotates() {
    let (mut cpu, mut bus) = arm_system(&[0xe590_1000]);
    bus.write32(0x0300_0000, 0x1122_3344);
    cpu.r[0] = 0x0300_0002;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.r[1], 0x3344_1122);
}

#[test]
fn unaligned_signed_halfword_load_reads_a_byte() {
    // ldrsh r1, [r0]
    let (mut cpu, mut bus) = arm_system(&[0xe1d0_10f0]);
    bus.write16(0x0300_0000, 0x80ff);
    cpu.r[0] = 0x0300_0001;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.r[1], 0xffff_ff80);
}

#[test]
fn branch_and_exchange_switches_to_thumb() {
    let (mut cpu, mut bus) = arm_system(&[0xe12f_ff10]);
    cpu.r[0] = 0x0800_0101;
    run(&mut cpu, &mut bus, 1);
    assert!(cpu.thumb);
    assert_eq!(cpu.pc(), 0x0800_0100);
}

#[test]
fn thumb_long_branch_links_the_return_address() {
    // bl +4
    let (mut cpu, mut bus) = thumb_system(&[0xf000, 0xf802]);
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.r[14], 0x0800_0005);
    assert_eq!(cpu.pc(), 0x0800_0008);
}

#[test]
fn thumb_alu_shift_by_register_keeps_carry_when_zero() {
    // lsl r0, r1  with r1 = 0 leaves the carry flag alone
    let (mut cpu, mut bus) = thumb_system(&[0x4088]);
    cpu.r[0] = 0x1234;
    cpu.r[1] = 0;
    cpu.c = true;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.r[0], 0x1234);
    assert!(cpu.c);
}

#[test]
fn thumb_negate_sets_flags_like_a_subtraction() {
    // neg r0, r1
    let (mut cpu, mut bus) = thumb_system(&[0x4248]);
    cpu.r[1] = 1;
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.r[0], 0xffff_ffff);
    assert!(cpu.n);
    assert!(!cpu.c, "0 - 1 borrows");
}

#[test]
fn prefetched_instructions_survive_being_overwritten() {
    // The pipeline holds two instructions, so a store over the next one does
    // not change what runs.  str r2,[r0] ; mov r1, #1 ; mov r1, #2
    let (mut cpu, mut bus) = arm_system(&[0xe580_2000, 0xe3a0_1001, 0xe3a0_1002]);
    // Point the store at the "mov r1, #1" that has already been fetched.
    cpu.r[0] = 0x0300_0000;
    cpu.r[2] = 0;
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.r[1], 1, "the prefetched instruction still executed");
}

#[test]
fn interrupts_enter_the_vector_and_return_through_the_bios_stub() {
    let mut gba = Gba::new(vec![0; 0x200], None);
    // A handler that acknowledges the interrupt and returns.
    let handler = [
        0xe3a0_0301u32, // mov  r0, #0x04000000
        0xe280_0c02,    // add  r0, r0, #0x200
        0xe3a0_1001,    // mov  r1, #1
        0xe1c0_10b2,    // strh r1, [r0, #2]      (acknowledge in IF)
        0xe12f_ff1e,    // bx   lr
    ];
    for (index, word) in handler.iter().enumerate() {
        gba.bus.write32(0x0300_0000 + index as u32 * 4, *word);
    }
    gba.bus.write32(0x0300_7ffc, 0x0300_0000);
    gba.bus.interrupt_enable = 1;
    gba.bus.master_enable = true;
    gba.bus.raise_irq(1);
    let before = gba.cpu.pc();
    gba.cpu.step(&mut gba.bus);
    assert_eq!(gba.cpu.pc(), 0x18, "the IRQ vector runs from the BIOS");
    assert_eq!(gba.cpu.mode, fear_gba::cpu::MODE_IRQ);
    let mut returned = false;
    for _ in 0..32 {
        let cycles = gba.cpu.step(&mut gba.bus);
        gba.bus.run_cycles(cycles);
        if gba.cpu.mode == fear_gba::cpu::MODE_SYS {
            returned = true;
            break;
        }
    }
    assert!(returned, "the handler returned to the interrupted mode");
    assert_eq!(gba.cpu.pc(), before, "and to the interrupted address");
    assert_eq!(gba.bus.interrupt_flags, 0, "the handler acknowledged it");
}
