//! High level BIOS: the SWI calls plus a small real-code interrupt stub.

use crate::bus::Bus;
use crate::cpu::{Cpu, MODE_SVC, MODE_SYS};

/// Builds a 16K BIOS image containing the interrupt dispatcher games rely on.
/// Everything else is handled by intercepting SWI in the core.
pub fn hle_image() -> Vec<u8> {
    let mut bios = vec![0u8; 0x4000];
    let mut put = |address: usize, word: u32| {
        bios[address..address + 4].copy_from_slice(&word.to_le_bytes());
    };
    // Reset and the unused vectors simply spin.
    put(0x00, 0xea00_002e); // b 0xc0
    put(0x08, 0xe1a0_0000); // nop (SWI is intercepted)
    put(0x0c, 0xeafffffe);
    put(0x10, 0xeafffffe);
    put(0x14, 0xeafffffe);
    put(0x18, 0xea00_0042); // b 0x128
    put(0x1c, 0xeafffffe);
    put(0xc0, 0xeafffffe);

    // The stock BIOS interrupt handler: save, call [0x03007FFC], restore.
    put(0x128, 0xe92d_500f); // stmfd sp!, {r0-r3, r12, lr}
    put(0x12c, 0xe3a0_0301); // mov   r0, #0x04000000
    put(0x130, 0xe28f_e000); // add   lr, pc, #0
    put(0x134, 0xe510_f004); // ldr   pc, [r0, #-4]
    put(0x138, 0xe8bd_500f); // ldmfd sp!, {r0-r3, r12, lr}
    put(0x13c, 0xe25e_f004); // subs  pc, lr, #4
    // A game that reads the BIOS region from outside sees the last word the
    // BIOS prefetched, so keep the stock word that follows the handler.
    put(0x144, 0xe55e_c002); // ldrb  r12, [lr, #-2]
    bios
}

impl Cpu {
    pub fn software_interrupt(&mut self, bus: &mut Bus, number: u8) {
        if !bus.use_hle_bios {
            self.exception(0x08, MODE_SVC);
            return;
        }
        match number {
            0x00 => self.soft_reset(bus),
            0x01 => self.register_ram_reset(bus),
            0x02 => self.halted = true,
            0x03 => self.halted = true,
            0x04 => {
                let discard = self.r[0] != 0;
                let flags = self.r[1] as u16;
                self.begin_intr_wait(bus, discard, flags);
            }
            0x05 => self.begin_intr_wait(bus, true, 1),
            0x06 => self.divide(self.r[0] as i32, self.r[1] as i32),
            0x07 => self.divide(self.r[1] as i32, self.r[0] as i32),
            0x08 => self.r[0] = (self.r[0] as f64).sqrt() as u32,
            0x09 => {
                let value = self.r[0] as i16 as f64 / 16384.0;
                self.r[0] = ((value.atan() / std::f64::consts::TAU) * 65536.0) as i32 as u32 & 0xffff;
            }
            0x0a => {
                let x = self.r[0] as i16 as f64;
                let y = self.r[1] as i16 as f64;
                let angle = y.atan2(x) / std::f64::consts::TAU * 65536.0;
                self.r[0] = (angle.round() as i32) as u32 & 0xffff;
            }
            0x0b => self.cpu_set(bus),
            0x0c => self.cpu_fast_set(bus),
            0x0d => self.r[0] = 0xbaae_187f,
            0x0e => self.bg_affine_set(bus),
            0x0f => self.obj_affine_set(bus),
            0x10 => self.bit_unpack(bus),
            0x11 | 0x12 => self.lz77_uncomp(bus),
            0x13 => self.huff_uncomp(bus),
            0x14 | 0x15 => self.rl_uncomp(bus),
            0x16 | 0x17 => self.diff_unfilter(bus, false),
            0x18 => self.diff_unfilter(bus, true),
            0x19 => {
                let bias = (self.r[0] & 0x3ff) as u16;
                bus.apu.bias = (bus.apu.bias & !0x3fe) | (bias << 1) & 0x3fe;
            }
            0x1f => {} // MidiKey2Freq and the sound driver entries are no-ops.
            0x26 => self.soft_reset(bus),
            0x27 => self.halted = true,
            _ => {}
        }
        // The real BIOS leaves this word on the bus when a SWI returns.
        bus.set_bios_latch(0xe3a0_2004);
    }

    fn begin_intr_wait(&mut self, bus: &mut Bus, discard: bool, flags: u16) {
        if discard {
            let current = bus.bios_irq_flags();
            bus.set_bios_irq_flags(current & !flags);
        }
        bus.master_enable = true;
        self.irq_disable = false;
        self.intr_wait = Some(flags);
        self.halted = true;
    }

