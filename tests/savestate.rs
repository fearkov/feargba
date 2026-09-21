//! Save states have to restore the machine exactly.

use fear_gba::Gba;
use std::{env, fs};

fn rom() -> Option<Vec<u8>> {
    let path = env::var_os("FEAR_GBA_TEST_ROM")?;
    fs::read(path).ok()
}

#[test]
fn a_state_round_trips_to_the_same_future() {
    let Some(image) = rom() else {
        eprintln!("skipped: set FEAR_GBA_TEST_ROM to a .gba file");
        return;
    };

    let mut gba = Gba::new(image.clone(), None);
    for _ in 0..400 {
        gba.run_frame();
        gba.bus.apu.buffer.clear();
    }
    let state = gba.save_state();

    // Keep running, then rewind and run the same frames again.
    for _ in 0..60 {
        gba.run_frame();
        gba.bus.apu.buffer.clear();
    }
    let expected: Vec<u32> = gba.framebuffer().to_vec();
    let expected_pc = gba.cpu.pc();

    gba.load_state(&state).expect("load");
    for _ in 0..60 {
        gba.run_frame();
        gba.bus.apu.buffer.clear();
    }
    assert_eq!(gba.cpu.pc(), expected_pc, "diverged from the captured run");
    assert_eq!(gba.framebuffer(), expected.as_slice());
}

#[test]
fn a_state_loads_into_a_fresh_machine() {
    let Some(image) = rom() else {
        eprintln!("skipped: set FEAR_GBA_TEST_ROM to a .gba file");
        return;
    };

    let mut source = Gba::new(image.clone(), None);
    for _ in 0..400 {
        source.run_frame();
        source.bus.apu.buffer.clear();
    }
    let state = source.save_state();
    for _ in 0..30 {
        source.run_frame();
        source.bus.apu.buffer.clear();
    }

    let mut target = Gba::new(image, None);
    target.load_state(&state).expect("load");
    for _ in 0..30 {
        target.run_frame();
        target.bus.apu.buffer.clear();
    }
    assert_eq!(target.framebuffer(), source.framebuffer());
}

#[test]
fn a_state_from_another_game_is_rejected() {
    let mut first = Gba::new(vec![0x11; 0x400], None);
    let state = first.save_state();
    let mut second = Gba::new(vec![0x22; 0x400], None);
    assert!(second.load_state(&state).is_err());
    assert!(first.load_state(&state).is_ok());
}

#[test]
fn rubbish_is_rejected_rather_than_panicking() {
    let mut gba = Gba::new(vec![0; 0x400], None);
    assert!(gba.load_state(b"not a state at all").is_err());
    let truncated = gba.save_state()[..20].to_vec();
    assert!(gba.load_state(&truncated).is_err());
}
