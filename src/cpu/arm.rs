//! ARM state instruction execution.

use super::{shift, Cpu, MODE_SYS, MODE_UND, MODE_USR};
use crate::bus::Bus;

impl Cpu {
    pub(super) fn execute_arm(&mut self, bus: &mut Bus, op: u32) {
        let condition = op >> 28;
        if condition != 0xe && !self.check_condition(condition) {
            return;
        }
        match (op >> 25) & 7 {
            0 => {
                if op & 0x0fff_fff0 == 0x012f_ff10 {
                    let target = self.r[(op & 0xf) as usize];
                    self.branch_exchange(target);
                } else if op & 0x90 == 0x90 {
                    if (op >> 5) & 3 == 0 {
                        if op & 0x0100_0000 != 0 {
                            self.arm_swap(bus, op);
                        } else if op & 0x0080_0000 != 0 {
                            self.arm_multiply_long(op);
                        } else {
                            self.arm_multiply(op);
                        }
                    } else {
                        self.arm_halfword_transfer(bus, op);
                    }
                } else if op & 0x0190_0000 == 0x0100_0000 {
                    self.arm_psr_transfer(op);
                } else {
                    self.arm_data_processing(op);
                }
            }
            1 => {
                if op & 0x0190_0000 == 0x0100_0000 {
                    self.arm_psr_transfer(op);
                } else {
                    self.arm_data_processing(op);
                }
            }
            2 | 3 => {
                // A set bit 4 in the register-offset form is undefined space.
                if (op >> 25) & 1 == 1 && op & 0x10 != 0 {
                    self.exception(0x04, MODE_UND);
                } else {
                    self.arm_single_transfer(bus, op);
                }
            }
            4 => self.arm_block_transfer(bus, op),
            5 => {
                let offset = ((op & 0x00ff_ffff) << 8) as i32 >> 6;
                if op & 0x0100_0000 != 0 {
                    self.r[14] = self.r[15].wrapping_sub(4);
                }
                let target = self.r[15].wrapping_add(offset as u32);
                self.set_pc(target & !3);
            }
            7 if op & 0x0f00_0000 == 0x0f00_0000 => {
                let comment = (op >> 16) & 0xff;
                self.software_interrupt(bus, comment as u8);
            }
            _ => self.exception(0x04, MODE_UND),
        }
    }

