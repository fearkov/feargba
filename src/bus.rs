//! System bus: memory map, IO registers, DMA and the interrupt controller.

use crate::apu::Apu;
use crate::cart::{Cart, SaveKind};
use crate::dma::{DmaChannel, DmaTiming};
use crate::ppu::{Ppu, EVENT_FRAME, EVENT_HBLANK, EVENT_VBLANK};
use crate::timer::Timers;

pub const IRQ_VBLANK: u16 = 1 << 0;
pub const IRQ_HBLANK: u16 = 1 << 1;
pub const IRQ_VCOUNT: u16 = 1 << 2;
pub const IRQ_DMA0: u16 = 1 << 8;
pub const IRQ_KEYPAD: u16 = 1 << 12;

const EWRAM_MASK: usize = 0x3_ffff;
const IWRAM_MASK: usize = 0x7fff;

pub struct Bus {
    pub bios: Vec<u8>,
    pub ewram: Vec<u8>,
    pub iwram: Vec<u8>,
    pub ppu: Ppu,
    pub apu: Apu,
    pub cart: Cart,
    pub timers: Timers,
    pub dma: [DmaChannel; 4],

    pub keyinput: u16,
    pub keycnt: u16,
    pub interrupt_enable: u16,
    pub interrupt_flags: u16,
    pub master_enable: bool,
    pub waitcnt: u16,
    pub postflg: u8,
    pub serial: [u16; 16],
    pub use_hle_bios: bool,

    /// Cycles the current instruction spent waiting on memory.
    pub access_cycles: u32,
    pub frame_ready: bool,
    open_bus: u32,
    bios_latch: u32,
    /// True while the program counter sits inside the BIOS.
    executing_bios: bool,
    wait16: [u32; 16],
    wait32: [u32; 16],
    dma_active: bool,
}

impl Bus {
    pub fn new(rom: Vec<u8>, bios: Vec<u8>) -> Self {
        let mut bus = Self {
            bios,
            ewram: vec![0; EWRAM_MASK + 1],
            iwram: vec![0; IWRAM_MASK + 1],
            ppu: Ppu::new(),
            apu: Apu::new(),
            cart: Cart::new(rom),
            timers: Timers::default(),
            dma: [DmaChannel::default(); 4],
            keyinput: 0x03ff,
            keycnt: 0,
            interrupt_enable: 0,
            interrupt_flags: 0,
            master_enable: false,
            waitcnt: 0,
            postflg: 0,
            serial: [0; 16],
            use_hle_bios: true,
            access_cycles: 0,
            frame_ready: false,
            open_bus: 0,
            bios_latch: 0xe129_f000,
            executing_bios: false,
            wait16: [1; 16],
            wait32: [1; 16],
            dma_active: false,
        };
        bus.update_waitstates();
        bus
    }

    fn update_waitstates(&mut self) {
        const NONSEQUENTIAL: [u32; 4] = [4, 3, 2, 8];
        let sram = NONSEQUENTIAL[(self.waitcnt & 3) as usize];
        let first = [
            NONSEQUENTIAL[((self.waitcnt >> 2) & 3) as usize],
            NONSEQUENTIAL[((self.waitcnt >> 5) & 3) as usize],
            NONSEQUENTIAL[((self.waitcnt >> 8) & 3) as usize],
        ];
        let second = [
            if self.waitcnt & 0x0010 != 0 { 1 } else { 2 },
            if self.waitcnt & 0x0080 != 0 { 1 } else { 4 },
            if self.waitcnt & 0x0400 != 0 { 1 } else { 8 },
        ];
        // Internal memory.
        self.wait16 = [1; 16];
        self.wait32 = [1; 16];
        self.wait16[2] = 3;
        self.wait32[2] = 6;
        self.wait32[5] = 2;
        self.wait32[6] = 2;
        for region in 0..3 {
            for mirror in 0..2 {
                let index = 8 + region * 2 + mirror;
                self.wait16[index] = 1 + first[region];
                self.wait32[index] = 2 + first[region] + second[region];
            }
        }
        self.wait16[0xe] = 1 + sram;
        self.wait16[0xf] = 1 + sram;
        self.wait32[0xe] = 1 + sram;
        self.wait32[0xf] = 1 + sram;
    }