    fn soft_reset(&mut self, bus: &mut Bus) {
        let entry_flag = bus.iwram[0x7ffa];
        for byte in bus.iwram[0x7e00..0x8000].iter_mut() {
            *byte = 0;
        }
        self.r = [0; 16];
        self.switch_mode(MODE_SVC);
        self.r[13] = 0x0300_7fe0;
        self.switch_mode(crate::cpu::MODE_IRQ);
        self.r[13] = 0x0300_7fa0;
        self.switch_mode(MODE_SYS);
        self.r[13] = 0x0300_7f00;
        self.thumb = false;
        self.irq_disable = false;
        self.halted = false;
        self.intr_wait = None;
        let target = if entry_flag != 0 {
            0x0200_0000
        } else {
            0x0800_0000
        };
        self.set_pc_external(target);
    }

    fn register_ram_reset(&mut self, bus: &mut Bus) {
        let flags = self.r[0];
        if flags & 0x01 != 0 {
            bus.ewram.fill(0);
        }
        if flags & 0x02 != 0 {
            bus.iwram[..0x7e00].fill(0);
        }
        if flags & 0x04 != 0 {
            bus.ppu.palette.fill(0);
        }
        if flags & 0x08 != 0 {
            bus.ppu.vram.fill(0);
        }
        if flags & 0x10 != 0 {
            bus.ppu.oam.fill(0);
        }
        if flags & 0x20 != 0 {
            for offset in (0x120..0x12c).step_by(2) {
                bus.write16_raw(0x0400_0000 + offset, 0);
            }
        }
        if flags & 0x40 != 0 {
            for offset in (0x060..0x0a8).step_by(2) {
                bus.write16_raw(0x0400_0000 + offset, 0);
            }
        }
        if flags & 0x80 != 0 {
            for offset in (0x000..0x060).step_by(2) {
                bus.write16_raw(0x0400_0000 + offset, 0);
            }
            for offset in (0x0b0..0x100).step_by(2) {
                bus.write16_raw(0x0400_0000 + offset, 0);
            }
            for offset in (0x130..0x140).step_by(2) {
                bus.write16_raw(0x0400_0000 + offset, 0);
            }
            for offset in (0x200..0x20c).step_by(2) {
                bus.write16_raw(0x0400_0000 + offset, 0);
            }
        }
        bus.ppu.dispcnt |= 0x0080;
    }

    fn divide(&mut self, dividend: i32, divisor: i32) {
        if divisor == 0 {
            return;
        }
        let quotient = dividend.wrapping_div(divisor);
        let remainder = dividend.wrapping_rem(divisor);
        self.r[0] = quotient as u32;
        self.r[1] = remainder as u32;
        self.r[3] = quotient.unsigned_abs();
    }

    fn cpu_set(&mut self, bus: &mut Bus) {
        let mut source = self.r[0];
        let mut destination = self.r[1];
        let control = self.r[2];
        let count = control & 0x000f_ffff;
        let fill = control & 1 << 24 != 0;
        let words = control & 1 << 26 != 0;
        if words {
            source &= !3;
            destination &= !3;
            let mut value = bus.read32(source);
            for _ in 0..count {
                if !fill {
                    value = bus.read32(source);
                    source = source.wrapping_add(4);
                }
                bus.write32(destination, value);
                destination = destination.wrapping_add(4);
            }
        } else {
            source &= !1;
            destination &= !1;
            let mut value = bus.read16(source);
            for _ in 0..count {
                if !fill {
                    value = bus.read16(source);
                    source = source.wrapping_add(2);
                }
                bus.write16(destination, value);
                destination = destination.wrapping_add(2);
            }
        }
    }

    fn cpu_fast_set(&mut self, bus: &mut Bus) {
        let mut source = self.r[0] & !3;
        let mut destination = self.r[1] & !3;
        let control = self.r[2];
        let count = (control & 0x000f_ffff).max(1);
        let fill = control & 1 << 24 != 0;
        let mut value = bus.read32(source);
        for _ in 0..count {
            if !fill {
                value = bus.read32(source);
                source = source.wrapping_add(4);
            }
            bus.write32(destination, value);
            destination = destination.wrapping_add(4);
        }
    }

