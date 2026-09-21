//! Runs a ROM until the program counter leaves mapped memory, then dumps the
//! instructions that led there. Set FEAR_GBA_TRACE_MODES to log processor mode
//! changes instead, which is how banking bugs show themselves.

use fear_gba::Gba;
use std::{collections::VecDeque, env, fs};

fn valid(pc: u32) -> bool {
    (pc < 0x4000) || matches!(pc >> 24, 0x02 | 0x03 | 0x08..=0x0d)
}

fn main() {
    let mut arguments = env::args().skip(1);
    let path = arguments.next().expect("usage: trace <rom.gba> [cycles]");
    let budget: u64 = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(40_000_000);
    let rom = fs::read(&path).unwrap();
    let mut gba = Gba::new(rom, None);

    let log_modes = std::env::var_os("FEAR_GBA_TRACE_MODES").is_some();
    let mut mode = gba.cpu.mode;
    let mut history: VecDeque<String> = VecDeque::with_capacity(24);
    let mut elapsed = 0u64;
    while elapsed < budget {
        let pc = gba.cpu.pc();
        if !valid(pc) {
            println!("left mapped memory at pc={pc:08x}");
            for line in &history {
                println!("{line}");
            }
            return;
        }
        let thumb = gba.cpu.thumb;
        let opcode = if thumb {
            gba.bus.read16(pc & !1) as u32
        } else {
            gba.bus.read32(pc & !3)
        };
        if history.len() == 24 {
            history.pop_front();
        }
        history.push_back(format!(
            "{}{pc:08x}: {opcode:08x}  r0={:08x} r1={:08x} r2={:08x} r3={:08x} r4={:08x} sp={:08x} lr={:08x} cpsr={:08x}",
            if thumb { "T " } else { "A " },
            gba.cpu.r[0],
            gba.cpu.r[1],
            gba.cpu.r[2],
            gba.cpu.r[3],
            gba.cpu.r[4],
            gba.cpu.r[13],
            gba.cpu.r[14],
            gba.cpu.cpsr()
        ));
        let cycles = gba.cpu.step(&mut gba.bus);
        gba.bus.run_cycles(cycles);
        elapsed += cycles as u64;
        if log_modes && gba.cpu.mode != mode {
            println!(
                "{pc:08x}: {opcode:08x}  mode {mode:02x} -> {:02x}  sp={:08x}",
                gba.cpu.mode, gba.cpu.r[13]
            );
            mode = gba.cpu.mode;
        }
    }
    println!("ran {elapsed} cycles without leaving mapped memory; pc={:08x}", gba.cpu.r[15]);
}