    /// Writes an IO register without charging the access to the CPU.
    pub fn write16_raw(&mut self, address: u32, value: u16) {
        self.store16(address & !1, value);
    }

    pub fn raise_irq(&mut self, bits: u16) {
        self.interrupt_flags |= bits;
    }

    pub fn irq_pending(&self) -> bool {
        self.master_enable && (self.interrupt_enable & self.interrupt_flags) != 0
    }

    /// The BIOS interrupt flag mirror at 0x03007FF8, used by `IntrWait`.
    pub fn bios_irq_flags(&self) -> u16 {
        u16::from_le_bytes([self.iwram[0x7ff8], self.iwram[0x7ff9]])
    }

    pub fn set_bios_irq_flags(&mut self, value: u16) {
        let bytes = value.to_le_bytes();
        self.iwram[0x7ff8] = bytes[0];
        self.iwram[0x7ff9] = bytes[1];
    }

    pub fn cycles_until_event(&self) -> u32 {
        self.ppu
            .cycles_until_event()
            .min(self.timers.cycles_until_overflow())
            .max(1)
    }

    /// Drives every peripheral forward by the cycles an instruction took.
    pub fn run_cycles(&mut self, cycles: u32) {
        let (ppu_irq, events) = self.ppu.tick(cycles);
        if ppu_irq != 0 {
            self.raise_irq(ppu_irq);
        }
        let (timer_irq, fifo) = self.timers.tick(cycles);
        if timer_irq != 0 {
            self.raise_irq(timer_irq);
        }
        self.apu.tick(cycles);

        if events & EVENT_HBLANK != 0 {
            self.trigger_dma(DmaTiming::HBlank);
        }
        if events & EVENT_VBLANK != 0 {
            self.trigger_dma(DmaTiming::VBlank);
        }
        if events & EVENT_FRAME != 0 {
            self.frame_ready = true;
        }
        if fifo != 0 {
            for timer in 0..2 {
                if fifo & 1 << timer == 0 {
                    continue;
                }
                let requests = self.apu.timer_overflow(timer);
                for (channel, wanted) in requests.iter().enumerate() {
                    if *wanted {
                        self.run_fifo_dma(channel);
                    }
                }
            }
        }
        if self.keycnt & 0x4000 != 0 {
            let mask = self.keycnt & 0x3ff;
            let pressed = !self.keyinput & 0x3ff;
            let triggered = if self.keycnt & 0x8000 != 0 {
                pressed & mask == mask && mask != 0
            } else {
                pressed & mask != 0
            };
            if triggered {
                self.raise_irq(IRQ_KEYPAD);
            }
        }
    }

    // ---------------------------------------------------------------- memory

    #[inline]
    fn region(address: u32) -> usize {
        ((address >> 24) & 0xf) as usize
    }

    pub fn read8(&mut self, address: u32) -> u8 {
        self.access_cycles += self.wait16[Self::region(address)];
        self.load8(address)
    }

    pub fn read16(&mut self, address: u32) -> u16 {
        self.access_cycles += self.wait16[Self::region(address)];
        self.load16(address)
    }

    pub fn read32(&mut self, address: u32) -> u32 {
        self.access_cycles += self.wait32[Self::region(address)];
        self.load32(address)
    }

    pub fn read16_code(&mut self, address: u32) -> u16 {
        self.access_cycles += self.wait16[Self::region(address)];
        self.executing_bios = address < 0x4000;
        let value = self.load16(address);
        if self.executing_bios {
            self.bios_latch = value as u32 | (value as u32) << 16;
        }
        self.open_bus = value as u32 | (value as u32) << 16;
        value
    }

    pub fn read32_code(&mut self, address: u32) -> u32 {
        self.access_cycles += self.wait32[Self::region(address)];
        self.executing_bios = address < 0x4000;
        let value = self.load32(address);
        if self.executing_bios {
            self.bios_latch = value;
        }
        self.open_bus = value;
        value
    }

