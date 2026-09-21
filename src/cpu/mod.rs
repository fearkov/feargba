//! ARM7TDMI core: register banks, exceptions and the fetch/execute loop.

mod arm;
mod thumb;

use crate::bus::Bus;

pub const MODE_USR: u32 = 0x10;
pub const MODE_FIQ: u32 = 0x11;
pub const MODE_IRQ: u32 = 0x12;
pub const MODE_SVC: u32 = 0x13;
pub const MODE_ABT: u32 = 0x17;
pub const MODE_UND: u32 = 0x1b;
pub const MODE_SYS: u32 = 0x1f;

/// Register bank index. User and system share a bank.
fn bank_of(mode: u32) -> usize {
    match mode {
        MODE_FIQ => 1,
        MODE_IRQ => 2,
        MODE_SVC => 3,
        MODE_ABT => 4,
        MODE_UND => 5,
        _ => 0,
    }
}

pub struct Cpu {
    pub r: [u32; 16],
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
    pub irq_disable: bool,
    pub fiq_disable: bool,
    pub thumb: bool,
    pub mode: u32,
    pub spsr: u32,
    spsr_bank: [u32; 6],
    /// r13/r14 for every bank; index 0 is shared by user and system.
    bank: [[u32; 2]; 6],
    /// r8-r12 saved while running in FIQ mode, and the FIQ copies themselves.
    usr_r8_12: [u32; 5],
    fiq_r8_12: [u32; 5],
    pub halted: bool,
    /// Flags an HLE `IntrWait`/`VBlankIntrWait` is blocked on.
    pub intr_wait: Option<u16>,
    /// Set by anything that writes r15 so the pipeline is reloaded.
    flush: bool,
    /// Slot 0 runs next, slot 1 is one fetch ahead. Keeping both means code
    /// that overwrites the instruction behind itself still runs the old one.
    pipeline: [u32; 2],
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            r: [0; 16],
            n: false,
            z: false,
            c: false,
            v: false,
            irq_disable: true,
            fiq_disable: true,
            thumb: false,
            mode: MODE_SVC,
            spsr: 0,
            spsr_bank: [0; 6],
            bank: [[0; 2]; 6],
            usr_r8_12: [0; 5],
            fiq_r8_12: [0; 5],
            halted: false,
            intr_wait: None,
            flush: true,
            pipeline: [0; 2],
        }
    }

    /// Enter the state the BIOS leaves behind, so games run without a BIOS image.
    pub fn skip_bios(&mut self) {
        self.r = [0; 16];
        self.bank = [[0; 2]; 6];
        self.bank[bank_of(MODE_SVC)] = [0x0300_7fe0, 0];
        self.bank[bank_of(MODE_IRQ)] = [0x0300_7fa0, 0];
        self.bank[0] = [0x0300_7f00, 0];
        self.r[13] = 0x0300_7f00;
        self.r[15] = 0x0800_0000;
        self.mode = MODE_SYS;
        self.thumb = false;
        self.irq_disable = false;
        self.fiq_disable = false;
        self.halted = false;
        self.intr_wait = None;
        self.flush = true;
    }

    /// Start from the BIOS reset vector (used when a real BIOS image is loaded).
    pub fn reset(&mut self) {
        self.r = [0; 16];
        self.mode = MODE_SVC;
        self.thumb = false;
        self.irq_disable = true;
        self.fiq_disable = true;
        self.halted = false;
        self.intr_wait = None;
        self.flush = true;
    }

    pub fn cpsr(&self) -> u32 {
        (self.n as u32) << 31
            | (self.z as u32) << 30
            | (self.c as u32) << 29
            | (self.v as u32) << 28
            | (self.irq_disable as u32) << 7
            | (self.fiq_disable as u32) << 6
            | (self.thumb as u32) << 5
            | self.mode
    }

    pub fn set_cpsr(&mut self, value: u32, change_mode: bool) {
        self.n = value & 1 << 31 != 0;
        self.z = value & 1 << 30 != 0;
        self.c = value & 1 << 29 != 0;
        self.v = value & 1 << 28 != 0;
        if change_mode {
            self.irq_disable = value & 1 << 7 != 0;
            self.fiq_disable = value & 1 << 6 != 0;
            self.thumb = value & 1 << 5 != 0;
            self.switch_mode(value & 0x1f);
        }
    }

    pub fn switch_mode(&mut self, mode: u32) {
        let mode = if bank_of(mode) == 0 && mode != MODE_USR {
            MODE_SYS
        } else {
            mode
        };
        if mode == self.mode {
            return;
        }
        let old = bank_of(self.mode);
        let new = bank_of(mode);
        self.bank[old] = [self.r[13], self.r[14]];
        self.spsr_bank[old] = self.spsr;
        if old == 1 && new != 1 {
            self.fiq_r8_12.copy_from_slice(&self.r[8..13]);
            self.r[8..13].copy_from_slice(&self.usr_r8_12);
        } else if old != 1 && new == 1 {
            self.usr_r8_12.copy_from_slice(&self.r[8..13]);
            self.r[8..13].copy_from_slice(&self.fiq_r8_12);
        }
        self.r[13] = self.bank[new][0];
        self.r[14] = self.bank[new][1];
        self.spsr = self.spsr_bank[new];
        self.mode = mode;
    }

    /// Reads a register through the user bank, for `LDM/STM` with the S bit.
    fn user_reg(&self, index: usize) -> u32 {
        match index {
            8..=12 if self.mode == MODE_FIQ => self.usr_r8_12[index - 8],
            13 | 14 if bank_of(self.mode) != 0 => self.bank[0][index - 13],
            _ => self.r[index],
        }
    }

    fn set_user_reg(&mut self, index: usize, value: u32) {
        match index {
            8..=12 if self.mode == MODE_FIQ => self.usr_r8_12[index - 8] = value,
            13 | 14 if bank_of(self.mode) != 0 => self.bank[0][index - 13] = value,
            _ => self.r[index] = value,
        }
    }

    #[inline]
    fn set_pc(&mut self, value: u32) {
        self.r[15] = value;
        self.flush = true;
    }

    /// Branch honouring the low bit as a state switch, as `BX` does.
    #[inline]
    fn branch_exchange(&mut self, value: u32) {
        self.thumb = value & 1 != 0;
        self.set_pc(value & if value & 1 != 0 { !1 } else { !3 });
    }

    pub fn exception(&mut self, vector: u32, mode: u32) {
        let return_address = self.r[15].wrapping_sub(if self.thumb { 2 } else { 4 });
        let cpsr = self.cpsr();
        self.switch_mode(mode);
        self.spsr = cpsr;
        self.r[14] = return_address;
        self.irq_disable = true;
        self.thumb = false;
        self.set_pc(vector);
    }

    /// True when the interrupt controller is asking for an IRQ we may take.
    pub fn irq_ready(&self, bus: &Bus) -> bool {
        !self.irq_disable && bus.irq_pending()
    }

    /// Address of the instruction that runs next. While a branch is pending
    /// r15 already holds the target, since the pipeline has not refilled yet.
    pub fn pc(&self) -> u32 {
        if self.flush {
            return self.r[15];
        }
        let width = if self.thumb { 2 } else { 4 };
        self.r[15].wrapping_sub(width * 2)
    }

    /// Enters the IRQ vector. The handler returns with `subs pc, lr, #4`, so lr
    /// holds the address of the pending instruction plus four.
    fn take_irq(&mut self) {
        let return_address = self.pc().wrapping_add(4);
        let cpsr = self.cpsr();
        self.switch_mode(MODE_IRQ);
        self.spsr = cpsr;
        self.r[14] = return_address;
        self.irq_disable = true;
        self.thumb = false;
        self.set_pc(0x18);
        self.halted = false;
    }

    /// Redirects execution from outside the instruction stream, as SoftReset does.
    pub fn set_pc_external(&mut self, value: u32) {
        self.set_pc(value);
    }

    /// Refills both prefetch slots after a branch or exception.
    fn reload_pipeline(&mut self, bus: &mut Bus) {
        if self.thumb {
            let pc = self.r[15] & !1;
            self.pipeline[0] = bus.read16_code(pc) as u32;
            self.pipeline[1] = bus.read16_code(pc.wrapping_add(2)) as u32;
            self.r[15] = pc.wrapping_add(4);
        } else {
            let pc = self.r[15] & !3;
            self.pipeline[0] = bus.read32_code(pc);
            self.pipeline[1] = bus.read32_code(pc.wrapping_add(4));
            self.r[15] = pc.wrapping_add(8);
        }
        self.flush = false;
    }

    /// Runs one instruction (or idles one cycle) and returns the cycles spent.
    pub fn step(&mut self, bus: &mut Bus) -> u32 {
        if bus.halt_requested {
            bus.halt_requested = false;
            self.halted = true;
        }
        if let Some(flags) = self.intr_wait {
            if bus.bios_irq_flags() & flags != 0 {
                let remaining = bus.bios_irq_flags() & !flags;
                bus.set_bios_irq_flags(remaining);
                self.intr_wait = None;
                self.halted = false;
            } else {
                self.halted = true;
            }
        }
        if self.halted {
            if self.irq_ready(bus) {
                self.take_irq();
                bus.access_cycles = 0;
                self.reload_pipeline(bus);
                return 3;
            }
            // Idle until a timer, DMA or the PPU raises something.
            return bus.cycles_until_event().clamp(1, 64);
        }
        bus.access_cycles = 0;
        if self.flush {
            self.reload_pipeline(bus);
        }
        if self.irq_ready(bus) {
            self.take_irq();
            self.reload_pipeline(bus);
            return 3 + bus.access_cycles;
        }

        let op = self.pipeline[0];
        self.pipeline[0] = self.pipeline[1];
        if self.thumb {
            self.pipeline[1] = bus.read16_code(self.r[15]) as u32;
            self.execute_thumb(bus, op as u16);
            if !self.flush {
                self.r[15] = self.r[15].wrapping_add(2);
            }
        } else {
            self.pipeline[1] = bus.read32_code(self.r[15]);
            self.execute_arm(bus, op);
            if !self.flush {
                self.r[15] = self.r[15].wrapping_add(4);
            }
        }
        if self.flush {
            self.reload_pipeline(bus);
        }
        1 + bus.access_cycles
    }

    #[inline]
    fn check_condition(&self, condition: u32) -> bool {
        match condition {
            0x0 => self.z,
            0x1 => !self.z,
            0x2 => self.c,
            0x3 => !self.c,
            0x4 => self.n,
            0x5 => !self.n,
            0x6 => self.v,
            0x7 => !self.v,
            0x8 => self.c && !self.z,
            0x9 => !self.c || self.z,
            0xa => self.n == self.v,
            0xb => self.n != self.v,
            0xc => !self.z && self.n == self.v,
            0xd => self.z || self.n != self.v,
            _ => true,
        }
    }

    #[inline]
    fn set_nz(&mut self, value: u32) {
        self.n = value & 0x8000_0000 != 0;
        self.z = value == 0;
    }

    #[inline]
    fn add_with_flags(&mut self, lhs: u32, rhs: u32, carry_in: bool) -> u32 {
        let wide = lhs as u64 + rhs as u64 + carry_in as u64;
        let result = wide as u32;
        self.c = wide > u32::MAX as u64;
        self.v = (!(lhs ^ rhs) & (lhs ^ result)) & 0x8000_0000 != 0;
        self.set_nz(result);
        result
    }

    #[inline]
    fn sub_with_flags(&mut self, lhs: u32, rhs: u32, carry_in: bool) -> u32 {
        let wide = lhs as u64 + (!rhs) as u64 + carry_in as u64;
        let result = wide as u32;
        self.c = wide > u32::MAX as u64;
        self.v = ((lhs ^ rhs) & (lhs ^ result)) & 0x8000_0000 != 0;
        self.set_nz(result);
        result
    }
}