    fn arm_data_processing(&mut self, op: u32) {
        let immediate = op & 1 << 25 != 0;
        let set_flags = op & 1 << 20 != 0;
        let opcode = (op >> 21) & 0xf;
        let rn = ((op >> 16) & 0xf) as usize;
        let rd = ((op >> 12) & 0xf) as usize;
        let mut carry = self.c;
        let operand = if immediate {
            let amount = ((op >> 8) & 0xf) * 2;
            let value = op & 0xff;
            if amount != 0 {
                carry = value.rotate_right(amount) & 0x8000_0000 != 0;
            }
            value.rotate_right(amount)
        } else {
            let kind = (op >> 5) & 3;
            let rm = (op & 0xf) as usize;
            if op & 1 << 4 != 0 {
                // Shift amount from a register: r15 reads four bytes further on.
                let amount = self.r[((op >> 8) & 0xf) as usize] & 0xff;
                let value = if rm == 15 {
                    self.r[15].wrapping_add(4)
                } else {
                    self.r[rm]
                };
                shift(kind, value, amount, false, &mut carry)
            } else {
                let amount = (op >> 7) & 0x1f;
                shift(kind, self.r[rm], amount, true, &mut carry)
            }
        };
        let lhs = if rn == 15 && !immediate && op & 1 << 4 != 0 {
            self.r[15].wrapping_add(4)
        } else {
            self.r[rn]
        };

        // TSTP/TEQP/CMPP/CMNP: with S set and r15 as destination these copy
        // SPSR into CPSR rather than comparing.
        if set_flags && rd == 15 && matches!(opcode, 0x8..=0xb) {
            if self.mode != MODE_USR && self.mode != MODE_SYS {
                let spsr = self.spsr;
                self.set_cpsr(spsr, true);
            }
            return;
        }

        let result = match opcode {
            0x0 => lhs & operand,
            0x1 => lhs ^ operand,
            0x2 => {
                if set_flags {
                    self.sub_with_flags(lhs, operand, true)
                } else {
                    lhs.wrapping_sub(operand)
                }
            }
            0x3 => {
                if set_flags {
                    self.sub_with_flags(operand, lhs, true)
                } else {
                    operand.wrapping_sub(lhs)
                }
            }
            0x4 => {
                if set_flags {
                    self.add_with_flags(lhs, operand, false)
                } else {
                    lhs.wrapping_add(operand)
                }
            }
            0x5 => {
                let carry_in = self.c;
                if set_flags {
                    self.add_with_flags(lhs, operand, carry_in)
                } else {
                    lhs.wrapping_add(operand).wrapping_add(carry_in as u32)
                }
            }
            0x6 => {
                let carry_in = self.c;
                if set_flags {
                    self.sub_with_flags(lhs, operand, carry_in)
                } else {
                    lhs.wrapping_sub(operand)
                        .wrapping_sub(1 - carry_in as u32)
                }
            }
            0x7 => {
                let carry_in = self.c;
                if set_flags {
                    self.sub_with_flags(operand, lhs, carry_in)
                } else {
                    operand
                        .wrapping_sub(lhs)
                        .wrapping_sub(1 - carry_in as u32)
                }
            }
            0x8 => lhs & operand,
            0x9 => lhs ^ operand,
            0xa => self.sub_with_flags(lhs, operand, true),
            0xb => self.add_with_flags(lhs, operand, false),
            0xc => lhs | operand,
            0xd => operand,
            0xe => lhs & !operand,
            _ => !operand,
        };

        let logical = matches!(opcode, 0x0 | 0x1 | 0x8 | 0x9 | 0xc | 0xd | 0xe | 0xf);
        let writes = !matches!(opcode, 0x8..=0xb);

        if set_flags && logical {
            self.c = carry;
        }
        if !writes {
            if set_flags && logical {
                self.set_nz(result);
            }
            return;
        }

        if rd == 15 {
            if set_flags {
                let spsr = self.spsr;
                self.set_cpsr(spsr, true);
            }
            if self.thumb {
                self.set_pc(result & !1);
            } else {
                self.set_pc(result & !3);
            }
        } else {
            self.r[rd] = result;
            // The arithmetic opcodes already wrote N and Z through their helpers.
            if set_flags && logical {
                self.set_nz(result);
            }
        }
    }

    fn arm_psr_transfer(&mut self, op: u32) {
        let spsr = op & 1 << 22 != 0;
        if op & 1 << 21 == 0 {
            // MRS
            let rd = ((op >> 12) & 0xf) as usize;
            self.r[rd] = if spsr { self.spsr } else { self.cpsr() };
            return;
        }
        // MSR
        let value = if op & 1 << 25 != 0 {
            (op & 0xff).rotate_right(((op >> 8) & 0xf) * 2)
        } else {
            self.r[(op & 0xf) as usize]
        };
        let fields = (op >> 16) & 0xf;
        let mut mask = 0u32;
        if fields & 1 != 0 {
            mask |= 0x0000_00ff;
        }
        if fields & 2 != 0 {
            mask |= 0x0000_ff00;
        }
        if fields & 4 != 0 {
            mask |= 0x00ff_0000;
        }
        if fields & 8 != 0 {
            mask |= 0xff00_0000;
        }
        if spsr {
            self.spsr = (self.spsr & !mask) | (value & mask);
        } else {
            if self.mode == MODE_USR {
                mask &= 0xf000_0000;
            }
            let updated = (self.cpsr() & !mask) | (value & mask);
            self.set_cpsr(updated, mask & 0xff != 0);
        }
    }