    /// Mimics the value the real BIOS leaves behind after a software interrupt.
    pub fn set_bios_latch(&mut self, value: u32) {
        self.bios_latch = value;
    }

    pub fn write8(&mut self, address: u32, value: u8) {
        self.access_cycles += self.wait16[Self::region(address)];
        self.store8(address, value);
    }

    pub fn write16(&mut self, address: u32, value: u16) {
        self.access_cycles += self.wait16[Self::region(address)];
        self.store16(address, value);
    }

    pub fn write32(&mut self, address: u32, value: u32) {
        self.access_cycles += self.wait32[Self::region(address)];
        self.store32(address, value);
    }

    fn load8(&mut self, address: u32) -> u8 {
        match Self::region(address) {
            0x0 => {
                if address < 0x4000 && self.executing_bios {
                    self.bios[address as usize]
                } else if address < 0x4000 {
                    (self.bios_latch >> ((address & 3) * 8)) as u8
                } else {
                    (self.open_bus >> ((address & 3) * 8)) as u8
                }
            }
            0x2 => self.ewram[address as usize & EWRAM_MASK],
            0x3 => self.iwram[address as usize & IWRAM_MASK],
            0x4 => {
                let offset = (address & 0xffff) as usize;
                let half = self.read_io(offset & !1);
                if offset & 1 != 0 {
                    (half >> 8) as u8
                } else {
                    half as u8
                }
            }
            0x5 => self.ppu.palette[address as usize & 0x3ff],
            0x6 => self.ppu.vram[vram_offset(address)],
            0x7 => self.ppu.oam[address as usize & 0x3ff],
            0x8..=0xd => {
                if self.cart.is_eeprom_address(address) {
                    self.cart.read_eeprom() as u8
                } else {
                    self.cart.read_rom8(address)
                }
            }
            0xe | 0xf => self.cart.read_save(address),
            _ => (self.open_bus >> ((address & 3) * 8)) as u8,
        }
    }

    fn load16(&mut self, raw_address: u32) -> u16 {
        let address = raw_address & !1;
        match Self::region(raw_address) {
            0x0 => {
                if address < 0x4000 && self.executing_bios {
                    u16::from_le_bytes([self.bios[address as usize], self.bios[address as usize + 1]])
                } else {
                    (self.bios_latch >> ((address & 2) * 8)) as u16
                }
            }
            0x2 => {
                let index = address as usize & EWRAM_MASK;
                u16::from_le_bytes([self.ewram[index], self.ewram[index + 1]])
            }
            0x3 => {
                let index = address as usize & IWRAM_MASK;
                u16::from_le_bytes([self.iwram[index], self.iwram[index + 1]])
            }
            0x4 => self.read_io((raw_address & 0xfffe) as usize),
            0x5 => {
                let index = address as usize & 0x3fe;
                u16::from_le_bytes([self.ppu.palette[index], self.ppu.palette[index + 1]])
            }
            0x6 => {
                let index = vram_offset(address) & !1;
                u16::from_le_bytes([self.ppu.vram[index], self.ppu.vram[index + 1]])
            }
            0x7 => {
                let index = address as usize & 0x3fe;
                u16::from_le_bytes([self.ppu.oam[index], self.ppu.oam[index + 1]])
            }
            0x8..=0xd => {
                if self.cart.is_eeprom_address(address) {
                    self.cart.read_eeprom()
                } else {
                    self.cart.read_rom16(address)
                }
            }
            0xe | 0xf => {
                let byte = self.cart.read_save(raw_address) as u16;
                byte | byte << 8
            }
            _ => (self.open_bus >> ((address & 2) * 8)) as u16,
        }
    }

