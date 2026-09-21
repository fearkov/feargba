//! Thumb state instruction execution.

use super::{shift, Cpu};
use crate::bus::Bus;

impl Cpu {
    pub(super) fn execute_thumb(&mut self, bus: &mut Bus, op: u16) {
        let op = op as u32;
        match op >> 12 {
            0x0 | 0x1 => {
                if op & 0x1800 == 0x1800 {
                    self.thumb_add_subtract(op);
                } else {
                    self.thumb_shift_immediate(op);
                }
            }
            0x2 | 0x3 => self.thumb_immediate(op),
            0x4 => {
                if op & 0x0800 != 0 {
                    // LDR rd, [pc, #imm]
                    let rd = ((op >> 8) & 7) as usize;
                    let address = (self.r[15] & !2).wrapping_add((op & 0xff) << 2);
                    self.r[rd] = bus.read32(address);
                } else if op & 0x0400 != 0 {
                    self.thumb_high_register(op);
                } else {
                    self.thumb_alu(op);
                }
            }
            0x5 => self.thumb_register_offset(bus, op),
            0x6 | 0x7 => {
                // Word or byte with a 5 bit immediate offset.
                let rd = (op & 7) as usize;
                let rb = ((op >> 3) & 7) as usize;
                let byte = op & 0x1000 != 0;
                let load = op & 0x0800 != 0;
                let offset = if byte {
                    (op >> 6) & 0x1f
                } else {
                    ((op >> 6) & 0x1f) << 2
                };
                let address = self.r[rb].wrapping_add(offset);
                match (load, byte) {
                    (true, true) => self.r[rd] = bus.read8(address) as u32,
                    (true, false) => {
                        self.r[rd] = bus.read32(address).rotate_right((address & 3) * 8)
                    }
                    (false, true) => bus.write8(address, self.r[rd] as u8),
                    (false, false) => bus.write32(address, self.r[rd]),
                }
            }
            0x8 => {
                let rd = (op & 7) as usize;
                let rb = ((op >> 3) & 7) as usize;
                let address = self.r[rb].wrapping_add(((op >> 6) & 0x1f) << 1);
                if op & 0x0800 != 0 {
                    let value = bus.read16(address) as u32;
                    self.r[rd] = if address & 1 != 0 {
                        value.rotate_right(8)
                    } else {
                        value
                    };
                } else {
                    bus.write16(address, self.r[rd] as u16);
                }
            }
            0x9 => {
                let rd = ((op >> 8) & 7) as usize;
                let address = self.r[13].wrapping_add((op & 0xff) << 2);
                if op & 0x0800 != 0 {
                    self.r[rd] = bus.read32(address).rotate_right((address & 3) * 8);
                } else {
                    bus.write32(address, self.r[rd]);
                }
            }
            0xa => {
                let rd = ((op >> 8) & 7) as usize;
                let base = if op & 0x0800 != 0 {
                    self.r[13]
                } else {
                    self.r[15] & !2
                };
                self.r[rd] = base.wrapping_add((op & 0xff) << 2);
            }
            0xb => {
                if op & 0x0f00 == 0x0000 {
                    let amount = (op & 0x7f) << 2;
                    self.r[13] = if op & 0x80 != 0 {
                        self.r[13].wrapping_sub(amount)
                    } else {
                        self.r[13].wrapping_add(amount)
                    };
                } else if op & 0x0600 == 0x0400 {
                    self.thumb_push_pop(bus, op);
                }
            }
            0xc => self.thumb_block_transfer(bus, op),
            0xd => {
                let condition = (op >> 8) & 0xf;
                if condition == 0xf {
                    self.software_interrupt(bus, (op & 0xff) as u8);
                } else if condition != 0xe && self.check_condition(condition) {
                    let offset = ((op & 0xff) as u8 as i8 as i32) << 1;
                    let target = self.r[15].wrapping_add(offset as u32);
                    self.set_pc(target & !1);
                }
            }
            0xe => {
                let offset = (((op & 0x7ff) << 21) as i32) >> 20;
                let target = self.r[15].wrapping_add(offset as u32);
                self.set_pc(target & !1);
            }
            _ => {
                if op & 0x0800 == 0 {
                    // First half: bank the high part of the offset in lr.
                    let offset = (((op & 0x7ff) << 21) as i32) >> 9;
                    self.r[14] = self.r[15].wrapping_add(offset as u32);
                } else {
                    let return_address = self.r[15].wrapping_sub(2) | 1;
                    let target = self.r[14].wrapping_add((op & 0x7ff) << 1);
                    self.r[14] = return_address;
                    self.set_pc(target & !1);
                }
            }
        }
    }

