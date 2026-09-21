//! Save states: a small binary format plus the trait each component fills in.

use std::fmt;

const MAGIC: &[u8; 4] = b"FGBA";
const VERSION: u32 = 1;

#[derive(Debug)]
pub enum StateError {
    NotAState,
    WrongVersion(u32),
    WrongGame,
    Truncated,
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::NotAState => write!(formatter, "not a save state"),
            StateError::WrongVersion(found) => {
                write!(formatter, "save state version {found}, expected {VERSION}")
            }
            StateError::WrongGame => write!(formatter, "save state is from another game"),
            StateError::Truncated => write!(formatter, "save state is truncated"),
        }
    }
}

impl std::error::Error for StateError {}

#[derive(Default)]
pub struct Writer {
    pub data: Vec<u8>,
}

impl Writer {
    pub fn u8(&mut self, value: u8) {
        self.data.push(value);
    }
    pub fn bool(&mut self, value: bool) {
        self.data.push(value as u8);
    }
    pub fn u16(&mut self, value: u16) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }
    pub fn u32(&mut self, value: u32) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }
    pub fn u64(&mut self, value: u64) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }
    pub fn i32(&mut self, value: i32) {
        self.u32(value as u32);
    }
    pub fn i16(&mut self, value: i16) {
        self.u16(value as u16);
    }
    pub fn usize(&mut self, value: usize) {
        self.u32(value as u32);
    }
    pub fn option_u16(&mut self, value: Option<u16>) {
        self.bool(value.is_some());
        self.u16(value.unwrap_or(0));
    }
    pub fn bytes(&mut self, value: &[u8]) {
        self.usize(value.len());
        self.data.extend_from_slice(value);
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], StateError> {
        let end = self.position + count;
        if end > self.data.len() {
            return Err(StateError::Truncated);
        }
        let slice = &self.data[self.position..end];
        self.position = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8, StateError> {
        Ok(self.take(1)?[0])
    }
    pub fn bool(&mut self) -> Result<bool, StateError> {
        Ok(self.u8()? != 0)
    }
    pub fn u16(&mut self) -> Result<u16, StateError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
    pub fn u32(&mut self) -> Result<u32, StateError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
    pub fn u64(&mut self) -> Result<u64, StateError> {
        let bytes = self.take(8)?;
        let mut value = [0u8; 8];
        value.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(value))
    }
    pub fn i32(&mut self) -> Result<i32, StateError> {
        Ok(self.u32()? as i32)
    }
    pub fn i16(&mut self) -> Result<i16, StateError> {
        Ok(self.u16()? as i16)
    }
    pub fn usize(&mut self) -> Result<usize, StateError> {
        Ok(self.u32()? as usize)
    }
    pub fn option_u16(&mut self) -> Result<Option<u16>, StateError> {
        let present = self.bool()?;
        let value = self.u16()?;
        Ok(present.then_some(value))
    }
    /// Reads a length prefixed block into an existing buffer, keeping its size.
    pub fn into_bytes(&mut self, target: &mut [u8]) -> Result<(), StateError> {
        let length = self.usize()?;
        let data = self.take(length)?;
        let shared = length.min(target.len());
        target[..shared].copy_from_slice(&data[..shared]);
        Ok(())
    }
    /// Reads a length prefixed block into a vector, resizing it to match.
    pub fn into_vec(&mut self, target: &mut Vec<u8>) -> Result<(), StateError> {
        let length = self.usize()?;
        let data = self.take(length)?;
        target.clear();
        target.extend_from_slice(data);
        Ok(())
    }
}

/// Every component that forms part of a save state.
pub trait Snapshot {
    fn save(&self, writer: &mut Writer);
    fn load(&mut self, reader: &mut Reader) -> Result<(), StateError>;
}

pub fn write_header(writer: &mut Writer, game: u32) {
    writer.data.extend_from_slice(MAGIC);
    writer.u32(VERSION);
    writer.u32(game);
}

pub fn read_header(reader: &mut Reader, game: u32) -> Result<(), StateError> {
    let magic = reader.take(4)?;
    if magic != MAGIC {
        return Err(StateError::NotAState);
    }
    let version = reader.u32()?;
    if version != VERSION {
        return Err(StateError::WrongVersion(version));
    }
    if reader.u32()? != game {
        return Err(StateError::WrongGame);
    }
    Ok(())
}