    fn load32(&mut self, raw_address: u32) -> u32 {
        let address = raw_address & !3;
        match Self::region(raw_address) {
            0x0 => {
                if address < 0x4000 && self.executing_bios {
                    let index = address as usize;
                    u32::from_le_bytes([
                        self.bios[index],
                        self.bios[index + 1],
                        self.bios[index + 2],
                        self.bios[index + 3],
                    ])
                } else {
                    self.bios_latch
                }
            }
            0x2 => {
                let index = address as usize & EWRAM_MASK;
                u32::from_le_bytes([
                    self.ewram[index],
                    self.ewram[index + 1],
                    self.ewram[index + 2],
                    self.ewram[index + 3],
                ])
            }
            0x3 => {
                let index = address as usize & IWRAM_MASK;
                u32::from_le_bytes([
                    self.iwram[index],
                    self.iwram[index + 1],
                    self.iwram[index + 2],
                    self.iwram[index + 3],
                ])
            }
            0x4 => {
                let offset = (address & 0xffff) as usize;
                self.read_io(offset) as u32 | (self.read_io(offset + 2) as u32) << 16
            }
            0xe | 0xf => {
                let byte = self.cart.read_save(raw_address) as u32;
                byte * 0x0101_0101
            }
            _ => self.load16(address) as u32 | (self.load16(address + 2) as u32) << 16,
        }
    }

    fn store8(&mut self, address: u32, value: u8) {
        match Self::region(address) {
            0x2 => self.ewram[address as usize & EWRAM_MASK] = value,
            0x3 => self.iwram[address as usize & IWRAM_MASK] = value,
            0x4 => self.write_io8((address & 0xffff) as usize, value),
            0x5 => {
                // Byte writes to palette memory duplicate into the halfword.
                let index = address as usize & 0x3fe;
                self.ppu.palette[index] = value;
                self.ppu.palette[index + 1] = value;
            }
            0x6 => {
                // Byte writes only take effect on background VRAM.
                let index = vram_offset(address) & !1;
                let limit = if self.ppu.dispcnt & 7 >= 3 { 0x14000 } else { 0x10000 };
                if index < limit {
                    self.ppu.vram[index] = value;
                    self.ppu.vram[index + 1] = value;
                }
            }
            0x7 => {}
            0x8..=0xd => {
                if self.cart.is_eeprom_address(address) {
                    self.cart.write_eeprom(value as u16);
                }
            }
            0xe | 0xf => self.cart.write_save(address, value),
            _ => {}
        }
    }

    fn store16(&mut self, raw_address: u32, value: u16) {
        let address = raw_address & !1;
        match Self::region(raw_address) {
            0x2 => {
                let index = address as usize & EWRAM_MASK;
                self.ewram[index..index + 2].copy_from_slice(&value.to_le_bytes());
            }
            0x3 => {
                let index = address as usize & IWRAM_MASK;
                self.iwram[index..index + 2].copy_from_slice(&value.to_le_bytes());
            }
            0x4 => self.write_io((raw_address & 0xfffe) as usize, value),
            0x5 => {
                let index = address as usize & 0x3fe;
                self.ppu.palette[index..index + 2].copy_from_slice(&value.to_le_bytes());
            }
            0x6 => {
                let index = vram_offset(address) & !1;
                self.ppu.vram[index..index + 2].copy_from_slice(&value.to_le_bytes());
            }
            0x7 => {
                let index = address as usize & 0x3fe;
                self.ppu.oam[index..index + 2].copy_from_slice(&value.to_le_bytes());
            }
            0x8..=0xd => {
                if self.cart.is_eeprom_address(address) {
                    self.cart.write_eeprom(value);
                } else {
                    self.cart.write_rom16(address, value);
                }
            }
            0xe | 0xf => {
                let byte = (value >> ((raw_address & 1) * 8)) as u8;
                self.cart.write_save(raw_address, byte);
            }
            _ => {}
        }
    }

    fn store32(&mut self, raw_address: u32, value: u32) {
        let address = raw_address & !3;
        match Self::region(raw_address) {
            0x4 => {
                let offset = (address & 0xffff) as usize;
                if offset == 0xa0 || offset == 0xa4 {
                    self.apu.push_fifo((offset - 0xa0) / 4, value);
                    return;
                }
                self.write_io(offset, value as u16);
                self.write_io(offset + 2, (value >> 16) as u16);
            }
            0xe | 0xf => {
                let byte = (value >> ((raw_address & 3) * 8)) as u8;
                self.cart.write_save(raw_address, byte);
            }
            _ => {
                self.store16(address & !3, value as u16);
                self.store16((address & !3) + 2, (value >> 16) as u16);
            }
        }
    }

