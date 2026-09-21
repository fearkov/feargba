//! Optional tests against real cartridges.
//!
//! Set `FEAR_GBA_TEST_SUITE` to a checkout of jsmolka's gba-tests to run the
//! hardware conformance ROMs, and `FEAR_GBA_TEST_ROM` to a game to smoke test
//! booting it. Both are skipped when the variable is unset.

use fear_gba::Gba;
use std::{env, fs, path::Path};

/// Those ROMs leave the number of the first failed test in r12, or zero.
fn failing_test_number(path: &Path, frames: u32) -> u32 {
    let rom = fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut gba = Gba::new(rom, None);
    for _ in 0..frames {
        gba.run_frame();
        gba.bus.apu.buffer.clear();
    }
    gba.cpu.r[12]
}

#[test]
fn hardware_conformance_roms_pass() {
    let Some(root) = env::var_os("FEAR_GBA_TEST_SUITE") else {
        eprintln!("skipped: set FEAR_GBA_TEST_SUITE to a gba-tests checkout");
        return;
    };
    let root = Path::new(&root);
    let roms = [
        "arm/arm.gba",
        "thumb/thumb.gba",
        "memory/memory.gba",
        "bios/bios.gba",
        "nes/nes.gba",
        "unsafe/unsafe.gba",
        "save/sram.gba",
        "save/flash64.gba",
        "save/flash128.gba",
        "save/none.gba",
    ];
    let mut failures = Vec::new();
    for name in roms {
        let path = root.join(name);
        if !path.exists() {
            continue;
        }
        let result = failing_test_number(&path, 150);
        if result != 0 {
            failures.push(format!("{name}: failed test {result}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join(", "));
}

#[test]
fn a_real_game_boots_and_draws() {
    let Some(path) = env::var_os("FEAR_GBA_TEST_ROM") else {
        eprintln!("skipped: set FEAR_GBA_TEST_ROM to a .gba file");
        return;
    };
    let rom = fs::read(&path).expect("test rom");
    let mut gba = Gba::new(rom, None);
    for _ in 0..600 {
        gba.run_frame();
        gba.bus.apu.buffer.clear();
    }
    let frame = gba.framebuffer();
    let first = frame[0];
    assert!(
        frame.iter().any(|pixel| *pixel != first),
        "the game drew nothing after ten seconds"
    );
}