    fn arm_multiply(&mut self, op: u32) {
        let rd = ((op >> 16) & 0xf) as usize;
        let rn = ((op >> 12) & 0xf) as usize;
        let rs = ((op >> 8) & 0xf) as usize;
        let rm = (op & 0xf) as usize;
        let mut result = self.r[rm].wrapping_mul(self.r[rs]);
        if op & 1 << 21 != 0 {
            result = result.wrapping_add(self.r[rn]);
        }
        self.r[rd] = result;
        if op & 1 << 20 != 0 {
            self.set_nz(result);
        }
    }

    fn arm_multiply_long(&mut self, op: u32) {
        let rd_high = ((op >> 16) & 0xf) as usize;
        let rd_low = ((op >> 12) & 0xf) as usize;
        let rs = ((op >> 8) & 0xf) as usize;
        let rm = (op & 0xf) as usize;
        let signed = op & 1 << 22 != 0;
        let accumulate = op & 1 << 21 != 0;
        let mut result = if signed {
            ((self.r[rm] as i32 as i64).wrapping_mul(self.r[rs] as i32 as i64)) as u64
        } else {
            (self.r[rm] as u64).wrapping_mul(self.r[rs] as u64)
        };
        if accumulate {
            result = result.wrapping_add((self.r[rd_high] as u64) << 32 | self.r[rd_low] as u64);
        }
        self.r[rd_low] = result as u32;
        self.r[rd_high] = (result >> 32) as u32;
        if op & 1 << 20 != 0 {
            self.n = result & 1 << 63 != 0;
            self.z = result == 0;
        }
    }

    fn arm_swap(&mut self, bus: &mut Bus, op: u32) {
        let rn = ((op >> 16) & 0xf) as usize;
        let rd = ((op >> 12) & 0xf) as usize;
        let rm = (op & 0xf) as usize;
        let address = self.r[rn];
        if op & 1 << 22 != 0 {
            let old = bus.read8(address) as u32;
            bus.write8(address, self.r[rm] as u8);
            self.r[rd] = old;
        } else {
            let old = bus.read32(address).rotate_right((address & 3) * 8);
            bus.write32(address, self.r[rm]);
            self.r[rd] = old;
        }
    }