    // ------------------------------------------------------------ io access

    fn read_io(&mut self, offset: usize) -> u16 {
        let offset = if offset >= 0x400 && offset & 0xff00 == 0x0800 {
            0x800 + (offset & 3)
        } else {
            offset
        };
        match offset {
            0x000 => self.ppu.dispcnt,
            0x002 => self.ppu.green_swap,
            0x004 => self.ppu.dispstat,
            0x006 => self.ppu.vcount,
            0x008..=0x00e => self.ppu.bgcnt[(offset - 0x008) / 2],
            0x048 => self.ppu.winin,
            0x04a => self.ppu.winout,
            0x050 => self.ppu.bldcnt,
            0x052 => self.ppu.bldalpha,
            0x060..=0x0a7 => self.apu.read_register(offset),
            0x0b8 | 0x0bc | 0x0c4 | 0x0c8 | 0x0d0 | 0x0d4 | 0x0dc => 0,
            0x0ba => self.dma[0].control,
            0x0c6 => self.dma[1].control,
            0x0d2 => self.dma[2].control,
            0x0de => self.dma[3].control,
            0x100..=0x10e => {
                let index = (offset - 0x100) / 4;
                self.timers.read(index, (offset / 2) & 1)
            }
            0x120..=0x12e | 0x134..=0x15e => self.serial[((offset - 0x120) / 2) & 0xf],
            0x130 => self.keyinput,
            0x132 => self.keycnt,
            0x200 => self.interrupt_enable,
            0x202 => self.interrupt_flags,
            0x204 => self.waitcnt,
            0x208 => self.master_enable as u16,
            0x300 => self.postflg as u16,
            _ => (self.open_bus >> ((offset & 2) * 8)) as u16,
        }
    }

    fn write_io8(&mut self, offset: usize, value: u8) {
        match offset {
            0x202 | 0x203 => {
                // Writing a one acknowledges that interrupt.
                let shift = (offset & 1) * 8;
                self.interrupt_flags &= !((value as u16) << shift);
            }
            0x0a0..=0x0a7 => self.apu.push_fifo((offset - 0xa0) / 4, value as u32),
            0x301 => {
                // HALTCNT: bit 7 clear halts, set stops.
                let _ = value;
            }
            _ => {
                let aligned = offset & !1;
                let current = self.read_io(aligned);
                let updated = if offset & 1 != 0 {
                    (current & 0x00ff) | (value as u16) << 8
                } else {
                    (current & 0xff00) | value as u16
                };
                self.write_io(aligned, updated);
            }
        }
    }

