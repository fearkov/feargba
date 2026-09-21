//! The four DMA channels.

#[derive(Clone, Copy, Default)]
pub struct DmaChannel {
    pub source: u32,
    pub destination: u32,
    pub count: u16,
    pub control: u16,
    /// Latched address and length, reloaded when the channel repeats.
    pub internal_source: u32,
    pub internal_destination: u32,
    pub internal_count: u32,
    pub active: bool,
}

/// What made a channel start; `control` bits 12-13 select one of these.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DmaTiming {
    Immediate,
    VBlank,
    HBlank,
    Special,
}

impl DmaChannel {
    pub fn timing(&self) -> DmaTiming {
        match (self.control >> 12) & 3 {
            0 => DmaTiming::Immediate,
            1 => DmaTiming::VBlank,
            2 => DmaTiming::HBlank,
            _ => DmaTiming::Special,
        }
    }
    pub fn enabled(&self) -> bool {
        self.control & 0x8000 != 0
    }
    pub fn repeats(&self) -> bool {
        self.control & 0x0200 != 0
    }
    pub fn word(&self) -> bool {
        self.control & 0x0400 != 0
    }
    pub fn irq(&self) -> bool {
        self.control & 0x4000 != 0
    }
    pub fn destination_control(&self) -> u16 {
        (self.control >> 5) & 3
    }
    pub fn source_control(&self) -> u16 {
        (self.control >> 7) & 3
    }
}
