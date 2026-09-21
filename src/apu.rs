//! Audio: the four PSG channels plus the two DirectSound FIFOs.

pub const SAMPLE_RATE: u32 = 32768;
const CYCLES_PER_SAMPLE: u32 = 16_777_216 / SAMPLE_RATE;
/// 16.78 MHz / 512 Hz frame sequencer.
const SEQUENCER_PERIOD: u32 = 32_768;

const DUTY: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 0],
];

#[derive(Default)]
struct Envelope {
    initial: u8,
    increase: bool,
    period: u8,
    volume: u8,
    timer: u8,
}

impl Envelope {
    fn trigger(&mut self) {
        self.volume = self.initial;
        self.timer = self.period;
    }
    fn tick(&mut self) {
        if self.period == 0 {
            return;
        }
        if self.timer > 0 {
            self.timer -= 1;
        }
        if self.timer == 0 {
            self.timer = self.period;
            if self.increase && self.volume < 15 {
                self.volume += 1;
            } else if !self.increase && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }
}

#[derive(Default)]
struct Square {
    enabled: bool,
    frequency: u16,
    timer: i32,
    phase: usize,
    duty: usize,
    length: u16,
    length_enabled: bool,
    envelope: Envelope,
    sweep_period: u8,
    sweep_decrease: bool,
    sweep_shift: u8,
    sweep_timer: u8,
    sweep_enabled: bool,
    sweep_shadow: u16,
}

impl Square {
    fn period(&self) -> i32 {
        (2048 - self.frequency as i32) * 4
    }
    fn trigger(&mut self) {
        self.enabled = true;
        self.timer = self.period();
        self.envelope.trigger();
        if self.length == 0 {
            self.length = 64;
        }
        self.sweep_shadow = self.frequency;
        self.sweep_timer = if self.sweep_period == 0 {
            8
        } else {
            self.sweep_period
        };
        self.sweep_enabled = self.sweep_period != 0 || self.sweep_shift != 0;
    }
    fn tick(&mut self, cycles: i32) {
        if !self.enabled {
            return;
        }
        self.timer -= cycles;
        while self.timer <= 0 {
            self.timer += self.period().max(1);
            self.phase = (self.phase + 1) & 7;
        }
    }
    /// Signed so a silent channel contributes nothing rather than a DC step.
    fn sample(&self) -> i16 {
        if !self.enabled || self.envelope.volume == 0 {
            return 0;
        }
        let volume = self.envelope.volume as i16;
        if DUTY[self.duty][self.phase] != 0 {
            volume
        } else {
            -volume
        }
    }
    fn tick_length(&mut self) {
        if self.length_enabled && self.length > 0 {
            self.length -= 1;
            if self.length == 0 {
                self.enabled = false;
            }
        }
    }
    fn tick_sweep(&mut self) {
        if !self.sweep_enabled {
            return;
        }
        if self.sweep_timer > 0 {
            self.sweep_timer -= 1;
        }
        if self.sweep_timer == 0 {
            self.sweep_timer = if self.sweep_period == 0 {
                8
            } else {
                self.sweep_period
            };
            if self.sweep_period != 0 && self.sweep_shift != 0 {
                let delta = self.sweep_shadow >> self.sweep_shift;
                let next = if self.sweep_decrease {
                    self.sweep_shadow.wrapping_sub(delta)
                } else {
                    self.sweep_shadow + delta
                };
                if next > 2047 {
                    self.enabled = false;
                } else {
                    self.sweep_shadow = next;
                    self.frequency = next;
                }
            }
        }
    }
}

#[derive(Default)]
struct Wave {
    enabled: bool,
    playing: bool,
    banks: [[u8; 16]; 2],
    bank: usize,
    two_banks: bool,
    position: usize,
    frequency: u16,
    timer: i32,
    volume: u8,
    force_75: bool,
    length: u16,
    length_enabled: bool,
}

impl Wave {
    fn period(&self) -> i32 {
        (2048 - self.frequency as i32) * 2
    }
    fn trigger(&mut self) {
        self.playing = true;
        self.position = 0;
        self.timer = self.period();
        if self.length == 0 {
            self.length = 256;
        }
    }
    fn tick(&mut self, cycles: i32) {
        if !self.playing || !self.enabled {
            return;
        }
        self.timer -= cycles;
        while self.timer <= 0 {
            self.timer += self.period().max(1);
            self.position += 1;
            if self.position >= 32 {
                self.position = 0;
                if self.two_banks {
                    self.bank ^= 1;
                }
            }
        }
    }
    fn sample(&self) -> i16 {
        if !self.playing || !self.enabled {
            return 0;
        }
        let byte = self.banks[self.bank][self.position / 2];
        let nibble = if self.position & 1 == 0 {
            byte >> 4
        } else {
            byte & 0xf
        } as i16;
        let centred = nibble - 8;
        if self.force_75 {
            centred * 3 / 4
        } else {
            match self.volume {
                0 => 0,
                1 => centred,
                2 => centred / 2,
                _ => centred / 4,
            }
        }
    }
    fn tick_length(&mut self) {
        if self.length_enabled && self.length > 0 {
            self.length -= 1;
            if self.length == 0 {
                self.playing = false;
            }
        }
    }
}

#[derive(Default)]
struct Noise {
    enabled: bool,
    lfsr: u16,
    timer: i32,
    divisor: u8,
    shift: u8,
    width7: bool,
    length: u16,
    length_enabled: bool,
    envelope: Envelope,
}

impl Noise {
    fn period(&self) -> i32 {
        let divisor = if self.divisor == 0 {
            8
        } else {
            self.divisor as i32 * 16
        };
        divisor << self.shift
    }
    fn trigger(&mut self) {
        self.enabled = true;
        self.lfsr = 0x7fff;
        self.timer = self.period();
        self.envelope.trigger();
        if self.length == 0 {
            self.length = 64;
        }
    }
    fn tick(&mut self, cycles: i32) {
        if !self.enabled {
            return;
        }
        self.timer -= cycles;
        while self.timer <= 0 {
            self.timer += self.period().max(1);
            let bit = (self.lfsr ^ (self.lfsr >> 1)) & 1;
            self.lfsr >>= 1;
            self.lfsr |= bit << 14;
            if self.width7 {
                self.lfsr = (self.lfsr & !0x40) | bit << 6;
            }
        }
    }
    fn sample(&self) -> i16 {
        if !self.enabled || self.envelope.volume == 0 {
            return 0;
        }
        let volume = self.envelope.volume as i16;
        if self.lfsr & 1 == 0 {
            volume
        } else {
            -volume
        }
    }
    fn tick_length(&mut self) {
        if self.length_enabled && self.length > 0 {
            self.length -= 1;
            if self.length == 0 {
                self.enabled = false;
            }
        }
    }
}

#[derive(Default)]
struct Fifo {
    data: [i8; 32],
    read: usize,
    write: usize,
    length: usize,
    current: i8,
}

impl Fifo {
    fn push(&mut self, value: i8) {
        if self.length < 32 {
            self.data[self.write] = value;
            self.write = (self.write + 1) & 31;
            self.length += 1;
        }
    }
    fn pop(&mut self) {
        if self.length > 0 {
            self.current = self.data[self.read];
            self.read = (self.read + 1) & 31;
            self.length -= 1;
        }
    }
    fn clear(&mut self) {
        self.read = 0;
        self.write = 0;
        self.length = 0;
        self.current = 0;
    }
}

pub struct Apu {
    square1: Square,
    square2: Square,
    wave: Wave,
    noise: Noise,
    fifo: [Fifo; 2],
    pub control_l: u16,
    pub control_h: u16,
    pub control_x: u16,
    pub bias: u16,
    sample_timer: u32,
    sequencer_timer: u32,
    sequencer_step: u32,
    pub buffer: Vec<i16>,
    pub enabled: bool,
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl Apu {
    pub fn new() -> Self {
        Self {
            square1: Square::default(),
            square2: Square::default(),
            wave: Wave::default(),
            noise: Noise::default(),
            fifo: [Fifo::default(), Fifo::default()],
            control_l: 0,
            control_h: 0,
            control_x: 0,
            bias: 0x200,
            sample_timer: 0,
            sequencer_timer: 0,
            sequencer_step: 0,
            buffer: Vec::with_capacity(4096),
            enabled: true,
        }
    }

    /// Called when a timer overflows; returns whether each FIFO wants a refill.
    pub fn timer_overflow(&mut self, timer: usize) -> [bool; 2] {
        let mut request = [false; 2];
        for (channel, wanted) in request.iter_mut().enumerate() {
            let selected = (self.control_h >> (10 + channel * 4)) & 1;
            if selected as usize != timer {
                continue;
            }
            self.fifo[channel].pop();
            if self.fifo[channel].length <= 16 {
                *wanted = true;
            }
        }
        request
    }

    pub fn push_fifo(&mut self, channel: usize, value: u32) {
        for shift in 0..4 {
            self.fifo[channel].push((value >> (shift * 8)) as i8);
        }
    }

    pub fn tick(&mut self, cycles: u32) {
        if self.control_x & 0x80 == 0 {
            // Master disable still advances the sample clock so audio keeps pace.
            self.sample_timer += cycles;
            while self.sample_timer >= CYCLES_PER_SAMPLE {
                self.sample_timer -= CYCLES_PER_SAMPLE;
                self.buffer.push(0);
                self.buffer.push(0);
            }
            return;
        }
        self.square1.tick(cycles as i32);
        self.square2.tick(cycles as i32);
        self.wave.tick(cycles as i32);
        self.noise.tick(cycles as i32);

        self.sequencer_timer += cycles;
        while self.sequencer_timer >= SEQUENCER_PERIOD {
            self.sequencer_timer -= SEQUENCER_PERIOD;
            match self.sequencer_step {
                0 | 4 => {
                    self.square1.tick_length();
                    self.square2.tick_length();
                    self.wave.tick_length();
                    self.noise.tick_length();
                }
                2 | 6 => {
                    self.square1.tick_length();
                    self.square2.tick_length();
                    self.wave.tick_length();
                    self.noise.tick_length();
                    self.square1.tick_sweep();
                }
                7 => {
                    self.square1.envelope.tick();
                    self.square2.envelope.tick();
                    self.noise.envelope.tick();
                }
                _ => {}
            }
            self.sequencer_step = (self.sequencer_step + 1) & 7;
        }

        self.sample_timer += cycles;
        while self.sample_timer >= CYCLES_PER_SAMPLE {
            self.sample_timer -= CYCLES_PER_SAMPLE;
            let (left, right) = self.mix();
            self.buffer.push(left);
            self.buffer.push(right);
        }
    }

    fn mix(&self) -> (i16, i16) {
        let psg_volume = match self.control_h & 3 {
            0 => 1,
            1 => 2,
            _ => 4,
        };
        let mut left = 0i32;
        let mut right = 0i32;
        let channels = [
            self.square1.sample(),
            self.square2.sample(),
            self.wave.sample(),
            self.noise.sample(),
        ];
        for (index, sample) in channels.iter().enumerate() {
            let sample = *sample as i32 * 96;
            if self.control_l & (1 << (8 + index)) != 0 {
                left += sample;
            }
            if self.control_l & (1 << (12 + index)) != 0 {
                right += sample;
            }
        }
        let left_volume = (self.control_l & 7) as i32 + 1;
        let right_volume = ((self.control_l >> 4) & 7) as i32 + 1;
        left = left * left_volume * psg_volume / 8 / 4;
        right = right * right_volume * psg_volume / 8 / 4;

        for channel in 0..2 {
            let volume = if self.control_h & (1 << (2 + channel)) != 0 {
                4
            } else {
                2
            };
            let sample = self.fifo[channel].current as i32 * volume * 32;
            if self.control_h & (1 << (8 + channel * 4)) != 0 {
                right += sample;
            }
            if self.control_h & (1 << (9 + channel * 4)) != 0 {
                left += sample;
            }
        }
        (
            left.clamp(-32768, 32767) as i16,
            right.clamp(-32768, 32767) as i16,
        )
    }

    pub fn read_register(&self, address: usize) -> u16 {
        match address {
            0x60 => (self.square1.sweep_period as u16) << 4
                | (self.square1.sweep_decrease as u16) << 3
                | self.square1.sweep_shift as u16,
            0x62 => (self.square1.duty as u16) << 6 | (self.square1.envelope.initial as u16) << 12,
            0x68 => (self.square2.duty as u16) << 6 | (self.square2.envelope.initial as u16) << 12,
            0x70 => (self.wave.enabled as u16) << 7 | (self.wave.two_banks as u16) << 5,
            0x80 => self.control_l,
            0x82 => self.control_h,
            0x84 => {
                self.control_x & 0x80
                    | (self.square1.enabled as u16)
                    | (self.square2.enabled as u16) << 1
                    | (self.wave.playing as u16) << 2
                    | (self.noise.enabled as u16) << 3
            }
            0x88 => self.bias,
            0x90..=0x9f => {
                let bank = self.wave.bank ^ 1;
                let index = address - 0x90;
                u16::from_le_bytes([
                    self.wave.banks[bank][index],
                    self.wave.banks[bank][index + 1],
                ])
            }
            _ => 0,
        }
    }

    pub fn write_register(&mut self, address: usize, value: u16) {
        match address {
            0x60 => {
                self.square1.sweep_period = ((value >> 4) & 7) as u8;
                self.square1.sweep_decrease = value & 8 != 0;
                self.square1.sweep_shift = (value & 7) as u8;
            }
            0x62 => {
                self.square1.length = 64 - (value & 0x3f);
                self.square1.duty = ((value >> 6) & 3) as usize;
                self.square1.envelope.period = ((value >> 8) & 7) as u8;
                self.square1.envelope.increase = value & 0x800 != 0;
                self.square1.envelope.initial = ((value >> 12) & 0xf) as u8;
                self.square1.envelope.volume = self.square1.envelope.initial;
                if value & 0xf800 == 0 {
                    self.square1.enabled = false;
                }
            }
            0x64 => {
                self.square1.frequency = value & 0x7ff;
                self.square1.length_enabled = value & 0x4000 != 0;
                if value & 0x8000 != 0 {
                    self.square1.trigger();
                }
            }
            0x68 => {
                self.square2.length = 64 - (value & 0x3f);
                self.square2.duty = ((value >> 6) & 3) as usize;
                self.square2.envelope.period = ((value >> 8) & 7) as u8;
                self.square2.envelope.increase = value & 0x800 != 0;
                self.square2.envelope.initial = ((value >> 12) & 0xf) as u8;
                self.square2.envelope.volume = self.square2.envelope.initial;
                if value & 0xf800 == 0 {
                    self.square2.enabled = false;
                }
            }
            0x6c => {
                self.square2.frequency = value & 0x7ff;
                self.square2.length_enabled = value & 0x4000 != 0;
                if value & 0x8000 != 0 {
                    self.square2.trigger();
                }
            }
            0x70 => {
                self.wave.two_banks = value & 0x20 != 0;
                self.wave.bank = ((value >> 6) & 1) as usize;
                self.wave.enabled = value & 0x80 != 0;
                if !self.wave.enabled {
                    self.wave.playing = false;
                }
            }
            0x72 => {
                self.wave.length = 256 - (value & 0xff);
                self.wave.volume = ((value >> 13) & 3) as u8;
                self.wave.force_75 = value & 0x8000 != 0;
            }
            0x74 => {
                self.wave.frequency = value & 0x7ff;
                self.wave.length_enabled = value & 0x4000 != 0;
                if value & 0x8000 != 0 {
                    self.wave.trigger();
                }
            }
            0x78 => {
                self.noise.length = 64 - (value & 0x3f);
                self.noise.envelope.period = ((value >> 8) & 7) as u8;
                self.noise.envelope.increase = value & 0x800 != 0;
                self.noise.envelope.initial = ((value >> 12) & 0xf) as u8;
                self.noise.envelope.volume = self.noise.envelope.initial;
                if value & 0xf800 == 0 {
                    self.noise.enabled = false;
                }
            }
            0x7c => {
                self.noise.divisor = (value & 7) as u8;
                self.noise.width7 = value & 8 != 0;
                self.noise.shift = ((value >> 4) & 0xf) as u8;
                self.noise.length_enabled = value & 0x4000 != 0;
                if value & 0x8000 != 0 {
                    self.noise.trigger();
                }
            }
            0x80 => self.control_l = value,
            0x82 => {
                self.control_h = value & 0x770f;
                if value & 0x0800 != 0 {
                    self.fifo[0].clear();
                }
                if value & 0x8000 != 0 {
                    self.fifo[1].clear();
                }
            }
            0x84 => {
                self.control_x = value & 0x80;
                if value & 0x80 == 0 {
                    self.square1.enabled = false;
                    self.square2.enabled = false;
                    self.wave.playing = false;
                    self.noise.enabled = false;
                }
            }
            0x88 => self.bias = value,
            0x90..=0x9f => {
                let bank = self.wave.bank ^ 1;
                let index = address - 0x90;
                let bytes = value.to_le_bytes();
                self.wave.banks[bank][index] = bytes[0];
                self.wave.banks[bank][index + 1] = bytes[1];
            }
            _ => {}
        }
    }
}