    fn write_io(&mut self, offset: usize, value: u16) {
        match offset {
            0x000 => self.ppu.dispcnt = value,
            0x002 => self.ppu.green_swap = value,
            0x004 => {
                self.ppu.dispstat = (self.ppu.dispstat & 0x0007) | (value & 0xff38);
            }
            0x008..=0x00e => self.ppu.bgcnt[(offset - 0x008) / 2] = value,
            0x010..=0x01e => {
                let index = (offset - 0x010) / 4;
                if offset & 2 == 0 {
                    self.ppu.bghofs[index] = value & 0x1ff;
                } else {
                    self.ppu.bgvofs[index] = value & 0x1ff;
                }
            }
            0x020 | 0x030 => self.ppu.bgpa[(offset - 0x020) / 16] = value as i16,
            0x022 | 0x032 => self.ppu.bgpb[(offset - 0x022) / 16] = value as i16,
            0x024 | 0x034 => self.ppu.bgpc[(offset - 0x024) / 16] = value as i16,
            0x026 | 0x036 => self.ppu.bgpd[(offset - 0x026) / 16] = value as i16,
            0x028 | 0x02a | 0x038 | 0x03a => {
                let index = (offset - 0x028) / 16;
                let current = self.ppu.bgx[index] as u32;
                let updated = if offset & 2 == 0 {
                    (current & 0xffff_0000) | value as u32
                } else {
                    (current & 0x0000_ffff) | (value as u32) << 16
                };
                self.ppu.set_bgx(index, sign_extend_28(updated));
            }
            0x02c | 0x02e | 0x03c | 0x03e => {
                let index = (offset - 0x02c) / 16;
                let current = self.ppu.bgy[index] as u32;
                let updated = if offset & 2 == 0 {
                    (current & 0xffff_0000) | value as u32
                } else {
                    (current & 0x0000_ffff) | (value as u32) << 16
                };
                self.ppu.set_bgy(index, sign_extend_28(updated));
            }
            0x040 | 0x042 => self.ppu.winh[(offset - 0x040) / 2] = value,
            0x044 | 0x046 => self.ppu.winv[(offset - 0x044) / 2] = value,
            0x048 => self.ppu.winin = value & 0x3f3f,
            0x04a => self.ppu.winout = value & 0x3f3f,
            0x04c => self.ppu.mosaic = value,
            0x050 => self.ppu.bldcnt = value & 0x3fff,
            0x052 => self.ppu.bldalpha = value & 0x1f1f,
            0x054 => self.ppu.bldy = value & 0x1f,
            0x060..=0x0a7 => self.apu.write_register(offset, value),
            0x0b0..=0x0de => self.write_dma(offset, value),
            0x100..=0x10e => {
                let index = (offset - 0x100) / 4;
                if offset & 2 == 0 {
                    self.timers.write_reload(index, value);
                } else {
                    self.timers.write_control(index, value);
                }
            }
            0x120..=0x12e | 0x134..=0x15e => {
                self.serial[((offset - 0x120) / 2) & 0xf] = value;
            }
            0x132 => self.keycnt = value,
            0x200 => self.interrupt_enable = value & 0x3fff,
            0x202 => self.interrupt_flags &= !value,
            0x204 => {
                self.waitcnt = value;
                self.update_waitstates();
            }
            0x208 => self.master_enable = value & 1 != 0,
            0x300 => self.postflg = value as u8,
            _ => {}
        }
    }

    // ------------------------------------------------------------------- dma

    fn write_dma(&mut self, offset: usize, value: u16) {
        let channel = (offset - 0x0b0) / 12;
        if channel > 3 {
            return;
        }
        let register = (offset - 0x0b0) % 12;
        let source_mask = if channel == 0 { 0x07ff_fffe } else { 0x0fff_fffe };
        let destination_mask = if channel == 3 { 0x0fff_fffe } else { 0x07ff_fffe };
        match register {
            0 => {
                let dma = &mut self.dma[channel];
                dma.source = (dma.source & 0xffff_0000) | value as u32;
            }
            2 => {
                let dma = &mut self.dma[channel];
                dma.source = (dma.source & 0x0000_ffff) | (value as u32) << 16;
            }
            4 => {
                let dma = &mut self.dma[channel];
                dma.destination = (dma.destination & 0xffff_0000) | value as u32;
            }
            6 => {
                let dma = &mut self.dma[channel];
                dma.destination = (dma.destination & 0x0000_ffff) | (value as u32) << 16;
            }
            8 => self.dma[channel].count = value,
            10 => {
                let was_enabled = self.dma[channel].enabled();
                self.dma[channel].control = value;
                if !was_enabled && self.dma[channel].enabled() {
                    let dma = &mut self.dma[channel];
                    dma.internal_source = dma.source & source_mask;
                    dma.internal_destination = dma.destination & destination_mask;
                    dma.internal_count = latched_count(channel, dma.count);
                    let immediate = dma.timing() == DmaTiming::Immediate;
                    self.maybe_detect_eeprom(channel);
                    if immediate {
                        self.run_dma(channel);
                    }
                }
                if !self.dma[channel].enabled() {
                    self.dma[channel].active = false;
                }
            }
            _ => {}
        }
    }