    fn bg_affine_set(&mut self, bus: &mut Bus) {
        let mut source = self.r[0];
        let mut destination = self.r[1];
        for _ in 0..self.r[2] {
            let texture_x = bus.read32(source) as i32;
            let texture_y = bus.read32(source + 4) as i32;
            let screen_x = bus.read16(source + 8) as i16 as i32;
            let screen_y = bus.read16(source + 10) as i16 as i32;
            let scale_x = bus.read16(source + 12) as i16 as f64 / 256.0;
            let scale_y = bus.read16(source + 14) as i16 as f64 / 256.0;
            let theta = (bus.read16(source + 16) >> 8) as f64 / 128.0 * std::f64::consts::PI;
            source += 20;

            let (sin, cos) = theta.sin_cos();
            let pa = (scale_x * cos * 256.0) as i32;
            let pb = (-scale_x * sin * 256.0) as i32;
            let pc = (scale_y * sin * 256.0) as i32;
            let pd = (scale_y * cos * 256.0) as i32;
            bus.write16(destination, pa as u16);
            bus.write16(destination + 2, pb as u16);
            bus.write16(destination + 4, pc as u16);
            bus.write16(destination + 6, pd as u16);
            let start_x = texture_x - (pa * screen_x + pb * screen_y);
            let start_y = texture_y - (pc * screen_x + pd * screen_y);
            bus.write32(destination + 8, start_x as u32);
            bus.write32(destination + 12, start_y as u32);
            destination += 16;
        }
    }

    fn obj_affine_set(&mut self, bus: &mut Bus) {
        let mut source = self.r[0];
        let mut destination = self.r[1];
        let stride = self.r[3];
        for _ in 0..self.r[2] {
            let scale_x = bus.read16(source) as i16 as f64 / 256.0;
            let scale_y = bus.read16(source + 2) as i16 as f64 / 256.0;
            let theta = (bus.read16(source + 4) >> 8) as f64 / 128.0 * std::f64::consts::PI;
            source += 8;
            let (sin, cos) = theta.sin_cos();
            let pa = (scale_x * cos * 256.0) as i32 as u16;
            let pb = (-scale_x * sin * 256.0) as i32 as u16;
            let pc = (scale_y * sin * 256.0) as i32 as u16;
            let pd = (scale_y * cos * 256.0) as i32 as u16;
            bus.write16(destination, pa);
            bus.write16(destination + stride, pb);
            bus.write16(destination + stride * 2, pc);
            bus.write16(destination + stride * 3, pd);
            destination += stride * 4;
        }
    }

    fn bit_unpack(&mut self, bus: &mut Bus) {
        let mut source = self.r[0];
        let mut destination = self.r[1];
        let parameters = self.r[2];
        let length = bus.read16(parameters) as u32;
        let source_width = bus.read8(parameters + 2) as u32;
        let destination_width = bus.read8(parameters + 3) as u32;
        let offset_word = bus.read32(parameters + 4);
        let offset = offset_word & 0x7fff_ffff;
        let zero_offset = offset_word & 0x8000_0000 != 0;
        if source_width == 0 || destination_width == 0 {
            return;
        }

        let mask = (1u32 << source_width) - 1;
        let mut buffer = 0u32;
        let mut bits = 0u32;
        for index in 0..length {
            let byte = bus.read8(source + index) as u32;
            let mut consumed = 0;
            while consumed < 8 {
                let value = (byte >> consumed) & mask;
                consumed += source_width;
                let value = if value != 0 || zero_offset {
                    value + offset
                } else {
                    0
                };
                buffer |= value << bits;
                bits += destination_width;
                if bits == 32 {
                    bus.write32(destination, buffer);
                    destination += 4;
                    buffer = 0;
                    bits = 0;
                }
            }
        }
        source += length;
        let _ = source;
        if bits > 0 {
            bus.write32(destination, buffer);
        }
    }

    fn lz77_uncomp(&mut self, bus: &mut Bus) {
        let mut source = self.r[0] & !3;
        let destination = self.r[1];
        let header = bus.read32(source);
        source += 4;
        if header & 0xf0 != 0x10 {
            return;
        }
        let size = (header >> 8) as usize;
        let mut output: Vec<u8> = Vec::with_capacity(size);
        while output.len() < size {
            let flags = bus.read8(source);
            source += 1;
            for bit in (0..8).rev() {
                if output.len() >= size {
                    break;
                }
                if flags & 1 << bit == 0 {
                    output.push(bus.read8(source));
                    source += 1;
                } else {
                    let first = bus.read8(source) as usize;
                    let second = bus.read8(source + 1) as usize;
                    source += 2;
                    let length = (first >> 4) + 3;
                    let displacement = ((first & 0xf) << 8 | second) + 1;
                    for _ in 0..length {
                        if output.len() >= size {
                            break;
                        }
                        let index = output.len().wrapping_sub(displacement);
                        let byte = output.get(index).copied().unwrap_or(0);
                        output.push(byte);
                    }
                }
            }
        }
        write_block(bus, destination, &output);
        self.r[0] = source;
    }