/// Barrel shifter. `carry` is the carry flag going in and coming out.
#[inline]
pub fn shift(kind: u32, value: u32, amount: u32, immediate: bool, carry: &mut bool) -> u32 {
    match kind {
        0 => {
            // LSL
            if amount == 0 {
                value
            } else if amount < 32 {
                *carry = value & (1 << (32 - amount)) != 0;
                value << amount
            } else {
                *carry = amount == 32 && value & 1 != 0;
                0
            }
        }
        1 => {
            // LSR; an immediate zero means 32.
            let amount = if immediate && amount == 0 { 32 } else { amount };
            if amount == 0 {
                value
            } else if amount < 32 {
                *carry = value & (1 << (amount - 1)) != 0;
                value >> amount
            } else {
                *carry = amount == 32 && value & 0x8000_0000 != 0;
                0
            }
        }
        2 => {
            // ASR; an immediate zero means 32.
            let amount = if immediate && amount == 0 { 32 } else { amount };
            if amount == 0 {
                value
            } else if amount < 32 {
                *carry = value & (1 << (amount - 1)) != 0;
                ((value as i32) >> amount) as u32
            } else {
                *carry = value & 0x8000_0000 != 0;
                if *carry {
                    u32::MAX
                } else {
                    0
                }
            }
        }
        _ => {
            // ROR, or RRX when the immediate amount is zero.
            if amount == 0 {
                if immediate {
                    let result = (*carry as u32) << 31 | value >> 1;
                    *carry = value & 1 != 0;
                    result
                } else {
                    value
                }
            } else if amount & 31 == 0 {
                *carry = value & 0x8000_0000 != 0;
                value
            } else {
                let amount = amount & 31;
                *carry = value & (1 << (amount - 1)) != 0;
                value.rotate_right(amount)
            }
        }
    }
}

