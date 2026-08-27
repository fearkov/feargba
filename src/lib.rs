pub const WIDTH: usize = 240;
pub const HEIGHT: usize = 160;

mod dma;
mod timer;
use dma::Dma;
use timer::Timer;

const BIOS_SIZE: usize = 0x4000;
const EWRAM_SIZE: usize = 0x40000;
const IWRAM_SIZE: usize = 0x8000;
const IO_SIZE: usize = 0x400;
const PALETTE_SIZE: usize = 0x400;
const VRAM_SIZE: usize = 0x18000;
const OAM_SIZE: usize = 0x400;

pub struct Bus {
    pub bios_loaded: bool,
    pub bios: Vec<u8>,
    pub ewram: Vec<u8>,
    pub iwram: Vec<u8>,
    pub io: Vec<u8>,
    pub palette: Vec<u8>,
    pub vram: Vec<u8>,
    pub oam: Vec<u8>,
    pub rom: Vec<u8>,
    pub save: Vec<u8>,
    timers: [Timer; 4],
    dmas: [Dma; 4],
    ppu_cycles: u32,
}

impl Bus {
    pub fn new(rom: Vec<u8>, bios: Option<Vec<u8>>) -> Self {
        let mut bus = Self {
            bios_loaded: false,
            bios: vec![0; BIOS_SIZE],
            ewram: vec![0; EWRAM_SIZE],
            iwram: vec![0; IWRAM_SIZE],
            io: vec![0; IO_SIZE],
            palette: vec![0; PALETTE_SIZE],
            vram: vec![0; VRAM_SIZE],
            oam: vec![0; OAM_SIZE],
            rom,
            save: vec![0xff; 0x10000],
            timers: [Timer::default(); 4],
            dmas: [Dma::default(); 4],
            ppu_cycles: 0,
        };
        if let Some(image) = bios {
            let len = image.len().min(BIOS_SIZE);
            bus.bios[..len].copy_from_slice(&image[..len]);
            bus.bios_loaded = len == BIOS_SIZE;
        }
        bus.io[0x00] = 0x80;
        bus.io[0x01] = 0x00;
        bus.io[0x04] = 0x00;
        bus.io[0x08] = 0xff;
        bus.io[0x09] = 0x03;
        bus
    }

    pub fn read8(&self, address: u32) -> u8 {
        match address >> 24 {
            0x00 => self.bios[(address as usize) & (BIOS_SIZE - 1)],
            0x02 => self.ewram[(address as usize) & (EWRAM_SIZE - 1)],
            0x03 => self.iwram[(address as usize) & (IWRAM_SIZE - 1)],
            0x04 => self.io[(address as usize) & (IO_SIZE - 1)],
            0x05 => self.palette[(address as usize) & (PALETTE_SIZE - 1)],
            0x06 => self.vram[vram_index(address)],
            0x07 => self.oam[(address as usize) & (OAM_SIZE - 1)],
            0x08..=0x0d => self
                .rom
                .get((address as usize - 0x08000000) % self.rom.len().max(1))
                .copied()
                .unwrap_or(0xff),
            0x0e..=0x0f => self.save[(address as usize) & 0xffff],
            _ => 0,
        }
    }

    pub fn read16(&self, address: u32) -> u16 {
        u16::from_le_bytes([self.read8(address), self.read8(address + 1)])
    }
    pub fn read32(&self, address: u32) -> u32 {
        u32::from_le_bytes([
            self.read8(address),
            self.read8(address + 1),
            self.read8(address + 2),
            self.read8(address + 3),
        ])
    }

    pub fn write8(&mut self, address: u32, value: u8) {
        match address >> 24 {
            0x02 => self.ewram[(address as usize) & (EWRAM_SIZE - 1)] = value,
            0x03 => self.iwram[(address as usize) & (IWRAM_SIZE - 1)] = value,
            0x04 => {
                let offset = (address as usize) & (IO_SIZE - 1);
                if offset == 0x202 {
                    self.io[offset] &= !value;
                } else {
                    self.io[offset] = value;
                }
                if (0x100..0x110).contains(&offset) {
                    let index = (offset - 0x100) / 4;
                    if offset & 3 < 2 {
                        self.timers[index].reload = u16::from_le_bytes([
                            self.io[index * 4 + 0x100],
                            self.io[index * 4 + 0x101],
                        ]);
                    } else {
                        self.timers[index].control = u16::from_le_bytes([
                            self.io[index * 4 + 0x102],
                            self.io[index * 4 + 0x103],
                        ]);
                        if value & 0x80 != 0 {
                            self.timers[index].counter = self.timers[index].reload;
                        }
                    }
                }
                if (0xb0..0xe0).contains(&offset) {
                    let channel = (offset - 0xb0) / 12;
                    let register = (offset - 0xb0) % 12;
                    let dma = &mut self.dmas[channel];
                    match register {
                        0..=3 => {
                            dma.source = (dma.source & !(0xff << (register * 8)))
                                | (value as u32) << (register * 8)
                        }
                        4..=7 => {
                            dma.destination = (dma.destination & !(0xff << ((register - 4) * 8)))
                                | (value as u32) << ((register - 4) * 8)
                        }
                        8 => dma.count = (dma.count & 0xff00) | value as u16,
                        9 => dma.count = (dma.count & 0x00ff) | (value as u16) << 8,
                        10 => dma.control = (dma.control & 0xff00) | value as u16,
                        11 => {
                            dma.control = (dma.control & 0x00ff) | (value as u16) << 8;
                            if dma.control & 0x8000 != 0 && dma.control & 3 == 0 {
                                self.run_dma(channel);
                            }
                        }
                        _ => {}
                    }
                }
            }
            0x05 => self.palette[(address as usize) & (PALETTE_SIZE - 1)] = value,
            0x06 => self.vram[vram_index(address)] = value,
            0x07 => self.oam[(address as usize) & (OAM_SIZE - 1)] = value,
            0x0e..=0x0f => self.save[(address as usize) & 0xffff] = value,
            _ => {}
        }
    }

    fn run_dma(&mut self, channel: usize) {
        let dma = self.dmas[channel];
        let count = if dma.count == 0 {
            if channel == 3 {
                0x10000
            } else {
                0x4000
            }
        } else {
            dma.count as u32
        };
        let width = if dma.control & (1 << 10) != 0 { 4 } else { 2 };
        let source_step = if dma.control & (1 << 7) != 0 {
            0
        } else if dma.control & (1 << 9) != 0 {
            -(width as i32)
        } else {
            width as i32
        };
        let destination_step = if dma.control & (1 << 6) != 0 {
            0
        } else if dma.control & (1 << 5) != 0 {
            -(width as i32)
        } else {
            width as i32
        };
        let mut source = dma.source;
        let mut destination = dma.destination;
        for _ in 0..count {
            if width == 4 {
                let value = self.read32(source);
                self.write32(destination, value);
            } else {
                let value = self.read16(source);
                self.write16(destination, value);
            }
            source = source.wrapping_add_signed(source_step);
            destination = destination.wrapping_add_signed(destination_step);
        }
        self.dmas[channel].source = source;
        self.dmas[channel].destination = destination;
        self.dmas[channel].control &= !0x8000;
    }

    pub fn write16(&mut self, address: u32, value: u16) {
        let bytes = value.to_le_bytes();
        self.write8(address, bytes[0]);
        self.write8(address + 1, bytes[1]);
    }
    pub fn write32(&mut self, address: u32, value: u32) {
        let bytes = value.to_le_bytes();
        for (offset, byte) in bytes.into_iter().enumerate() {
            self.write8(address + offset as u32, byte);
        }
    }

