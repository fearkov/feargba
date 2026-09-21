//! The four hardware timers, including cascade mode.

const PRESCALER_SHIFT: [u32; 4] = [0, 6, 8, 10];

#[derive(Clone, Copy, Default)]
pub struct Timer {
    pub counter: u16,
    pub reload: u16,
    pub control: u16,
    /// Cycles accumulated below the prescaler threshold.
    residue: u32,
}

impl Timer {
    fn enabled(&self) -> bool {
        self.control & 0x80 != 0
    }
    fn cascade(&self) -> bool {
        self.control & 0x04 != 0
    }
    fn irq(&self) -> bool {
        self.control & 0x40 != 0
    }
}

#[derive(Default)]
pub struct Timers {
    pub channel: [Timer; 4],
}

impl Timers {
    pub fn read(&self, index: usize, half: usize) -> u16 {
        if half == 0 {
            self.channel[index].counter
        } else {
            self.channel[index].control
        }
    }

    pub fn write_reload(&mut self, index: usize, value: u16) {
        self.channel[index].reload = value;
    }

    pub fn write_control(&mut self, index: usize, value: u16) {
        let timer = &mut self.channel[index];
        let was_enabled = timer.enabled();
        timer.control = value & 0xc7;
        if !was_enabled && timer.enabled() {
            timer.counter = timer.reload;
            timer.residue = 0;
        }
    }

    /// Advances every timer. Returns the IRQ mask to raise and which of the
    /// first two timers overflowed, which the sound FIFOs are clocked from.
    pub fn tick(&mut self, cycles: u32) -> (u16, u8) {
        let mut irq = 0u16;
        let mut fifo = 0u8;
        let mut carried = 0u32;
        for index in 0..4 {
            let timer = &mut self.channel[index];
            if !timer.enabled() {
                carried = 0;
                continue;
            }
            let increments = if timer.cascade() && index > 0 {
                carried
            } else {
                timer.residue += cycles;
                let shift = PRESCALER_SHIFT[(timer.control & 3) as usize];
                let steps = timer.residue >> shift;
                timer.residue -= steps << shift;
                steps
            };
            carried = 0;
            if increments == 0 {
                continue;
            }
            let total = timer.counter as u32 + increments;
            if total > 0xffff {
                let period = 0x1_0000 - timer.reload as u32;
                let excess = total - 0x1_0000;
                let overflows = excess / period + 1;
                timer.counter = (timer.reload as u32 + excess % period) as u16;
                carried = overflows;
                if timer.irq() {
                    irq |= 1 << (3 + index);
                }
                if index < 2 {
                    fifo |= 1 << index;
                }
            } else {
                timer.counter = total as u16;
            }
        }
        (irq, fifo)
    }

    /// Cycles until the soonest overflow, used to skip ahead while halted.
    pub fn cycles_until_overflow(&self) -> u32 {
        let mut soonest = u32::MAX;
        for timer in &self.channel {
            if !timer.enabled() || timer.cascade() {
                continue;
            }
            let shift = PRESCALER_SHIFT[(timer.control & 3) as usize];
            let remaining = (0x1_0000 - timer.counter as u32) << shift;
            soonest = soonest.min(remaining.saturating_sub(timer.residue).max(1));
        }
        soonest
    }
}

impl crate::state::Snapshot for Timers {
    fn save(&self, writer: &mut crate::state::Writer) {
        for timer in &self.channel {
            writer.u16(timer.counter);
            writer.u16(timer.reload);
            writer.u16(timer.control);
            writer.u32(timer.residue);
        }
    }

    fn load(&mut self, reader: &mut crate::state::Reader) -> Result<(), crate::state::StateError> {
        for timer in &mut self.channel {
            timer.counter = reader.u16()?;
            timer.reload = reader.u16()?;
            timer.control = reader.u16()?;
            timer.residue = reader.u32()?;
        }
        Ok(())
    }
}
