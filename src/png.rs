//! Minimal PNG writer, used for headless screenshots.

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        let mut value = index as u32;
        for _ in 0..8 {
            value = if value & 1 != 0 {
                0xedb8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
        }
        *entry = value;
    }
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc = table[((crc ^ *byte as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xffff_ffff
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for byte in data {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    output.extend_from_slice(&body);
    output.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// Encodes XRGB8888 pixels as a PNG with stored (uncompressed) deflate blocks.
pub fn encode(pixels: &[u32], width: usize, height: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(height * (width * 3 + 1));
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            let pixel = pixels[y * width + x];
            raw.push((pixel >> 16) as u8);
            raw.push((pixel >> 8) as u8);
            raw.push(pixel as u8);
        }
    }

    let mut deflate = vec![0x78, 0x01];
    for (index, block) in raw.chunks(65535).enumerate() {
        let last = (index + 1) * 65535 >= raw.len();
        deflate.push(last as u8);
        deflate.extend_from_slice(&(block.len() as u16).to_le_bytes());
        deflate.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        deflate.extend_from_slice(block);
    }
    deflate.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut header = Vec::new();
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &deflate);
    chunk(&mut png, b"IEND", &[]);
    png
}
