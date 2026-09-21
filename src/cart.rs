//! Cartridge: ROM, backup memory and the GPIO real time clock.

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SaveKind {
    None,
    Sram,
    Flash64,
    Flash128,
    Eeprom512,
    Eeprom8k,
}

impl SaveKind {
    pub fn size(self) -> usize {
        match self {
            SaveKind::None => 0,
            SaveKind::Sram => 0x8000,
            SaveKind::Flash64 => 0x10000,
            SaveKind::Flash128 => 0x20000,
            SaveKind::Eeprom512 => 0x200,
            SaveKind::Eeprom8k => 0x2000,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FlashState {
    Ready,
    Command1,
    Command2,
    WriteByte,
    BankSelect,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EepromState {
    Ready,
    Address,
    ReadDummy,
    Read,
    Write,
}

pub struct Cart {
    pub rom: Vec<u8>,
    /// Set from FEAR_GBA_TRACE_FLASH to log the backup command stream.
    pub trace_flash: bool,
    pub save: Vec<u8>,
    pub kind: SaveKind,
    pub save_dirty: bool,
    flash_state: FlashState,
    flash_id_mode: bool,
    flash_erase_armed: bool,
    flash_bank: usize,
    eeprom_state: EepromState,
    eeprom_address: usize,
    eeprom_buffer: u64,
    eeprom_bits: u32,
    pub rtc: Rtc,
    pub has_rtc: bool,
}

impl Cart {
    pub fn new(rom: Vec<u8>) -> Self {
        let kind = detect_save(&rom);
        let has_rtc = find(&rom, b"SIIRTC_V").is_some();
        let mut cart = Self {
            rom,
            trace_flash: std::env::var_os("FEAR_GBA_TRACE_FLASH").is_some(),
            save: vec![0xff; kind.size().max(1)],
            kind,
            save_dirty: false,
            flash_state: FlashState::Ready,
            flash_id_mode: false,
            flash_erase_armed: false,
            flash_bank: 0,
            eeprom_state: EepromState::Ready,
            eeprom_address: 0,
            eeprom_buffer: 0,
            eeprom_bits: 0,
            rtc: Rtc::new(),
            has_rtc,
        };
        if matches!(cart.kind, SaveKind::Eeprom512 | SaveKind::Eeprom8k) {
            cart.save.fill(0xff);
        }
        cart
    }

    pub fn load_save(&mut self, data: &[u8]) {
        let length = data.len().min(self.save.len());
        self.save[..length].copy_from_slice(&data[..length]);
    }

    /// True when a read of this ROM address is really an EEPROM access.
    pub fn is_eeprom_address(&self, address: u32) -> bool {
        if !matches!(self.kind, SaveKind::Eeprom512 | SaveKind::Eeprom8k) {
            return false;
        }
        let offset = address & 0x01ff_ffff;
        if self.rom.len() > 0x0100_0000 {
            (0x0d00_0000..=0x0dff_ffff).contains(&address) && offset >= 0x01ff_ff00
        } else {
            (0x0d00_0000..=0x0dff_ffff).contains(&address)
        }
    }

    pub fn read_rom8(&self, address: u32) -> u8 {
        let offset = (address & 0x01ff_ffff) as usize;
        if self.has_rtc && (0xc4..0xca).contains(&offset) {
            let half = self.rtc.read(offset & !1);
            return if offset & 1 != 0 {
                (half >> 8) as u8
            } else {
                half as u8
            };
        }
        self.rom.get(offset).copied().unwrap_or_else(|| {
            // Out of bounds reads see the low half of the address.
            let index = (address / 2) as u16;
            if address & 1 != 0 {
                (index >> 8) as u8
            } else {
                index as u8
            }
        })
    }

    pub fn read_rom16(&self, address: u32) -> u16 {
        let offset = (address & 0x01ff_fffe) as usize;
        if self.has_rtc && (0xc4..0xca).contains(&offset) {
            return self.rtc.read(offset);
        }
        if offset + 1 < self.rom.len() {
            u16::from_le_bytes([self.rom[offset], self.rom[offset + 1]])
        } else {
            (address / 2) as u16
        }
    }

    pub fn write_rom16(&mut self, address: u32, value: u16) {
        let offset = (address & 0x01ff_fffe) as usize;
        if self.has_rtc && (0xc4..0xca).contains(&offset) {
            self.rtc.write(offset, value);
        }
    }

    pub fn read_save(&mut self, address: u32) -> u8 {
        match self.kind {
            SaveKind::Sram => self.save[(address as usize) & 0x7fff],
            SaveKind::Flash64 | SaveKind::Flash128 => {
                let offset = (address as usize) & 0xffff;
                if self.flash_id_mode && offset < 2 {
                    let (manufacturer, device) = if self.kind == SaveKind::Flash128 {
                        (0x62, 0x13)
                    } else {
                        (0x32, 0x1b)
                    };
                    return if offset == 0 { manufacturer } else { device };
                }
                self.save[self.flash_bank * 0x10000 + offset]
            }
            _ => 0xff,
        }
    }

    pub fn write_save(&mut self, address: u32, value: u8) {
        match self.kind {
            SaveKind::Sram => {
                self.save[(address as usize) & 0x7fff] = value;
                self.save_dirty = true;
            }
            SaveKind::Flash64 | SaveKind::Flash128 => self.write_flash(address, value),
            _ => {}
        }
    }

    fn write_flash(&mut self, address: u32, value: u8) {
        let offset = (address as usize) & 0xffff;
        match self.flash_state {
            FlashState::WriteByte => {
                if self.trace_flash {
                    eprintln!("flash program {offset:04x}={value:02x} bank {}", self.flash_bank);
                }
                self.save[self.flash_bank * 0x10000 + offset] &= value;
                self.save_dirty = true;
                self.flash_state = FlashState::Ready;
                return;
            }
            FlashState::BankSelect => {
                if offset == 0 && self.kind == SaveKind::Flash128 {
                    self.flash_bank = (value & 1) as usize;
                }
                self.flash_state = FlashState::Ready;
                return;
            }
            _ => {}
        }
        match (self.flash_state, offset, value) {
            (FlashState::Ready, 0x5555, 0xaa) => self.flash_state = FlashState::Command1,
            (FlashState::Command1, 0x2aaa, 0x55) => self.flash_state = FlashState::Command2,
            (FlashState::Command2, 0x5555, command) => {
                if self.trace_flash {
                    eprintln!("flash command {command:02x}");
                }
                self.flash_state = FlashState::Ready;
                match command {
                    0x90 => self.flash_id_mode = true,
                    0xf0 => self.flash_id_mode = false,
                    0x80 => self.flash_erase_armed = true,
                    0x10 if self.flash_erase_armed => {
                        self.save.fill(0xff);
                        self.save_dirty = true;
                        self.flash_erase_armed = false;
                    }
                    0xa0 => self.flash_state = FlashState::WriteByte,
                    0xb0 => self.flash_state = FlashState::BankSelect,
                    _ => self.flash_erase_armed = false,
                }
            }
            (FlashState::Command2, sector, 0x30) if self.flash_erase_armed => {
                if self.trace_flash {
                    eprintln!("flash erase sector {sector:04x} bank {}", self.flash_bank);
                }
                let base = self.flash_bank * 0x10000 + (sector & 0xf000);
                let end = (base + 0x1000).min(self.save.len());
                self.save[base..end].fill(0xff);
                self.save_dirty = true;
                self.flash_erase_armed = false;
                self.flash_state = FlashState::Ready;
            }
            (state, offset, value) => {
                if self.trace_flash && !matches!(state, FlashState::Ready) {
                    eprintln!("flash aborted at {offset:04x}={value:02x}");
                }
                self.flash_state = FlashState::Ready;
            }
        }
    }

    fn eeprom_address_bits(&self) -> u32 {
        if self.kind == SaveKind::Eeprom8k {
            14
        } else {
            6
        }
    }

    /// EEPROM is bit serial; the game drives it with 16 bit DMA bursts.
    pub fn read_eeprom(&mut self) -> u16 {
        match self.eeprom_state {
            EepromState::ReadDummy => {
                self.eeprom_bits += 1;
                if self.eeprom_bits == 4 {
                    self.eeprom_bits = 0;
                    self.eeprom_state = EepromState::Read;
                }
                0
            }
            EepromState::Read => {
                let byte = self.eeprom_address * 8 + (self.eeprom_bits / 8) as usize;
                let bit = 7 - (self.eeprom_bits % 8);
                self.eeprom_bits += 1;
                if self.eeprom_bits == 64 {
                    self.eeprom_bits = 0;
                    self.eeprom_state = EepromState::Ready;
                }
                let value = self.save.get(byte).copied().unwrap_or(0xff);
                ((value >> bit) & 1) as u16
            }
            _ => 1,
        }
    }

    pub fn write_eeprom(&mut self, value: u16) {
        let bit = (value & 1) as u64;
        match self.eeprom_state {
            EepromState::Ready => {
                self.eeprom_buffer = (self.eeprom_buffer << 1) | bit;
                self.eeprom_bits += 1;
                if self.eeprom_bits == 2 {
                    let request = self.eeprom_buffer & 3;
                    self.eeprom_bits = 0;
                    self.eeprom_buffer = 0;
                    match request {
                        3 => self.eeprom_state = EepromState::Address,
                        2 => self.eeprom_state = EepromState::Write,
                        _ => {}
                    }
                }
            }
            EepromState::Address => {
                self.eeprom_buffer = (self.eeprom_buffer << 1) | bit;
                self.eeprom_bits += 1;
                if self.eeprom_bits == self.eeprom_address_bits() {
                    self.eeprom_address = (self.eeprom_buffer as usize) & 0x3ff;
                    self.eeprom_bits = 0;
                    self.eeprom_buffer = 0;
                    self.eeprom_state = EepromState::ReadDummy;
                }
            }
            EepromState::Write => {
                let address_bits = self.eeprom_address_bits();
                if self.eeprom_bits < address_bits {
                    self.eeprom_buffer = (self.eeprom_buffer << 1) | bit;
                    self.eeprom_bits += 1;
                    if self.eeprom_bits == address_bits {
                        self.eeprom_address = (self.eeprom_buffer as usize) & 0x3ff;
                        self.eeprom_buffer = 0;
                    }
                } else {
                    self.eeprom_buffer = (self.eeprom_buffer << 1) | bit;
                    self.eeprom_bits += 1;
                    if self.eeprom_bits == address_bits + 64 {
                        let base = self.eeprom_address * 8;
                        let bytes = self.eeprom_buffer.to_be_bytes();
                        for (index, byte) in bytes.into_iter().enumerate() {
                            if base + index < self.save.len() {
                                self.save[base + index] = byte;
                            }
                        }
                        self.save_dirty = true;
                        self.eeprom_bits = 0;
                        self.eeprom_buffer = 0;
                        self.eeprom_state = EepromState::Ready;
                    }
                }
            }
            _ => {
                self.eeprom_bits = 0;
                self.eeprom_buffer = 0;
                self.eeprom_state = EepromState::Ready;
            }
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn detect_save(rom: &[u8]) -> SaveKind {
    if find(rom, b"FLASH1M_V").is_some() {
        SaveKind::Flash128
    } else if find(rom, b"FLASH512_V").is_some() || find(rom, b"FLASH_V").is_some() {
        SaveKind::Flash64
    } else if find(rom, b"EEPROM_V").is_some() {
        SaveKind::Eeprom8k
    } else {
        // SRAM_V, SRAM_F_V and anything unrecognised: plain SRAM is the safe
        // default, since a game that never touches backup memory costs nothing.
        SaveKind::Sram
    }
}

const PIN_SCK: u16 = 1;
const PIN_SIO: u16 = 2;
const PIN_CS: u16 = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RtcState {
    Idle,
    Command,
    Transfer,
}

/// Seiko S3511 clock on the cartridge GPIO pins.
pub struct Rtc {
    data: u16,
    direction: u16,
    readable: bool,
    state: RtcState,
    command: u8,
    bits: u32,
    buffer: u8,
    byte_index: usize,
    transfer: Vec<u8>,
    reading: bool,
    control: u8,
    last_sck: bool,
    last_cs: bool,
}

impl Rtc {
    fn new() -> Self {
        Self {
            data: 0,
            direction: 0,
            readable: false,
            state: RtcState::Idle,
            command: 0,
            bits: 0,
            buffer: 0,
            byte_index: 0,
            transfer: Vec::new(),
            reading: false,
            control: 0x40,
            last_sck: false,
            last_cs: false,
        }
    }

    fn read(&self, offset: usize) -> u16 {
        if !self.readable {
            return 0;
        }
        match offset {
            0xc4 => self.data & !self.direction & 7,
            0xc6 => self.direction,
            0xc8 => self.readable as u16,
            _ => 0,
        }
    }

    fn write(&mut self, offset: usize, value: u16) {
        match offset {
            0xc4 => self.write_pins(value & 7),
            0xc6 => self.direction = value & 7,
            0xc8 => self.readable = value & 1 != 0,
            _ => {}
        }
    }

    fn write_pins(&mut self, value: u16) {
        // Only pins configured as outputs are driven by the game.
        let driven = (value & self.direction) | (self.data & !self.direction);
        let sck = driven & PIN_SCK != 0;
        let sio = driven & PIN_SIO != 0;
        let cs = driven & PIN_CS != 0;
        let last_sck = self.last_sck;
        let last_cs = self.last_cs;
        self.last_sck = sck;
        self.last_cs = cs;
        self.data = (self.data & !self.direction) | (value & self.direction);

        if !cs {
            if last_cs {
                self.state = RtcState::Idle;
            }
            return;
        }
        if !last_cs {
            self.state = RtcState::Command;
            self.bits = 0;
            self.buffer = 0;
            self.byte_index = 0;
            self.transfer.clear();
            return;
        }

        match self.state {
            RtcState::Command => {
                // The game clocks the command byte in on rising edges.
                if !last_sck && sck {
                    self.buffer = (self.buffer << 1) | sio as u8;
                    self.bits += 1;
                    if self.bits == 8 {
                        let mut command = self.buffer;
                        if command & 0xf0 != 0x60 {
                            command = command.reverse_bits();
                        }
                        self.command = command;
                        self.reading = command & 1 != 0;
                        self.bits = 0;
                        self.buffer = 0;
                        self.byte_index = 0;
                        self.begin_transfer();
                        self.state = RtcState::Transfer;
                    }
                }
            }
            RtcState::Transfer => {
                if self.reading {
                    // Present the next bit while the clock is low.
                    if last_sck && !sck {
                        let byte = self
                            .transfer
                            .get(self.byte_index)
                            .copied()
                            .unwrap_or(0);
                        let bit = (byte >> (self.bits & 7)) & 1;
                        self.data = (self.data & !PIN_SIO) | (bit as u16) << 1;
                        self.bits += 1;
                        if self.bits & 7 == 0 {
                            self.byte_index += 1;
                            if self.byte_index >= self.transfer.len() {
                                self.state = RtcState::Idle;
                            }
                        }
                    }
                } else if !last_sck && sck {
                    self.buffer |= (sio as u8) << (self.bits & 7);
                    self.bits += 1;
                    if self.bits & 7 == 0 {
                        self.finish_byte();
                        self.buffer = 0;
                        self.byte_index += 1;
                        if self.byte_index >= self.transfer.len() {
                            self.state = RtcState::Idle;
                        }
                    }
                }
            }
            RtcState::Idle => {}
        }
    }

    fn begin_transfer(&mut self) {
        let register = (self.command >> 1) & 7;
        self.transfer = match register {
            0 => {
                self.control = 0;
                Vec::new()
            }
            1 => vec![self.control],
            2 => self.date_time(),
            3 => self.date_time()[4..].to_vec(),
            _ => vec![0],
        };
        if self.transfer.is_empty() {
            self.state = RtcState::Idle;
        }
    }

    fn finish_byte(&mut self) {
        let register = (self.command >> 1) & 7;
        if register == 1 {
            self.control = self.buffer & 0x6a;
        }
    }

    fn date_time(&self) -> Vec<u8> {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0) as i64;
        let (year, month, day, weekday, hour, minute, second) = civil_from_unix(seconds);
        let hour24 = self.control & 0x40 != 0;
        let mut hour_value = hour;
        let mut pm = 0;
        if !hour24 {
            pm = if hour >= 12 { 0x80 } else { 0 };
            hour_value = hour % 12;
        }
        vec![
            bcd((year % 100) as u32),
            bcd(month),
            bcd(day),
            weekday as u8,
            bcd(hour_value) | pm,
            bcd(minute),
            bcd(second),
        ]
    }
}

fn bcd(value: u32) -> u8 {
    (((value / 10) << 4) | (value % 10)) as u8
}

/// Converts a Unix timestamp into local-ish civil time fields.
fn civil_from_unix(seconds: i64) -> (i64, u32, u32, u32, u32, u32, u32) {
    let days = seconds.div_euclid(86400);
    let rem = seconds.rem_euclid(86400) as u32;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    let weekday = ((days + 4).rem_euclid(7)) as u32;

    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d, weekday, hour, minute, second)
}

use crate::state::{Reader, Snapshot, StateError, Writer};

impl FlashState {
    fn code(self) -> u8 {
        match self {
            FlashState::Ready => 0,
            FlashState::Command1 => 1,
            FlashState::Command2 => 2,
            FlashState::WriteByte => 3,
            FlashState::BankSelect => 4,
        }
    }
    fn from_code(code: u8) -> Self {
        match code {
            1 => FlashState::Command1,
            2 => FlashState::Command2,
            3 => FlashState::WriteByte,
            4 => FlashState::BankSelect,
            _ => FlashState::Ready,
        }
    }
}

impl EepromState {
    fn code(self) -> u8 {
        match self {
            EepromState::Ready => 0,
            EepromState::Address => 1,
            EepromState::ReadDummy => 2,
            EepromState::Read => 3,
            EepromState::Write => 4,
        }
    }
    fn from_code(code: u8) -> Self {
        match code {
            1 => EepromState::Address,
            2 => EepromState::ReadDummy,
            3 => EepromState::Read,
            4 => EepromState::Write,
            _ => EepromState::Ready,
        }
    }
}

impl SaveKind {
    fn code(self) -> u8 {
        match self {
            SaveKind::None => 0,
            SaveKind::Sram => 1,
            SaveKind::Flash64 => 2,
            SaveKind::Flash128 => 3,
            SaveKind::Eeprom512 => 4,
            SaveKind::Eeprom8k => 5,
        }
    }
    fn from_code(code: u8) -> Self {
        match code {
            1 => SaveKind::Sram,
            2 => SaveKind::Flash64,
            3 => SaveKind::Flash128,
            4 => SaveKind::Eeprom512,
            5 => SaveKind::Eeprom8k,
            _ => SaveKind::None,
        }
    }
}

impl Snapshot for Rtc {
    fn save(&self, writer: &mut Writer) {
        writer.u16(self.data);
        writer.u16(self.direction);
        writer.bool(self.readable);
        writer.u8(match self.state {
            RtcState::Idle => 0,
            RtcState::Command => 1,
            RtcState::Transfer => 2,
        });
        writer.u8(self.command);
        writer.u32(self.bits);
        writer.u8(self.buffer);
        writer.usize(self.byte_index);
        writer.bytes(&self.transfer);
        writer.bool(self.reading);
        writer.u8(self.control);
        writer.bool(self.last_sck);
        writer.bool(self.last_cs);
    }
    fn load(&mut self, reader: &mut Reader) -> Result<(), StateError> {
        self.data = reader.u16()?;
        self.direction = reader.u16()?;
        self.readable = reader.bool()?;
        self.state = match reader.u8()? {
            1 => RtcState::Command,
            2 => RtcState::Transfer,
            _ => RtcState::Idle,
        };
        self.command = reader.u8()?;
        self.bits = reader.u32()?;
        self.buffer = reader.u8()?;
        self.byte_index = reader.usize()?;
        reader.into_vec(&mut self.transfer)?;
        self.reading = reader.bool()?;
        self.control = reader.u8()?;
        self.last_sck = reader.bool()?;
        self.last_cs = reader.bool()?;
        Ok(())
    }
}

impl Snapshot for Cart {
    fn save(&self, writer: &mut Writer) {
        writer.u8(self.kind.code());
        writer.bytes(&self.save);
        writer.u8(self.flash_state.code());
        writer.bool(self.flash_id_mode);
        writer.bool(self.flash_erase_armed);
        writer.usize(self.flash_bank);
        writer.u8(self.eeprom_state.code());
        writer.usize(self.eeprom_address);
        writer.u64(self.eeprom_buffer);
        writer.u32(self.eeprom_bits);
        writer.bool(self.has_rtc);
        self.rtc.save(writer);
    }
    fn load(&mut self, reader: &mut Reader) -> Result<(), StateError> {
        self.kind = SaveKind::from_code(reader.u8()?);
        reader.into_vec(&mut self.save)?;
        self.flash_state = FlashState::from_code(reader.u8()?);
        self.flash_id_mode = reader.bool()?;
        self.flash_erase_armed = reader.bool()?;
        self.flash_bank = reader.usize()? & 1;
        self.eeprom_state = EepromState::from_code(reader.u8()?);
        self.eeprom_address = reader.usize()?;
        self.eeprom_buffer = reader.u64()?;
        self.eeprom_bits = reader.u32()?;
        self.has_rtc = reader.bool()?;
        self.rtc.load(reader)?;
        self.save_dirty = true;
        Ok(())
    }
}