    pub fn set_keypad(&mut self, pressed: u16) {
        let value = !pressed & 0x03ff;
        let bytes = value.to_le_bytes();
        self.io[0x130] = bytes[0];
        self.io[0x131] = bytes[1];
    }

    pub fn load_save(&mut self, data: &[u8]) {
        let length = data.len().min(self.save.len());
        self.save[..length].copy_from_slice(&data[..length]);
    }

    pub fn tick(&mut self, cycles: u32) {
        for index in 0..4 {
            let timer = &mut self.timers[index];
            if timer.control & 0x80 == 0 || (index > 0 && timer.control & 0x04 != 0) {
                continue;
            }
            timer.prescaler += cycles;
            let divider = match timer.control & 3 {
                0 => 1,
                1 => 64,
                2 => 256,
                _ => 1024,
            };
            while timer.prescaler >= divider {
                timer.prescaler -= divider;
                let (value, overflow) = timer.counter.overflowing_add(1);
                timer.counter = if overflow { timer.reload } else { value };
                if overflow && timer.control & 0x40 != 0 {
                    self.io[0x202] |= 1 << (3 + index);
                }
            }
            let address = 0x100 + index * 4;
            let bytes = timer.counter.to_le_bytes();
            self.io[address] = bytes[0];
            self.io[address + 1] = bytes[1];
        }
        self.ppu_cycles += cycles;
        while self.ppu_cycles >= 1232 {
            self.ppu_cycles -= 1232;
            self.io[6] = self.io[6].wrapping_add(1) % 228;
            let vcount = self.io[6];
            let compare = self.io[9];
            if vcount == 160 && self.io[5] & 1 << 3 != 0 {
                self.io[0x202] |= 1 << 0;
            }
            if vcount == compare && self.io[5] & 1 << 5 != 0 {
                self.io[0x202] |= 1 << 2;
            }
        }
        let vcount = self.io[6];
        self.io[4] = (self.io[4] & !7)
            | if vcount >= 160 { 1 } else { 0 }
            | if self.ppu_cycles >= 1006 { 2 } else { 0 }
            | if vcount == self.io[9] { 4 } else { 0 };
    }

    pub fn framebuffer(&self) -> Vec<u32> {
        let mode = self.io[0] & 0x07;
        if mode == 3 {
            return self.render_mode3();
        }
        if mode == 4 {
            return self.render_mode4();
        }
        if mode == 5 {
            return self.render_mode5();
        }
        if mode == 0 {
            self.render_mode0()
        } else {
            vec![0; WIDTH * HEIGHT]
        }
    }

    fn render_mode3(&self) -> Vec<u32> {
        let mut frame = vec![0; WIDTH * HEIGHT];
        for (index, pixel) in frame.iter_mut().enumerate() {
            *pixel = color_to_rgb(self.read_vram16(index * 2));
        }
        frame
    }

    fn render_mode4(&self) -> Vec<u32> {
        let mut frame = vec![0; WIDTH * HEIGHT];
        let page = if self.io[0] & 0x10 != 0 { 0xa000 } else { 0 };
        for (index, pixel) in frame.iter_mut().enumerate() {
            let palette_index = self.vram[page + index];
            *pixel = color_to_rgb(self.read_palette16(palette_index as usize * 2));
        }
        frame
    }

    fn render_mode5(&self) -> Vec<u32> {
        let mut frame = vec![0; WIDTH * HEIGHT];
        let page = if self.io[0] & 0x10 != 0 { 0xa000 } else { 0 };
        for y in 0..128 {
            for x in 0..160 {
                let source = page + (y * 160 + x) * 2;
                let color = u16::from_le_bytes([self.vram[source], self.vram[source + 1]]);
                frame[(y + 16) * WIDTH + x + 40] = color_to_rgb(color);
            }
        }
        frame
    }

    fn render_mode0(&self) -> Vec<u32> {
        let mut frame = vec![0; WIDTH * HEIGHT];
        let control = u16::from_le_bytes([self.io[0], self.io[1]]);
        if control & 0x0100 == 0 {
            return frame;
        }
        let bg_control = u16::from_le_bytes([self.io[8], self.io[9]]);
        let char_base = ((bg_control >> 2) & 3) as usize * 0x4000;
        let screen_base = ((bg_control >> 8) & 0x1f) as usize * 0x800;
        let color_256 = bg_control & (1 << 7) != 0;
        let scroll_x = u16::from_le_bytes([self.io[0x10], self.io[0x11]]) as usize;
        let scroll_y = u16::from_le_bytes([self.io[0x12], self.io[0x13]]) as usize;
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let world_x = (x + scroll_x) & 511;
                let world_y = (y + scroll_y) & 511;
                let tile_x = world_x / 8;
                let tile_y = world_y / 8;
                let map_offset = screen_base + (tile_y % 32) * 64 + (tile_x % 32) * 2;
                let entry = self.read_vram16(map_offset);
                let tile = (entry & 0x03ff) as usize;
                let tile_x_in = world_x & 7;
                let tile_y_in = world_y & 7;
                let palette_index = if color_256 {
                    self.vram[char_base + tile * 64 + tile_y_in * 8 + tile_x_in]
                } else {
                    let packed = self.vram[char_base + tile * 32 + tile_y_in * 4 + tile_x_in / 2];
                    let nibble = if tile_x_in & 1 == 0 {
                        packed & 0x0f
                    } else {
                        packed >> 4
                    };
                    nibble + ((entry >> 12) as u8 & 0x0f) * 16
                };
                *frame.get_mut(y * WIDTH + x).unwrap() =
                    color_to_rgb(self.read_palette16(palette_index as usize * 2));
            }
        }
        frame
    }

    fn read_vram16(&self, offset: usize) -> u16 {
        let offset = offset % VRAM_SIZE;
        u16::from_le_bytes([self.vram[offset], self.vram[(offset + 1) % VRAM_SIZE]])
    }

    fn read_palette16(&self, offset: usize) -> u16 {
        u16::from_le_bytes([
            self.palette[offset % PALETTE_SIZE],
            self.palette[(offset + 1) % PALETTE_SIZE],
        ])
    }
}

fn color_to_rgb(color: u16) -> u32 {
    let r = ((color & 0x1f) as u32) * 255 / 31;
    let g = (((color >> 5) & 0x1f) as u32) * 255 / 31;
    let b = (((color >> 10) & 0x1f) as u32) * 255 / 31;
    (r << 16) | (g << 8) | b
}

fn vram_index(address: u32) -> usize {
    let offset = (address as usize) & 0x1ffff;
    if offset >= 0x18000 {
        offset & 0x17fff
    } else {
        offset
    }
}

const N: u32 = 1 << 31;
const Z: u32 = 1 << 30;
const C: u32 = 1 << 29;
const V: u32 = 1 << 28;
const T: u32 = 1 << 5;