impl crate::state::Snapshot for Cpu {
    fn save(&self, writer: &mut crate::state::Writer) {
        for register in self.r {
            writer.u32(register);
        }
        writer.bool(self.n);
        writer.bool(self.z);
        writer.bool(self.c);
        writer.bool(self.v);
        writer.bool(self.irq_disable);
        writer.bool(self.fiq_disable);
        writer.bool(self.thumb);
        writer.u32(self.mode);
        writer.u32(self.spsr);
        for saved in self.spsr_bank {
            writer.u32(saved);
        }
        for bank in self.bank {
            writer.u32(bank[0]);
            writer.u32(bank[1]);
        }
        for register in self.usr_r8_12 {
            writer.u32(register);
        }
        for register in self.fiq_r8_12 {
            writer.u32(register);
        }
        writer.bool(self.halted);
        writer.option_u16(self.intr_wait);
        writer.bool(self.flush);
        writer.u32(self.pipeline[0]);
        writer.u32(self.pipeline[1]);
    }

    fn load(&mut self, reader: &mut crate::state::Reader) -> Result<(), crate::state::StateError> {
        for register in self.r.iter_mut() {
            *register = reader.u32()?;
        }
        self.n = reader.bool()?;
        self.z = reader.bool()?;
        self.c = reader.bool()?;
        self.v = reader.bool()?;
        self.irq_disable = reader.bool()?;
        self.fiq_disable = reader.bool()?;
        self.thumb = reader.bool()?;
        self.mode = reader.u32()?;
        self.spsr = reader.u32()?;
        for saved in self.spsr_bank.iter_mut() {
            *saved = reader.u32()?;
        }
        for bank in self.bank.iter_mut() {
            bank[0] = reader.u32()?;
            bank[1] = reader.u32()?;
        }
        for register in self.usr_r8_12.iter_mut() {
            *register = reader.u32()?;
        }
        for register in self.fiq_r8_12.iter_mut() {
            *register = reader.u32()?;
        }
        self.halted = reader.bool()?;
        self.intr_wait = reader.option_u16()?;
        self.flush = reader.bool()?;
        self.pipeline[0] = reader.u32()?;
        self.pipeline[1] = reader.u32()?;
        Ok(())
    }
}