    /// EEPROM size is implied by how many bits the game clocks out.
    fn maybe_detect_eeprom(&mut self, channel: usize) {
        if !matches!(
            self.cart.kind,
            SaveKind::Eeprom512 | SaveKind::Eeprom8k
        ) {
            return;
        }
        let dma = &self.dma[channel];
        let destination = dma.internal_destination;
        if !self.cart.is_eeprom_address(destination) {
            return;
        }
        let kind = match dma.internal_count {
            9 | 73 => SaveKind::Eeprom512,
            17 | 81 => SaveKind::Eeprom8k,
            _ => return,
        };
        if self.cart.kind != kind {
            self.cart.kind = kind;
            self.cart.save = vec![0xff; kind.size()];
        }
    }

    pub fn trigger_dma(&mut self, timing: DmaTiming) {
        for channel in 0..4 {
            if self.dma[channel].enabled() && self.dma[channel].timing() == timing {
                self.run_dma(channel);
            }
        }
    }

    fn run_fifo_dma(&mut self, fifo: usize) {
        for channel in 1..=2 {
            let dma = &self.dma[channel];
            if !dma.enabled() || dma.timing() != DmaTiming::Special {
                continue;
            }
            let target = 0x0400_00a0 + fifo as u32 * 4;
            if dma.internal_destination != target {
                continue;
            }
            let mut source = self.dma[channel].internal_source;
            for _ in 0..4 {
                let value = self.load32(source & !3);
                self.apu.push_fifo(fifo, value);
                source = source.wrapping_add(4);
            }
            self.dma[channel].internal_source = source;
            self.access_cycles += 8;
            if self.dma[channel].irq() {
                self.raise_irq(1 << (8 + channel));
            }
        }
    }

    fn run_dma(&mut self, channel: usize) {
        if self.dma_active {
            return;
        }
        self.dma_active = true;
        let dma = self.dma[channel];
        let word = dma.word() || dma.timing() == DmaTiming::Special;
        let width: u32 = if word { 4 } else { 2 };

        let source_step = match dma.source_control() {
            0 => width as i32,
            1 => -(width as i32),
            _ => 0,
        };
        let destination_step = match dma.destination_control() {
            0 | 3 => width as i32,
            1 => -(width as i32),
            _ => 0,
        };

        let mut source = dma.internal_source & !(width - 1);
        let mut destination = dma.internal_destination & !(width - 1);
        let count = dma.internal_count;

        for _ in 0..count {
            if word {
                let value = self.load32(source);
                self.store32(destination, value);
            } else {
                let value = self.load16(source);
                self.store16(destination, value);
            }
            source = source.wrapping_add_signed(source_step);
            destination = destination.wrapping_add_signed(destination_step);
        }
        self.access_cycles += count * if word { 4 } else { 2 } + 2;

        self.dma[channel].internal_source = source;
        if dma.destination_control() == 3 {
            self.dma[channel].internal_destination = dma.destination
                & if channel == 3 { 0x0fff_fffe } else { 0x07ff_fffe };
        } else {
            self.dma[channel].internal_destination = destination;
        }
        if dma.irq() {
            self.raise_irq(1 << (8 + channel));
        }
        if dma.repeats() && dma.timing() != DmaTiming::Immediate {
            self.dma[channel].internal_count = latched_count(channel, self.dma[channel].count);
        } else {
            self.dma[channel].control &= !0x8000;
        }
        self.dma_active = false;
    }
}

fn latched_count(channel: usize, count: u16) -> u32 {
    let mask = if channel == 3 { 0xffff } else { 0x3fff };
    let value = count as u32 & mask;
    if value == 0 {
        mask + 1
    } else {
        value
    }
}

fn sign_extend_28(value: u32) -> i32 {
    ((value & 0x0fff_ffff) << 4) as i32 >> 4
}

/// VRAM mirrors in 128K steps, and the upper 32K repeats the 64-96K block.
fn vram_offset(address: u32) -> usize {
    let offset = (address as usize) & 0x1ffff;
    if offset >= 0x18000 {
        offset - 0x8000
    } else {
        offset
    }
}