    fn thumb_shift_immediate(&mut self, op: u32) {
        let rd = (op & 7) as usize;
        let rs = ((op >> 3) & 7) as usize;
        let amount = (op >> 6) & 0x1f;
        let kind = (op >> 11) & 3;
        let mut carry = self.c;
        let result = shift(kind, self.r[rs], amount, true, &mut carry);
        self.c = carry;
        self.r[rd] = result;
        self.set_nz(result);
    }

    fn thumb_add_subtract(&mut self, op: u32) {
        let rd = (op & 7) as usize;
        let rs = ((op >> 3) & 7) as usize;
        let operand = if op & 0x0400 != 0 {
            (op >> 6) & 7
        } else {
            self.r[((op >> 6) & 7) as usize]
        };
        let lhs = self.r[rs];
        self.r[rd] = if op & 0x0200 != 0 {
            self.sub_with_flags(lhs, operand, true)
        } else {
            self.add_with_flags(lhs, operand, false)
        };
    }

    fn thumb_immediate(&mut self, op: u32) {
        let rd = ((op >> 8) & 7) as usize;
        let value = op & 0xff;
        match (op >> 11) & 3 {
            0 => {
                self.r[rd] = value;
                self.set_nz(value);
            }
            1 => {
                let lhs = self.r[rd];
                self.sub_with_flags(lhs, value, true);
            }
            2 => {
                let lhs = self.r[rd];
                self.r[rd] = self.add_with_flags(lhs, value, false);
            }
            _ => {
                let lhs = self.r[rd];
                self.r[rd] = self.sub_with_flags(lhs, value, true);
            }
        }
    }

    fn thumb_alu(&mut self, op: u32) {
        let rd = (op & 7) as usize;
        let rs = ((op >> 3) & 7) as usize;
        let lhs = self.r[rd];
        let rhs = self.r[rs];
        let mut carry = self.c;
        match (op >> 6) & 0xf {
            0x0 => {
                let result = lhs & rhs;
                self.r[rd] = result;
                self.set_nz(result);
            }
            0x1 => {
                let result = lhs ^ rhs;
                self.r[rd] = result;
                self.set_nz(result);
            }
            0x2 | 0x3 | 0x4 | 0x7 => {
                let kind = match (op >> 6) & 0xf {
                    0x2 => 0,
                    0x3 => 1,
                    0x4 => 2,
                    _ => 3,
                };
                let result = shift(kind, lhs, rhs & 0xff, false, &mut carry);
                self.c = carry;
                self.r[rd] = result;
                self.set_nz(result);
            }
            0x5 => {
                let carry_in = self.c;
                self.r[rd] = self.add_with_flags(lhs, rhs, carry_in);
            }
            0x6 => {
                let carry_in = self.c;
                self.r[rd] = self.sub_with_flags(lhs, rhs, carry_in);
            }
            0x8 => self.set_nz(lhs & rhs),
            0x9 => self.r[rd] = self.sub_with_flags(0, rhs, true),
            0xa => {
                self.sub_with_flags(lhs, rhs, true);
            }
            0xb => {
                self.add_with_flags(lhs, rhs, false);
            }
            0xc => {
                let result = lhs | rhs;
                self.r[rd] = result;
                self.set_nz(result);
            }
            0xd => {
                let result = lhs.wrapping_mul(rhs);
                self.r[rd] = result;
                self.set_nz(result);
            }
            0xe => {
                let result = lhs & !rhs;
                self.r[rd] = result;
                self.set_nz(result);
            }
            _ => {
                let result = !rhs;
                self.r[rd] = result;
                self.set_nz(result);
            }
        }
    }