    fn arm_halfword_transfer(&mut self, bus: &mut Bus, op: u32) {
        let pre = op & 1 << 24 != 0;
        let up = op & 1 << 23 != 0;
        let immediate = op & 1 << 22 != 0;
        let writeback = op & 1 << 21 != 0;
        let load = op & 1 << 20 != 0;
        let rn = ((op >> 16) & 0xf) as usize;
        let rd = ((op >> 12) & 0xf) as usize;
        let offset = if immediate {
            ((op >> 4) & 0xf0) | (op & 0xf)
        } else {
            self.r[(op & 0xf) as usize]
        };
        let base = self.r[rn];
        let offset_address = if up {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let address = if pre { offset_address } else { base };

        if load {
            let value = match (op >> 5) & 3 {
                1 => {
                    let half = bus.read16(address) as u32;
                    if address & 1 != 0 {
                        half.rotate_right(8)
                    } else {
                        half
                    }
                }
                2 => bus.read8(address) as i8 as i32 as u32,
                _ => {
                    // A misaligned LDRSH reads a signed byte instead.
                    if address & 1 != 0 {
                        bus.read8(address) as i8 as i32 as u32
                    } else {
                        bus.read16(address) as i16 as i32 as u32
                    }
                }
            };
            if writeback || !pre {
                self.r[rn] = offset_address;
            }
            if rd == 15 {
                self.set_pc(value & !3);
            } else {
                self.r[rd] = value;
            }
        } else {
            let value = if rd == 15 {
                self.r[15].wrapping_add(4)
            } else {
                self.r[rd]
            };
            bus.write16(address, value as u16);
            if writeback || !pre {
                self.r[rn] = offset_address;
            }
        }
    }

    fn arm_single_transfer(&mut self, bus: &mut Bus, op: u32) {
        let register_offset = op & 1 << 25 != 0;
        let pre = op & 1 << 24 != 0;
        let up = op & 1 << 23 != 0;
        let byte = op & 1 << 22 != 0;
        let writeback = op & 1 << 21 != 0;
        let load = op & 1 << 20 != 0;
        let rn = ((op >> 16) & 0xf) as usize;
        let rd = ((op >> 12) & 0xf) as usize;
        let offset = if register_offset {
            let mut carry = self.c;
            let kind = (op >> 5) & 3;
            let amount = (op >> 7) & 0x1f;
            shift(kind, self.r[(op & 0xf) as usize], amount, true, &mut carry)
        } else {
            op & 0xfff
        };
        let base = self.r[rn];
        let offset_address = if up {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let address = if pre { offset_address } else { base };

        if load {
            let value = if byte {
                bus.read8(address) as u32
            } else {
                bus.read32(address).rotate_right((address & 3) * 8)
            };
            if (writeback || !pre) && rn != rd {
                self.r[rn] = offset_address;
            }
            if rd == 15 {
                self.set_pc(value & !3);
            } else {
                self.r[rd] = value;
            }
        } else {
            let value = if rd == 15 {
                self.r[15].wrapping_add(4)
            } else {
                self.r[rd]
            };
            if byte {
                bus.write8(address, value as u8);
            } else {
                bus.write32(address, value);
            }
            if writeback || !pre {
                self.r[rn] = offset_address;
            }
        }
    }

    fn arm_block_transfer(&mut self, bus: &mut Bus, op: u32) {
        let pre = op & 1 << 24 != 0;
        let up = op & 1 << 23 != 0;
        let user_bank = op & 1 << 22 != 0;
        let writeback = op & 1 << 21 != 0;
        let load = op & 1 << 20 != 0;
        let rn = ((op >> 16) & 0xf) as usize;
        let mut list = op & 0xffff;
        let base = self.r[rn];

        // An empty list transfers r15 alone and moves the base by 0x40.
        let empty = list == 0;
        if empty {
            list = 1 << 15;
        }
        let count = list.count_ones();
        let width = if empty { 0x40 } else { count * 4 };

        let mut address = if up { base } else { base.wrapping_sub(width) };
        let final_base = if up {
            base.wrapping_add(width)
        } else {
            base.wrapping_sub(width)
        };
        if up == pre {
            address = address.wrapping_add(4);
        }
        address &= !3;

        let transfer_pc = list & 1 << 15 != 0;
        let swap_bank = user_bank && !(load && transfer_pc);

        // A stored base reads back as the updated value unless it comes first.
        if writeback && !load && rn != list.trailing_zeros() as usize {
            self.r[rn] = final_base;
        }

        for register in 0..16 {
            if list & 1 << register == 0 {
                continue;
            }
            if load {
                let value = bus.read32(address);
                if swap_bank {
                    self.set_user_reg(register, value);
                } else if register == 15 {
                    self.set_pc(value & !3);
                } else {
                    self.r[register] = value;
                }
            } else {
                let value = if register == 15 {
                    self.r[15].wrapping_add(4)
                } else if swap_bank {
                    self.user_reg(register)
                } else {
                    self.r[register]
                };
                bus.write32(address, value);
            }
            address = address.wrapping_add(4);
        }

        if writeback && (!load || list & 1 << rn == 0) {
            self.r[rn] = final_base;
        }

        if load && transfer_pc && user_bank {
            let spsr = self.spsr;
            self.set_cpsr(spsr, true);
            let pc = self.r[15];
            if self.thumb {
                self.set_pc(pc & !1);
            } else {
                self.set_pc(pc & !3);
            }
        }
    }
}