    fn rl_uncomp(&mut self, bus: &mut Bus) {
        let mut source = self.r[0] & !3;
        let destination = self.r[1];
        let header = bus.read32(source);
        source += 4;
        if header & 0xf0 != 0x30 {
            return;
        }
        let size = (header >> 8) as usize;
        let mut output: Vec<u8> = Vec::with_capacity(size);
        while output.len() < size {
            let control = bus.read8(source);
            source += 1;
            if control & 0x80 != 0 {
                let count = (control & 0x7f) as usize + 3;
                let value = bus.read8(source);
                source += 1;
                for _ in 0..count {
                    if output.len() >= size {
                        break;
                    }
                    output.push(value);
                }
            } else {
                let count = (control & 0x7f) as usize + 1;
                for _ in 0..count {
                    if output.len() >= size {
                        break;
                    }
                    output.push(bus.read8(source));
                    source += 1;
                }
            }
        }
        write_block(bus, destination, &output);
        self.r[0] = source;
    }

    fn huff_uncomp(&mut self, bus: &mut Bus) {
        let source = self.r[0] & !3;
        let destination = self.r[1];
        let header = bus.read32(source);
        if header & 0xf0 != 0x20 {
            return;
        }
        let symbol_bits = header & 0xf;
        if symbol_bits == 0 {
            return;
        }
        let size = (header >> 8) as usize;
        let tree_base = source + 4;
        let tree_size = (bus.read8(tree_base) as u32 + 1) * 2;
        let mut stream = tree_base + tree_size;

        let mut output: Vec<u8> = Vec::with_capacity(size);
        let mut partial = 0u32;
        let mut partial_bits = 0u32;
        let mut node = tree_base + 1;
        let mut node_is_root = true;
        let mut word = bus.read32(stream);
        stream += 4;
        let mut remaining_bits = 32u32;

        while output.len() < size {
            if remaining_bits == 0 {
                word = bus.read32(stream);
                stream += 4;
                remaining_bits = 32;
            }
            let bit = word >> 31 & 1;
            word <<= 1;
            remaining_bits -= 1;

            let value = bus.read8(node);
            let offset = (value & 0x3f) as u32;
            let base = (node & !1) + offset * 2 + 2;
            let child = base + bit;
            let leaf_mask = if bit != 0 { 0x40 } else { 0x80 };
            let is_leaf = value & leaf_mask != 0 && !node_is_root;
            node_is_root = false;

            if is_leaf || (node == tree_base + 1 && value & leaf_mask != 0) {
                // Unreachable in practice; kept so malformed data terminates.
            }
            if value & leaf_mask != 0 {
                let data = bus.read8(child) as u32 & ((1 << symbol_bits) - 1);
                partial |= data << partial_bits;
                partial_bits += symbol_bits;
                if partial_bits == 32 {
                    output.extend_from_slice(&partial.to_le_bytes());
                    partial = 0;
                    partial_bits = 0;
                }
                node = tree_base + 1;
                node_is_root = true;
            } else {
                node = child;
            }
        }
        if partial_bits > 0 {
            output.extend_from_slice(&partial.to_le_bytes());
        }
        output.truncate(size);
        write_block(bus, destination, &output);
    }

    fn diff_unfilter(&mut self, bus: &mut Bus, halfwords: bool) {
        let mut source = self.r[0] & !3;
        let destination = self.r[1];
        let header = bus.read32(source);
        source += 4;
        let size = (header >> 8) as usize;
        let mut output: Vec<u8> = Vec::with_capacity(size);
        if halfwords {
            let mut previous = 0u16;
            while output.len() < size {
                previous = previous.wrapping_add(bus.read16(source));
                source += 2;
                output.extend_from_slice(&previous.to_le_bytes());
            }
        } else {
            let mut previous = 0u8;
            while output.len() < size {
                previous = previous.wrapping_add(bus.read8(source));
                source += 1;
                output.push(previous);
            }
        }
        output.truncate(size);
        write_block(bus, destination, &output);
    }
}

/// Writes decompressed data out in halfwords so VRAM accepts it.
fn write_block(bus: &mut Bus, destination: u32, data: &[u8]) {
    if destination & 1 != 0 {
        for (index, byte) in data.iter().enumerate() {
            bus.write8(destination + index as u32, *byte);
        }
        return;
    }
    let mut address = destination;
    let (pairs, remainder) = data.as_chunks::<2>();
    for pair in pairs {
        bus.write16(address, u16::from_le_bytes(*pair));
        address += 2;
    }
    if let Some(&last) = remainder.first() {
        let existing = bus.read16(address) & 0xff00;
        bus.write16(address, existing | last as u16);
    }
}