    fn thumb_high_register(&mut self, op: u32) {
        let rd = ((op & 7) | ((op >> 4) & 8)) as usize;
        let rs = ((op >> 3) & 0xf) as usize;
        let source = if rs == 15 { self.r[15] & !1 } else { self.r[rs] };
        match (op >> 8) & 3 {
            0 => {
                let result = self.r[rd].wrapping_add(source);
                if rd == 15 {
                    self.set_pc(result & !1);
                } else {
                    self.r[rd] = result;
                }
            }
            1 => {
                let lhs = self.r[rd];
                self.sub_with_flags(lhs, source, true);
            }
            2 => {
                if rd == 15 {
                    self.set_pc(source & !1);
                } else {
                    self.r[rd] = source;
                }
            }
            _ => self.branch_exchange(source),
        }
    }

    fn thumb_register_offset(&mut self, bus: &mut Bus, op: u32) {
        let rd = (op & 7) as usize;
        let rb = ((op >> 3) & 7) as usize;
        let ro = ((op >> 6) & 7) as usize;
        let address = self.r[rb].wrapping_add(self.r[ro]);
        if op & 0x0200 == 0 {
            match (op >> 10) & 3 {
                0 => bus.write32(address, self.r[rd]),
                1 => bus.write8(address, self.r[rd] as u8),
                2 => self.r[rd] = bus.read32(address).rotate_right((address & 3) * 8),
                _ => self.r[rd] = bus.read8(address) as u32,
            }
        } else {
            match (op >> 10) & 3 {
                0 => bus.write16(address, self.r[rd] as u16),
                1 => self.r[rd] = bus.read8(address) as i8 as i32 as u32,
                2 => {
                    let value = bus.read16(address) as u32;
                    self.r[rd] = if address & 1 != 0 {
                        value.rotate_right(8)
                    } else {
                        value
                    };
                }
                _ => {
                    self.r[rd] = if address & 1 != 0 {
                        bus.read8(address) as i8 as i32 as u32
                    } else {
                        bus.read16(address) as i16 as i32 as u32
                    }
                }
            }
        }
    }

    fn thumb_push_pop(&mut self, bus: &mut Bus, op: u32) {
        let load = op & 0x0800 != 0;
        let extra = op & 0x0100 != 0;
        let list = op & 0xff;
        let count = list.count_ones() + extra as u32;
        if count == 0 {
            // An empty list moves the stack by 0x40 and transfers r15.
            if load {
                let value = bus.read32(self.r[13]);
                self.set_pc(value & !1);
                self.r[13] = self.r[13].wrapping_add(0x40);
            } else {
                let address = self.r[13].wrapping_sub(0x40);
                bus.write32(address, self.r[15].wrapping_add(2));
                self.r[13] = address;
            }
            return;
        }
        if load {
            let mut address = self.r[13];
            for register in 0..8 {
                if list & 1 << register != 0 {
                    self.r[register] = bus.read32(address);
                    address = address.wrapping_add(4);
                }
            }
            if extra {
                let value = bus.read32(address);
                address = address.wrapping_add(4);
                self.set_pc(value & !1);
            }
            self.r[13] = address;
        } else {
            let mut address = self.r[13].wrapping_sub(count * 4);
            self.r[13] = address;
            for register in 0..8 {
                if list & 1 << register != 0 {
                    bus.write32(address, self.r[register]);
                    address = address.wrapping_add(4);
                }
            }
            if extra {
                bus.write32(address, self.r[14]);
            }
        }
    }

    fn thumb_block_transfer(&mut self, bus: &mut Bus, op: u32) {
        let load = op & 0x0800 != 0;
        let rb = ((op >> 8) & 7) as usize;
        let list = op & 0xff;
        let base = self.r[rb];
        if list == 0 {
            if load {
                let value = bus.read32(base & !3);
                self.set_pc(value & !1);
            } else {
                bus.write32(base & !3, self.r[15].wrapping_add(2));
            }
            self.r[rb] = base.wrapping_add(0x40);
            return;
        }
        let final_base = base.wrapping_add(list.count_ones() * 4);
        if !load && list & 1 << rb != 0 && list.trailing_zeros() as usize != rb {
            self.r[rb] = final_base;
        }
        let mut address = base & !3;
        for register in 0..8 {
            if list & 1 << register == 0 {
                continue;
            }
            if load {
                self.r[register] = bus.read32(address);
            } else {
                bus.write32(address, self.r[register]);
            }
            address = address.wrapping_add(4);
        }
        if !load || list & 1 << rb == 0 {
            self.r[rb] = final_base;
        }
    }
}