pub struct Cpu {
    pub r: [u32; 16],
    pub cpsr: u32,
    pub halted: bool,
    pub last_pc: u32,
    pub last_instruction: u32,
    pub unsupported: u64,
    pub first_unsupported_pc: Option<u32>,
    pub first_unsupported_instruction: u32,
    pub first_thumb_pc: Option<u32>,
    pub first_thumb_instruction: u32,
    pub first_arm_after_thumb_pc: Option<u32>,
    pub first_arm_after_thumb_instruction: u32,
    pub first_invalid_pc: Option<u32>,
    pub first_invalid_instruction: u32,
    pub last_valid_pc: u32,
    pub last_valid_instruction: u32,
    spsr: u32,
    thumb_bl_base: Option<u32>,
    mode: u8,
    banked_user: [u32; 2],
    banked_irq: [u32; 2],
    banked_svc: [u32; 2],
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            r: [0; 16],
            cpsr: 0xd3,
            halted: false,
            last_pc: 0,
            last_instruction: 0,
            unsupported: 0,
            first_unsupported_pc: None,
            first_unsupported_instruction: 0,
            first_thumb_pc: None,
            first_thumb_instruction: 0,
            first_arm_after_thumb_pc: None,
            first_arm_after_thumb_instruction: 0,
            first_invalid_pc: None,
            first_invalid_instruction: 0,
            last_valid_pc: 0,
            last_valid_instruction: 0,
            spsr: 0,
            thumb_bl_base: None,
            mode: 0x13,
            banked_user: [0; 2],
            banked_irq: [0; 2],
            banked_svc: [0; 2],
        }
    }
    pub fn reset(&mut self) {
        self.r = [0; 16];
        self.r[15] = 0x08000000;
        self.cpsr = 0xd3;
        self.halted = false;
        self.last_pc = 0;
        self.last_instruction = 0;
        self.unsupported = 0;
        self.first_unsupported_pc = None;
        self.first_unsupported_instruction = 0;
        self.first_thumb_pc = None;
        self.first_thumb_instruction = 0;
        self.first_arm_after_thumb_pc = None;
        self.first_arm_after_thumb_instruction = 0;
        self.first_invalid_pc = None;
        self.first_invalid_instruction = 0;
        self.last_valid_pc = 0;
        self.last_valid_instruction = 0;
        self.spsr = 0;
        self.thumb_bl_base = None;
        self.mode = 0x13;
        self.banked_user = [0; 2];
        self.banked_irq = [0; 2];
        self.banked_svc = [0; 2];
    }

    pub fn fast_boot(&mut self, bus: &mut Bus) {
        self.reset();
        self.r[13] = 0x0300_7f00;
        self.banked_irq[0] = 0x0300_7fa0;
        self.banked_svc[0] = 0x0300_7fe0;
        self.r[15] = 0x0800_0000;
        self.cpsr = 0x1f;
        bus.io[0x300] = 1;
        bus.io[0x130] = 0xff;
        bus.io[0x131] = 0x03;
    }

    pub fn step(&mut self, bus: &mut Bus) -> u32 {
        if self.halted && !self.irq_pending(bus) {
            bus.tick(1);
            return 1;
        }
        if self.irq_pending(bus) {
            self.enter_irq(bus);
        }
        let cycles = if self.cpsr & T != 0 {
            self.step_thumb(bus)
        } else {
            self.step_arm(bus)
        };
        bus.tick(cycles);
        cycles
    }

    fn irq_pending(&self, bus: &Bus) -> bool {
        self.cpsr & (1 << 7) == 0
            && bus.io[0x208] & 1 != 0
            && (u16::from_le_bytes([bus.io[0x200], bus.io[0x201]])
                & u16::from_le_bytes([bus.io[0x202], bus.io[0x203]]))
                != 0
    }

    fn enter_irq(&mut self, bus: &mut Bus) {
        self.save_banked_registers();
        self.spsr = self.cpsr;
        self.mode = 0x12;
        self.r[13] = self.banked_irq[0];
        self.r[14] = self.r[15].wrapping_add(if self.cpsr & T != 0 { 2 } else { 4 });
        self.banked_irq[1] = self.r[14];
        self.r[15] = 0x18;
        self.cpsr = (self.cpsr & !T) | (1 << 7) | 0x12;
        self.halted = false;
        bus.io[0x202] &= !(1 << 3);
    }

    fn save_banked_registers(&mut self) {
        let bank = match self.mode {
            0x12 => &mut self.banked_irq,
            0x13 => &mut self.banked_svc,
            _ => &mut self.banked_user,
        };
        bank[0] = self.r[13];
        bank[1] = self.r[14];
    }

    fn restore_mode(&mut self, mode: u8) {
        self.save_banked_registers();
        self.mode = mode;
        let bank = match mode {
            0x12 => self.banked_irq,
            0x13 => self.banked_svc,
            _ => self.banked_user,
        };
        self.r[13] = bank[0];
        self.r[14] = bank[1];
    }

    fn condition(&self, condition: u32) -> bool {
        let flags = self.cpsr;
        match condition {
            0 => flags & Z != 0,
            1 => flags & Z == 0,
            2 => flags & C != 0,
            3 => flags & C == 0,
            4 => flags & N != 0,
            5 => flags & N == 0,
            6 => flags & V != 0,
            7 => flags & V == 0,
            8 => flags & C != 0 && flags & Z == 0,
            9 => flags & C == 0 || flags & Z != 0,
            10 => (flags & N) == (flags & V),
            11 => (flags & N) != (flags & V),
            12 => flags & Z == 0 && (flags & N) == (flags & V),
            13 => flags & Z != 0 || (flags & N) != (flags & V),
            14 => true,
            _ => false,
        }
    }

    fn set_nz(&mut self, value: u32) {
        self.cpsr = (self.cpsr & !(N | Z))
            | if value & N != 0 { N } else { 0 }
            | if value == 0 { Z } else { 0 };
    }

    fn swi(&mut self, bus: &mut Bus, number: u8) {
        match number {
            0x00 => {
                self.r[15] = 0x0800_0000;
                self.cpsr = 0xd3;
                self.halted = false;
            }
            0x01 => self.register_ram_reset(bus),
            0x02 | 0x03 | 0x05 => self.halted = true,
            0x04 => self.halted = true,
            0x0b => self.cpu_set(bus, false),
            0x0c => self.cpu_set(bus, true),
            0x10 | 0x11 => self.lz77_decode(bus),
            0x13 | 0x14 => self.rl_decode(bus),
            0x06 => {
                let dividend = self.r[0] as i32;
                let divisor = self.r[1] as i32;
                if divisor != 0 {
                    self.r[0] = dividend.wrapping_div(divisor) as u32;
                    self.r[1] = dividend.wrapping_rem(divisor) as u32;
                    self.r[3] = divisor.unsigned_abs();
                }
            }
            _ => {}
        }
    }

    fn cpu_set(&mut self, bus: &mut Bus, fast: bool) {
        let mut source = self.r[0];
        let mut destination = self.r[1];
        let count = self.r[2] & 0x1f_ffff;
        let fill = self.r[2] & (1 << 24) != 0;
        let word = fast || self.r[2] & (1 << 26) != 0;
        let units = if fast { count * 8 } else { count };
        if word {
            let value = bus.read32(source);
            for _ in 0..units {
                bus.write32(destination, if fill { value } else { bus.read32(source) });
                source += 4;
                destination += 4;
            }
        } else {
            let value = bus.read16(source);
            for _ in 0..units {
                bus.write16(destination, if fill { value } else { bus.read16(source) });
                source += 2;
                destination += 2;
            }
        }
        self.r[0] = source;
        self.r[1] = destination;
    }

    fn register_ram_reset(&mut self, bus: &mut Bus) {
        let mask = self.r[0] as u8;
        if mask & 1 != 0 {
            bus.ewram.fill(0);
        }
        if mask & 2 != 0 {
            bus.iwram.fill(0);
        }
        if mask & 4 != 0 {
            bus.palette.fill(0);
        }
        if mask & 8 != 0 {
            bus.vram.fill(0);
        }
        if mask & 16 != 0 {
            bus.oam.fill(0);
        }
        if mask & 0x80 != 0 {
            bus.io.fill(0);
            bus.io[0x130] = 0xff;
            bus.io[0x131] = 0x03;
        }
    }

    fn lz77_decode(&mut self, bus: &mut Bus) {
        let mut source = self.r[0];
        let destination = self.r[1];
        if bus.read8(source) != 0x10 {
            return;
        }
        source += 1;
        let size = bus.read8(source) as usize
            | (bus.read8(source + 1) as usize) << 8
            | (bus.read8(source + 2) as usize) << 16;
        source += 3;
        let mut output = Vec::with_capacity(size);
        while output.len() < size {
            let flags = bus.read8(source);
            source += 1;
            for bit in (0..8).rev() {
                if output.len() >= size {
                    break;
                }
                if flags & (1 << bit) == 0 {
                    output.push(bus.read8(source));
                    source += 1;
                } else {
                    let first = bus.read8(source);
                    let second = bus.read8(source + 1);
                    source += 2;
                    let length = (first >> 4) as usize + 3;
                    let displacement = (((first as usize & 0x0f) << 8) | second as usize) + 1;
                    for _ in 0..length {
                        if output.len() >= size {
                            break;
                        }
                        let index = output.len().saturating_sub(displacement);
                        output.push(output.get(index).copied().unwrap_or(0));
                    }
                }
            }
        }
        for (index, byte) in output.into_iter().enumerate() {
            bus.write8(destination + index as u32, byte);
        }
    }

    fn rl_decode(&mut self, bus: &mut Bus) {
        let mut source = self.r[0];
        let destination = self.r[1];
        if bus.read8(source) != 0x30 {
            return;
        }
        let size = bus.read8(source + 1) as usize
            | (bus.read8(source + 2) as usize) << 8
            | (bus.read8(source + 3) as usize) << 16;
        source += 4;
        let mut output = Vec::with_capacity(size);
        while output.len() < size {
            let control = bus.read8(source);
            source += 1;
            let count = (control & 0x7f) as usize + 1;
            if control & 0x80 != 0 {
                let value = bus.read8(source);
                source += 1;
                for _ in 0..count {
                    output.push(value);
                }
            } else {
                for _ in 0..count {
                    output.push(bus.read8(source));
                    source += 1;
                }
            }
        }
        output.truncate(size);
        for (index, byte) in output.into_iter().enumerate() {
            bus.write8(destination + index as u32, byte);
        }
    }

    fn step_arm(&mut self, bus: &mut Bus) -> u32 {
        let address = self.r[15] & !3;
        self.r[15] = address;
        let instruction = bus.read32(address);
        if self.first_thumb_pc.is_some() && self.first_arm_after_thumb_pc.is_none() {
            self.first_arm_after_thumb_pc = Some(address);
            self.first_arm_after_thumb_instruction = instruction;
        }
        self.last_pc = address;
        self.last_instruction = instruction;
        if !valid_cpu_address(address) && self.first_invalid_pc.is_none() {
            self.first_invalid_pc = Some(address);
            self.first_invalid_instruction = instruction;
        } else if valid_cpu_address(address) {
            self.last_valid_pc = address;
            self.last_valid_instruction = instruction;
        }
        self.r[15] = address.wrapping_add(4);
        if instruction & 0xfe00_0000 == 0xfa00_0000 {
            let offset = (((instruction & 0x00ff_ffff) << 2) as i32) << 6 >> 6;
            let h = if instruction & (1 << 24) != 0 { 2 } else { 0 };
            self.r[14] = self.r[15].wrapping_sub(4);
            self.cpsr |= T;
            self.r[15] = self.r[15]
                .wrapping_add(4)
                .wrapping_add(offset as u32)
                .wrapping_add(h);
            return 3;
        }
        if !self.condition(instruction >> 28) {
            return 1;
        }
        if instruction & 0x0f000000 == 0x0f000000 {
            self.swi(bus, (instruction & 0xff) as u8);
            return 3;
        }
        if instruction & 0x0fb00ff0 == 0x01000090 {
            let rn = ((instruction >> 16) & 0xf) as usize;
            let rd = ((instruction >> 12) & 0xf) as usize;
            let rm = (instruction & 0xf) as usize;
            let address = self.r[rn];
            let old = if instruction & (1 << 22) != 0 {
                bus.read8(address) as u32
            } else {
                bus.read32(address)
            };
            if instruction & (1 << 22) != 0 {
                bus.write8(address, self.r[rm] as u8);
            } else {
                bus.write32(address, self.r[rm]);
            }
            self.r[rd] = old;
            return 4;
        }
        if instruction & 0x0fbf0fff == 0x010f0000 {
            let rd = ((instruction >> 12) & 0xf) as usize;
            self.r[rd] = self.cpsr;
            return 1;
        }
        if instruction & 0x0f000090 == 0x01000090 {
            return self.arm_halfword_transfer(bus, instruction);
        }
        if instruction & 0x0f0000f0 == 0x00000090 {
            let rm = (instruction & 0xf) as usize;
            let rs = ((instruction >> 8) & 0xf) as usize;
            let rn = ((instruction >> 12) & 0xf) as usize;
            let rd = ((instruction >> 16) & 0xf) as usize;
            let mut value = self.r[rm].wrapping_mul(self.r[rs]);
            if instruction & (1 << 21) != 0 {
                value = value.wrapping_add(self.r[rn]);
            }
            self.r[rd] = value;
            if instruction & (1 << 20) != 0 {
                self.set_nz(value);
            }
            return 2;
        }
        if instruction & 0x0fb0fff0 == 0x0120f000 {
            let source = if instruction & (1 << 25) != 0 {
                (instruction & 0xff).rotate_right(((instruction >> 8) & 0xf) * 2)
            } else {
                self.r[(instruction & 0xf) as usize]
            };
            let fields = (instruction >> 16) & 0xf;
            let field_mask = if fields & 1 != 0 { 0x000000ff } else { 0 }
                | if fields & 2 != 0 { 0x0000ff00 } else { 0 }
                | if fields & 4 != 0 { 0x00ff0000 } else { 0 }
                | if fields & 8 != 0 { 0xff000000 } else { 0 };
            let old_mode = self.mode;
            self.cpsr = (self.cpsr & !field_mask) | (source & field_mask);
            let new_mode = (self.cpsr & 0x1f) as u8;
            if field_mask & 0xff != 0 && new_mode != old_mode {
                self.restore_mode(new_mode);
            }
            return 1;
        }
        if instruction & 0x0ffffff0 == 0x012fff10 {
            let target = self.r[(instruction & 0xf) as usize];
            self.cpsr = (self.cpsr & !T) | (target & 1) << 5;
            self.r[15] = target & if self.cpsr & T != 0 { !1 } else { !3 };
            return 3;
        }
        if instruction & 0x0ffffff0 == 0x012fff30 {
            self.r[14] = self.r[15].wrapping_sub(4);
            let target = self.r[(instruction & 0xf) as usize];
            self.cpsr = (self.cpsr & !T) | (target & 1) << 5;
            self.r[15] = target & if self.cpsr & T != 0 { !1 } else { !3 };
            return 3;
        }
        if instruction & 0x0e000000 == 0x0a000000 {
            let offset = (((instruction & 0x00ffffff) << 2) as i32) << 6 >> 6;
            self.r[15] = self.r[15].wrapping_add(4).wrapping_add(offset as u32);
            if instruction & (1 << 24) != 0 {
                self.r[14] = self.r[15].wrapping_sub(4);
            }
            return 3;
        }
        if instruction & 0x0e000000 == 0x08000000 {
            return self.arm_block_transfer(bus, instruction);
        }
        if (instruction & 0x000000f0) == 0x00000090 && (instruction & 0x0fc00000) == 0 {
            let rm = (instruction & 0xf) as usize;
            let rs = ((instruction >> 8) & 0xf) as usize;
            let rn = ((instruction >> 12) & 0xf) as usize;
            let rd = ((instruction >> 16) & 0xf) as usize;
            let mut value = self.r[rm].wrapping_mul(self.r[rs]);
            if instruction & (1 << 21) != 0 {
                value = value.wrapping_add(self.r[rn]);
            }
            self.r[rd] = value;
            if instruction & (1 << 20) != 0 {
                self.set_nz(value);
            }
            return 2;
        }
        if instruction & 0x0c000000 == 0x04000000 {
            return self.arm_load_store(bus, instruction);
        }
        if instruction & 0x0c000000 == 0 {
            return self.arm_data_processing(instruction);
        }
        self.unsupported += 1;
        if self.first_unsupported_pc.is_none() {
            self.first_unsupported_pc = Some(address);
            self.first_unsupported_instruction = instruction;
        }
        1
    }

    fn arm_data_processing(&mut self, instruction: u32) -> u32 {
        let immediate = instruction & (1 << 25) != 0;
        let opcode = (instruction >> 21) & 0xf;
        let rn = ((instruction >> 16) & 0xf) as usize;
        let rd = ((instruction >> 12) & 0xf) as usize;
        let operand = if immediate {
            let value = instruction & 0xff;
            value.rotate_right(((instruction >> 8) & 0xf) * 2)
        } else {
            let source_register = (instruction & 0xf) as usize;
            let value = if source_register == 15 {
                self.r[15].wrapping_add(4)
            } else {
                self.r[source_register]
            };
            let shift = if instruction & (1 << 4) != 0 {
                self.r[((instruction >> 8) & 0xf) as usize] & 0xff
            } else {
                (instruction >> 7) & 0x1f
            };
            match (instruction >> 5) & 3 {
                0 => value.wrapping_shl(shift),
                1 => value.wrapping_shr(if shift == 0 { 32 } else { shift }),
                2 => {
                    if shift == 0 {
                        if value & 0x8000_0000 != 0 {
                            u32::MAX
                        } else {
                            0
                        }
                    } else {
                        ((value as i32) >> shift) as u32
                    }
                }
                _ => value.rotate_right(if shift == 0 { 1 } else { shift }),
            }
        };
        let lhs = if rn == 15 {
            self.r[15].wrapping_add(4)
        } else {
            self.r[rn]
        };
        let result = match opcode {
            0 => lhs & operand,
            1 => lhs ^ operand,
            2 => lhs.wrapping_sub(operand),
            3 => lhs
                .wrapping_sub(operand)
                .wrapping_sub(if self.cpsr & C != 0 { 0 } else { 1 }),
            4 => lhs.wrapping_add(operand),
            5 => lhs
                .wrapping_add(operand)
                .wrapping_add(if self.cpsr & C != 0 { 1 } else { 0 }),
            6 => lhs
                .wrapping_sub(operand)
                .wrapping_sub(if self.cpsr & C != 0 { 0 } else { 1 }),
            10 => {
                self.set_nz(lhs.wrapping_sub(operand));
                return 1;
            }
            8 => {
                self.set_nz(lhs & operand);
                return 1;
            }
            9 => {
                self.set_nz(lhs ^ operand);
                return 1;
            }
            11 => {
                self.set_nz(lhs.wrapping_add(operand));
                return 1;
            }
            12 => lhs | operand,
            13 => operand,
            15 => !operand,
            _ => return 1,
        };
        if rd == 15 {
            self.r[15] = result & !3;
        } else {
            self.r[rd] = result;
        }
        if rd == 15 && instruction & (1 << 20) != 0 {
            let mode = (self.spsr & 0x1f) as u8;
            self.cpsr = self.spsr;
            self.restore_mode(mode);
        } else if instruction & (1 << 20) != 0 {
            self.set_nz(result);
        }
        1
    }

    fn arm_load_store(&mut self, bus: &mut Bus, instruction: u32) -> u32 {
        let rn = ((instruction >> 16) & 0xf) as usize;
        let rd = ((instruction >> 12) & 0xf) as usize;
        let offset = if instruction & (1 << 25) != 0 {
            let rm = (instruction & 0xf) as usize;
            let shift = (instruction >> 7) & 0x1f;
            self.r[rm].wrapping_shl(shift)
        } else {
            instruction & 0xfff
        };
        let base = if rn == 15 {
            self.r[15].wrapping_add(4)
        } else {
            self.r[rn]
        };
        let adjusted = if instruction & (1 << 23) != 0 {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let address = if instruction & (1 << 24) != 0 {
            adjusted
        } else {
            base
        };
        if instruction & (1 << 20) != 0 {
            let value = if instruction & (1 << 22) != 0 {
                bus.read8(address) as u32
            } else {
                bus.read32(address)
            };
            if rd == 15 {
                self.r[15] = value & !3;
            } else {
                self.r[rd] = value;
            }
        } else if instruction & (1 << 22) != 0 {
            bus.write8(address, self.r[rd] as u8);
        } else {
            bus.write32(address, self.r[rd]);
        }
        if (instruction & (1 << 21) != 0 || instruction & (1 << 24) == 0) && rn != 15 {
            self.r[rn] = adjusted;
        }
        2
    }

    fn arm_halfword_transfer(&mut self, bus: &mut Bus, instruction: u32) -> u32 {
        let rn = ((instruction >> 16) & 0xf) as usize;
        let rd = ((instruction >> 12) & 0xf) as usize;
        let offset = ((instruction >> 4) & 0xf0) | (instruction & 0xf);
        let base = if rn == 15 {
            self.r[15].wrapping_add(4)
        } else {
            self.r[rn]
        };
        let address = if instruction & (1 << 23) != 0 {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        if instruction & (1 << 20) != 0 {
            self.r[rd] = match (instruction >> 5) & 3 {
                2 => bus.read8(address) as i8 as i32 as u32,
                3 => bus.read16(address) as i16 as i32 as u32,
                _ => bus.read16(address) as u32,
            };
        } else {
            bus.write16(address, self.r[rd] as u16);
        }
        if instruction & (1 << 21) != 0 && rn != 15 {
            self.r[rn] = address;
        }
        2
    }

    fn arm_block_transfer(&mut self, bus: &mut Bus, instruction: u32) -> u32 {
        let base_register = ((instruction >> 16) & 0xf) as usize;
        let registers = instruction & 0xffff;
        let count = registers.count_ones();
        if count == 0 {
            return 1;
        }
        let base = self.r[base_register];
        let up = instruction & (1 << 23) != 0;
        let before = instruction & (1 << 24) != 0;
        let writeback = instruction & (1 << 21) != 0;
        let start = if up {
            base + if before { 4 } else { 0 }
        } else {
            base - count * 4 + if before { 0 } else { 4 }
        };
        let mut address = start;
        for register in 0..16 {
            if registers & (1 << register) == 0 {
                continue;
            }
            if instruction & (1 << 20) != 0 {
                self.r[register] = bus.read32(address);
            } else {
                bus.write32(address, self.r[register]);
            }
            address += 4;
        }
        if instruction & (1 << 22) != 0 && instruction & (1 << 20) != 0 {
            self.cpsr = self.spsr;
        }
        if writeback {
            self.r[base_register] = if up {
                base + count * 4
            } else {
                base - count * 4
            };
        }
        2 + count
    }

    fn step_thumb(&mut self, bus: &mut Bus) -> u32 {
        let address = self.r[15] & !1;
        self.r[15] = address;
        let instruction = bus.read16(address);
        if self.first_thumb_pc.is_none() {
            self.first_thumb_pc = Some(address);
            self.first_thumb_instruction = instruction as u32;
        }
        self.last_pc = address;
        self.last_instruction = instruction as u32;
        if !valid_cpu_address(address) && self.first_invalid_pc.is_none() {
            self.first_invalid_pc = Some(address);
            self.first_invalid_instruction = instruction as u32;
        }
        if valid_cpu_address(address) {
            self.last_valid_pc = address;
            self.last_valid_instruction = instruction as u32;
        }
        self.r[15] = address.wrapping_add(2);
        let op = instruction as u32;
        if op & 0xf800 == 0xf000 {
            let offset = (((op & 0x7ff) << 1) as i16) << 5 >> 5;
            self.thumb_bl_base = Some(self.r[15].wrapping_add((offset as i32 as u32) << 11));
            return 1;
        }
        if op & 0xf800 == 0xf800 {
            if let Some(base) = self.thumb_bl_base.take() {
                self.r[14] = (self.r[15].wrapping_sub(1)) | 1;
                self.r[15] = base.wrapping_add((op & 0x7ff) << 1);
                return 3;
            }
        }
        if op & 0xfc00 == 0x4400 {
            let rd = ((op & 7) | ((op >> 4) & 8)) as usize;
            let rs = ((op >> 3) & 0xf) as usize;
            match (op >> 8) & 3 {
                0 => self.r[rd] = self.r[rd].wrapping_add(self.r[rs]),
                1 => {
                    let result = self.r[rd].wrapping_sub(self.r[rs]);
                    self.set_nz(result);
                }
                2 => self.r[rd] = self.r[rs],
                3 => {
                    let target = self.r[rs];
                    self.cpsr = (self.cpsr & !T) | (target & 1) << 5;
                    self.r[15] = target & if self.cpsr & T != 0 { !1 } else { !3 };
                }
                _ => unreachable!(),
            }
            return 1;
        }
        if op & 0xf800 == 0x0000 || op & 0xf800 == 0x0800 || op & 0xf800 == 0x1000 {
            let shift = ((op >> 6) & 0x1f) as u32;
            let rs = ((op >> 3) & 7) as usize;
            let rd = (op & 7) as usize;
            self.r[rd] = match op >> 11 {
                0 => self.r[rs].wrapping_shl(shift),
                1 => self.r[rs].wrapping_shr(shift),
                _ => ((self.r[rs] as i32) >> shift) as u32,
            };
            self.set_nz(self.r[rd]);
            return 1;
        }
        if op & 0xfc00 == 0x1800 {
            let rd = (op & 7) as usize;
            let rs = ((op >> 3) & 7) as usize;
            let rn = ((op >> 6) & 7) as usize;
            self.r[rd] = if op & (1 << 10) != 0 {
                self.r[rs].wrapping_sub(if op & (1 << 9) != 0 {
                    op >> 6 & 7
                } else {
                    self.r[rn]
                })
            } else {
                self.r[rs].wrapping_add(if op & (1 << 9) != 0 {
                    op >> 6 & 7
                } else {
                    self.r[rn]
                })
            };
            self.set_nz(self.r[rd]);
            return 1;
        }
        if op & 0xf800 == 0xe000 {
            let offset = ((((op & 0x7ff) << 1) as i16) << 4) >> 4;
            self.r[15] = self.r[15].wrapping_add(offset as i32 as u32);
            return 3;
        }
        if op & 0xf000 == 0xd000 && op & 0x0f00 != 0x0f00 {
            let condition = (op >> 8) & 0xf;
            if self.condition(condition) {
                self.r[15] = self.r[15].wrapping_add(((op & 0xff) as i8 as i32 * 2) as u32);
            }
            return 3;
        }
        if op & 0xf800 == 0x2000 {
            let rd = ((op >> 8) & 7) as usize;
            self.r[rd] = op & 0xff;
            self.set_nz(self.r[rd]);
            return 1;
        }
        if op & 0xf800 == 0x2800 {
            let rd = ((op >> 8) & 7) as usize;
            self.set_nz(self.r[rd].wrapping_sub(op & 0xff));
            return 1;
        }
        if op & 0xf800 == 0x3000 {
            let rd = ((op >> 8) & 7) as usize;
            self.r[rd] = self.r[rd].wrapping_add(op & 0xff);
            self.set_nz(self.r[rd]);
            return 1;
        }
        if op & 0xf800 == 0x3800 {
            let rd = ((op >> 8) & 7) as usize;
            self.r[rd] = self.r[rd].wrapping_sub(op & 0xff);
            self.set_nz(self.r[rd]);
            return 1;
        }
        if op & 0xfc00 == 0x4000 {
            let rd = (op & 7) as usize;
            let rs = ((op >> 3) & 7) as usize;
            self.r[rd] = match (op >> 6) & 0xf {
                0 => self.r[rd] & self.r[rs],
                1 => self.r[rd] ^ self.r[rs],
                2 => self.r[rd].wrapping_sub(self.r[rs]),
                3 => self.r[rd].wrapping_add(self.r[rs]),
                4 => self.r[rd].wrapping_sub(1),
                5 => self.r[rd].wrapping_add(1),
                6 => self.r[rd] << (self.r[rs] & 0xff),
                7 => self.r[rd] >> (self.r[rs] & 0xff),
                8 => self.r[rd] & self.r[rs],
                9 => (!self.r[rs]).wrapping_add(1),
                10 => self.r[rd].wrapping_sub(self.r[rs]),
                11 => self.r[rd].wrapping_add(self.r[rs]),
                12 => self.r[rd] | self.r[rs],
                13 => self.r[rd].wrapping_mul(self.r[rs]),
                14 => self.r[rd] & !self.r[rs],
                _ => !self.r[rs],
            };
            self.set_nz(self.r[rd]);
            return 1;
        }
        if op & 0xf800 == 0x4800 {
            let rd = ((op >> 8) & 7) as usize;
            let address = ((self.r[15].wrapping_add(2)) & !2).wrapping_add((op & 0xff) << 2);
            self.r[rd] = bus.read32(address);
            return 2;
        }
        if op & 0xf200 == 0x5000 {
            let rd = (op & 7) as usize;
            let rn = ((op >> 3) & 7) as usize;
            let rm = ((op >> 6) & 7) as usize;
            let address = self.r[rn].wrapping_add(self.r[rm]);
            if op & (1 << 10) != 0 {
                bus.write8(address, self.r[rd] as u8);
            } else {
                self.r[rd] = bus.read8(address) as u32;
            }
            return 2;
        }
        if op & 0xf000 == 0x6000 {
            let rd = (op & 7) as usize;
            let rn = ((op >> 3) & 7) as usize;
            let offset = ((op >> 6) & 0x1f) << 2;
            let address = self.r[rn].wrapping_add(offset);
            if op & (1 << 11) != 0 {
                let value = bus.read32(address);
                if rd == 15 {
                    self.r[15] = value & !1;
                } else {
                    self.r[rd] = value;
                }
            } else {
                bus.write32(address, self.r[rd]);
            }
            return 2;
        }
        if op & 0xf000 == 0x7000 {
            let rd = (op & 7) as usize;
            let rn = ((op >> 3) & 7) as usize;
            let address = self.r[rn].wrapping_add((op >> 6) & 0x1f);
            if op & (1 << 11) != 0 {
                self.r[rd] = bus.read8(address) as u32;
            } else {
                bus.write8(address, self.r[rd] as u8);
            }
            return 2;
        }
        if op & 0xf000 == 0x8000 {
            let rd = (op & 7) as usize;
            let rn = ((op >> 3) & 7) as usize;
            let address = self.r[rn].wrapping_add(((op >> 6) & 0x1f) << 1);
            if op & (1 << 11) != 0 {
                self.r[rd] = bus.read16(address) as u32;
            } else {
                bus.write16(address, self.r[rd] as u16);
            }
            return 2;
        }
        if op & 0xf000 == 0xc000 {
            let base_register = ((op >> 8) & 7) as usize;
            let registers = op & 0xff;
            let mut address = self.r[base_register];
            for register in 0..8 {
                if registers & (1 << register) == 0 {
                    continue;
                }
                if op & (1 << 11) != 0 {
                    self.r[register] = bus.read32(address);
                } else {
                    bus.write32(address, self.r[register]);
                }
                address += 4;
            }
            self.r[base_register] = address;
            return 2;
        }
        if op & 0xf000 == 0x9000 {
            let rd = ((op >> 8) & 7) as usize;
            let address = self.r[13].wrapping_add((op & 0xff) << 2);
            if op & (1 << 11) != 0 {
                self.r[rd] = bus.read32(address);
            } else {
                bus.write32(address, self.r[rd]);
            }
            return 2;
        }
        if op & 0xf000 == 0xa000 {
            let rd = ((op >> 8) & 7) as usize;
            let base = if op & (1 << 11) != 0 {
                self.r[13]
            } else {
                (self.r[15].wrapping_add(2)) & !2
            };
            self.r[rd] = base.wrapping_add((op & 0xff) << 2);
            return 1;
        }
        if op & 0xff00 == 0xb000 {
            let amount = (op & 0x7f) << 2;
            if op & (1 << 7) != 0 {
                self.r[13] = self.r[13].wrapping_sub(amount);
            } else {
                self.r[13] = self.r[13].wrapping_add(amount);
            }
            return 1;
        }
        if op & 0xfe00 == 0xb400 {
            let count = (op & 0xff).count_ones() + if op & (1 << 8) != 0 { 1 } else { 0 };
            let mut address = self.r[13].wrapping_sub(count * 4);
            for register in 0..8 {
                if op & (1 << register) != 0 {
                    bus.write32(address, self.r[register]);
                    address += 4;
                }
            }
            if op & (1 << 8) != 0 {
                bus.write32(address, self.r[14]);
            }
            self.r[13] = self.r[13].wrapping_sub(count * 4);
            return 2;
        }
        if op & 0xfe00 == 0xbc00 {
            let mut address = self.r[13];
            for register in 0..8 {
                if op & (1 << register) != 0 {
                    self.r[register] = bus.read32(address);
                    address = address.wrapping_add(4);
                }
            }
            if op & (1 << 8) != 0 {
                self.r[15] = bus.read32(address);
                address += 4;
            }
            self.r[13] = address;
            return 2;
        }
        if op & 0xff00 == 0xdf00 {
            self.swi(bus, (op & 0xff) as u8);
            return 3;
        }
        self.unsupported += 1;
        if self.first_unsupported_pc.is_none() {
            self.first_unsupported_pc = Some(address);
            self.first_unsupported_instruction = op;
        }
        1
    }
}

fn valid_cpu_address(address: u32) -> bool {
    matches!(address >> 24, 0x00 | 0x02 | 0x03 | 0x08..=0x0f)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_mirrors_and_vram_work() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.write32(0x0204_0000, 0x1234_5678);
        assert_eq!(bus.read32(0x0200_0000), 0x1234_5678);
        bus.write16(0x0601_0000, 0x7fff);
        assert_eq!(bus.read16(0x0601_0000), 0x7fff);
    }
    #[test]
    fn cartridge_save_memory_round_trips() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.write8(0x0e00_1234, 0x5a);
        let mut restored = Bus::new(vec![0; 4], None);
        restored.load_save(&bus.save);
        assert_eq!(restored.read8(0x0e00_1234), 0x5a);
    }
    #[test]
    fn register_ram_reset_clears_selected_regions() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.ewram[0] = 1;
        bus.iwram[0] = 2;
        bus.vram[0] = 3;
        bus.oam[0] = 4;
        let mut cpu = Cpu::new();
        cpu.r[0] = 0x1b;
        cpu.swi(&mut bus, 1);
        assert_eq!(bus.ewram[0], 0);
        assert_eq!(bus.iwram[0], 0);
        assert_eq!(bus.vram[0], 0);
        assert_eq!(bus.oam[0], 0);
    }
    #[test]
    fn timer_overflow_reloads_and_requests_irq() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.write16(0x0400_0100, 0xffff);
        bus.write8(0x0400_0102, 0xc0);
        bus.tick(1);
        assert_eq!(bus.read16(0x0400_0100), 0xffff);
        assert_ne!(bus.read16(0x0400_0202) & (1 << 3), 0);
    }
    #[test]
    fn ppu_timing_reports_hblank_and_vblank() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.tick(1005);
        assert_eq!(bus.read8(0x0400_0004) & 2, 0);
        bus.tick(1);
        assert_ne!(bus.read8(0x0400_0004) & 2, 0);
        bus.tick(1232 * 160);
        assert_eq!(bus.read8(0x0400_0006), 160);
        assert_ne!(bus.read8(0x0400_0004) & 1, 0);
    }
    #[test]
    fn mode4_renders_palette_indices() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.io[0] = 4;
        bus.write8(0x0600_0000, 1);
        bus.write16(0x0500_0002, 0x001f);
        assert_eq!(bus.framebuffer()[0], 0xff0000);
    }
    #[test]
    fn mode5_renders_centered_16bit_bitmap() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.io[0] = 5;
        bus.write16(0x0600_0000, 0x03e0);
        let frame = bus.framebuffer();
        assert_eq!(frame[16 * WIDTH + 40], 0x00ff00);
        assert_eq!(frame[0], 0);
    }
    #[test]
    fn dma_immediate_word_transfer_works() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.write32(0x0200_0000, 0x1122_3344);
        bus.write32(0x0400_00b0, 0x0200_0000);
        bus.write32(0x0400_00b4, 0x0300_0000);
        bus.write16(0x0400_00b8, 1);
        bus.write16(0x0400_00ba, 0x8400);
        assert_eq!(bus.read32(0x0300_0000), 0x1122_3344);
    }
    #[test]
    fn irq_enters_vector_and_masks_further_irqs() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.io[0x200] = 1 << 3;
        bus.io[0x208] = 1;
        bus.io[0x202] = 1 << 3;
        let mut cpu = Cpu::new();
        cpu.r[15] = 0x0300_0100;
        cpu.enter_irq(&mut bus);
        assert_eq!(cpu.r[15], 0x18);
        assert_eq!(cpu.r[14], 0x0300_0104);
        assert_ne!(cpu.cpsr & (1 << 7), 0);
    }
    #[test]
    fn irq_uses_banked_stack_and_link_registers() {
        let mut bus = Bus::new(vec![0; 4], None);
        let mut cpu = Cpu::new();
        cpu.r[13] = 0x0300_7f00;
        cpu.r[14] = 0x0800_0100;
        cpu.enter_irq(&mut bus);
        cpu.r[13] = 0x0300_3f00;
        cpu.r[14] = 0x0000_0018;
        cpu.restore_mode(0x13);
        assert_eq!(cpu.r[13], 0x0300_7f00);
        assert_eq!(cpu.r[14], 0x0800_0100);
    }
    #[test]
    fn keypad_is_active_low() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.set_keypad(1 << 0);
        assert_eq!(bus.read16(0x0400_0130), 0x03fe);
    }
    #[test]
    fn cpu_set_copies_words_without_bios() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.write32(0x0200_0000, 0x1234_5678);
        let mut cpu = Cpu::new();
        cpu.r[0] = 0x0200_0000;
        cpu.r[1] = 0x0300_0000;
        cpu.r[2] = (1 << 26) | 1;
        cpu.swi(&mut bus, 0x0b);
        assert_eq!(bus.read32(0x0300_0000), 0x1234_5678);
    }
    #[test]
    fn cpu_fast_set_fills_words_without_bios() {
        let mut bus = Bus::new(vec![0; 4], None);
        bus.write32(0x0200_0000, 0xaabb_ccdd);
        let mut cpu = Cpu::new();
        cpu.r[0] = 0x0200_0000;
        cpu.r[1] = 0x0300_0000;
        cpu.r[2] = (1 << 24) | 1;
        cpu.swi(&mut bus, 0x0c);
        assert_eq!(bus.read32(0x0300_0000), 0xaabb_ccdd);
        assert_eq!(bus.read32(0x0300_0004), 0xaabb_ccdd);
    }
    #[test]
    fn lz77_swi_decodes_literals() {
        let mut bus = Bus::new(vec![0; 4], None);
        let compressed = [0x10, 4, 0, 0, 0, b'A', b'B', b'C', b'D'];
        for (index, byte) in compressed.into_iter().enumerate() {
            bus.write8(0x0200_0000 + index as u32, byte);
        }
        let mut cpu = Cpu::new();
        cpu.r[0] = 0x0200_0000;
        cpu.r[1] = 0x0300_0000;
        cpu.swi(&mut bus, 0x10);
        assert_eq!(&bus.iwram[..4], b"ABCD");
    }
    #[test]
    fn rl_swi_decodes_runs() {
        let mut bus = Bus::new(vec![0; 4], None);
        let compressed = [0x30, 4, 0, 0, 0x83, b'B'];
        for (index, byte) in compressed.into_iter().enumerate() {
            bus.write8(0x0200_0000 + index as u32, byte);
        }
        let mut cpu = Cpu::new();
        cpu.r[0] = 0x0200_0000;
        cpu.r[1] = 0x0300_0000;
        cpu.swi(&mut bus, 0x13);
        assert_eq!(&bus.iwram[..4], b"BBBB");
    }
    #[test]
    fn thumb_backward_branch_sign_extends() {
        let mut rom = vec![0; 4];
        rom[0..2].copy_from_slice(&0xe7feu16.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.cpsr |= T;
        cpu.r[15] = 0x0800_0000;
        cpu.step(&mut bus);
        assert_eq!(cpu.r[15], 0x07ff_fffe);
    }
    #[test]
    fn arm_mov_and_add_execute() {
        let mut rom = vec![0; 16];
        rom[0..4].copy_from_slice(&0xe3a0_0002u32.to_le_bytes());
        rom[4..8].copy_from_slice(&0xe280_1003u32.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.reset();
        cpu.step(&mut bus);
        cpu.step(&mut bus);
        assert_eq!(cpu.r[1], 5);
    }
    #[test]
    fn arm_register_shifted_mov_uses_shift_type() {
        let mut rom = vec![0; 4];
        rom[..4].copy_from_slice(&0xe1a00141u32.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.reset();
        cpu.r[1] = 0x8000_0000;
        cpu.step(&mut bus);
        assert_eq!(cpu.r[0], 0xe000_0000);
    }
    #[test]
    fn arm_halfword_load_executes() {
        let mut rom = vec![0; 4];
        rom[..4].copy_from_slice(&0xe1d100b2u32.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        bus.write16(0x0200_0002, 0x4567);
        let mut cpu = Cpu::new();
        cpu.reset();
        cpu.r[1] = 0x0200_0000;
        cpu.step(&mut bus);
        assert_eq!(cpu.r[0], 0x4567);
    }
    #[test]
    fn arm_signed_halfword_load_sign_extends() {
        let mut rom = vec![0; 4];
        rom[..4].copy_from_slice(&0xe1d100f2u32.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        bus.write16(0x0200_0002, 0x8001);
        let mut cpu = Cpu::new();
        cpu.reset();
        cpu.r[1] = 0x0200_0000;
        cpu.step(&mut bus);
        assert_eq!(cpu.r[0], 0xffff_8001);
    }
    #[test]
    fn arm_mrs_reads_cpsr() {
        let mut rom = vec![0; 4];
        rom[..4].copy_from_slice(&0xe10f0000u32.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.reset();
        cpu.step(&mut bus);
        assert_eq!(cpu.r[0], 0xd3);
    }
    #[test]
    fn arm_msr_writes_cpsr_control_field() {
        let mut rom = vec![0; 4];
        rom[..4].copy_from_slice(&0xe129f000u32.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.reset();
        cpu.r[0] = 0x12;
        cpu.step(&mut bus);
        assert_eq!(cpu.cpsr & 0x1f, 0x12);
    }
    #[test]
    fn arm_msr_switches_active_exception_bank() {
        let mut rom = vec![0; 4];
        rom[..4].copy_from_slice(&0xe129f000u32.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.reset();
        cpu.r[13] = 0x0300_7fe0;
        cpu.banked_irq[0] = 0x0300_7fa0;
        cpu.r[0] = 0x12;
        cpu.step(&mut bus);
        assert_eq!(cpu.r[13], 0x0300_7fa0);
    }
    #[test]
    fn arm_blx_immediate_enters_thumb_state() {
        let mut rom = vec![0; 4];
        rom[..4].copy_from_slice(&0xfa000000u32.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.reset();
        cpu.step(&mut bus);
        assert_eq!(cpu.r[15], 0x0800_0008);
        assert_ne!(cpu.cpsr & T, 0);
        assert_eq!(cpu.r[14], 0x0800_0000);
    }
    #[test]
    fn emerald_reset_vector_branches_to_bootstrap() {
        let mut rom = vec![0; 0x208];
        rom[..4].copy_from_slice(&0xea00_007fu32.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.fast_boot(&mut bus);
        cpu.step(&mut bus);
        assert_eq!(cpu.r[15], 0x0800_0204);
    }
    #[test]
    fn arm_movs_pc_lr_restores_saved_cpsr() {
        let mut rom = vec![0; 4];
        rom[..4].copy_from_slice(&0xe1b0f00eu32.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.reset();
        cpu.r[14] = 0x0200_0100;
        cpu.spsr = 0x0000_003f;
        cpu.step(&mut bus);
        assert_eq!(cpu.r[15], 0x0200_0100);
        assert_eq!(cpu.cpsr, 0x0000_003f);
    }
    #[test]
    fn thumb_swi_halt_stops_execution() {
        let mut rom = vec![0; 2];
        rom[..2].copy_from_slice(&0xdf02u16.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.cpsr |= T;
        cpu.r[15] = 0x0800_0000;
        cpu.step(&mut bus);
        assert!(cpu.halted);
    }
    #[test]
    fn thumb_high_register_move_and_multiple_store_execute() {
        let mut rom = vec![0; 4];
        rom[..2].copy_from_slice(&0x4680u16.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.cpsr |= T;
        cpu.r[0] = 9;
        cpu.r[8] = 0x0200_0000;
        cpu.r[15] = 0x0800_0000;
        cpu.step(&mut bus);
        assert_eq!(cpu.r[8], 9);
    }
    #[test]
    fn thumb_bl_links_and_branches() {
        let mut rom = vec![0; 4];
        rom[..2].copy_from_slice(&0xf000u16.to_le_bytes());
        rom[2..4].copy_from_slice(&0xf800u16.to_le_bytes());
        let mut bus = Bus::new(rom, None);
        let mut cpu = Cpu::new();
        cpu.cpsr |= T;
        cpu.r[15] = 0x0800_0000;
        cpu.step(&mut bus);
        cpu.step(&mut bus);
        assert_eq!(cpu.r[15], 0x0800_0002);
        assert_eq!(cpu.r[14], 0x0800_0003);
    }
}
