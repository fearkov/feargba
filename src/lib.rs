//! fear-gba: a Game Boy Advance emulator.

pub mod apu;
pub mod bios;
pub mod bus;
pub mod cart;
pub mod cpu;
pub mod dma;
pub mod png;
pub mod ppu;
pub mod timer;

pub use bus::Bus;
pub use cpu::Cpu;
pub use ppu::{HEIGHT, WIDTH};

/// 4 cycles per dot, 308 dots per line, 228 lines.
pub const CYCLES_PER_FRAME: u32 = 280_896;

pub const KEY_A: u16 = 1 << 0;
pub const KEY_B: u16 = 1 << 1;
pub const KEY_SELECT: u16 = 1 << 2;
pub const KEY_START: u16 = 1 << 3;
pub const KEY_RIGHT: u16 = 1 << 4;
pub const KEY_LEFT: u16 = 1 << 5;
pub const KEY_UP: u16 = 1 << 6;
pub const KEY_DOWN: u16 = 1 << 7;
pub const KEY_R: u16 = 1 << 8;
pub const KEY_L: u16 = 1 << 9;

pub struct Gba {
    pub cpu: Cpu,
    pub bus: Bus,
}

impl Gba {
    /// Builds a system. Without a BIOS image the high level BIOS is used and
    /// the CPU starts directly at the cartridge entry point.
    pub fn new(rom: Vec<u8>, bios: Option<Vec<u8>>) -> Self {
        let real_bios = bios.is_some();
        let image = match bios {
            Some(mut image) => {
                image.resize(0x4000, 0);
                image
            }
            None => bios::hle_image(),
        };
        let mut bus = Bus::new(rom, image);
        bus.use_hle_bios = !real_bios;
        let mut cpu = Cpu::new();
        if real_bios {
            cpu.reset();
        } else {
            cpu.skip_bios();
            bus.postflg = 1;
        }
        Self { cpu, bus }
    }

    /// Runs until the PPU finishes a frame.
    pub fn run_frame(&mut self) {
        self.bus.frame_ready = false;
        let mut guard = 0u32;
        while !self.bus.frame_ready {
            let cycles = self.cpu.step(&mut self.bus);
            self.bus.run_cycles(cycles);
            guard += cycles;
            if guard > CYCLES_PER_FRAME * 4 {
                break;
            }
        }
    }

    /// Runs a fixed number of cycles, for tests and diagnostics.
    pub fn run_cycles(&mut self, cycles: u32) {
        let mut elapsed = 0;
        while elapsed < cycles {
            let step = self.cpu.step(&mut self.bus);
            self.bus.run_cycles(step);
            elapsed += step;
        }
    }

    /// `keys` holds the pressed buttons; KEYINPUT itself is active low.
    pub fn set_keys(&mut self, keys: u16) {
        self.bus.keyinput = !keys & 0x03ff;
    }

    pub fn framebuffer(&self) -> &[u32] {
        &self.bus.ppu.framebuffer
    }

    pub fn take_audio(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.bus.apu.buffer)
    }
}
