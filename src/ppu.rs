//! Picture processing unit: scanline renderer for every background mode,
//! sprites, windows and the colour special effects.

pub const WIDTH: usize = 240;
pub const HEIGHT: usize = 160;

const CYCLES_PER_LINE: u32 = 1232;
const HBLANK_CYCLES: u32 = 960;
const TOTAL_LINES: u16 = 228;

/// Marks a scanline slot no layer has written yet.
const TRANSPARENT: u16 = 0x8000;

/// Hblank on a visible line, which is when background DMA runs.
pub const EVENT_HBLANK: u8 = 1;
pub const EVENT_VBLANK: u8 = 2;
pub const EVENT_FRAME: u8 = 4;
/// Hblank on any line at all, including the ones below the screen.
pub const EVENT_HBLANK_ANY: u8 = 8;

#[derive(Clone, Copy, Default)]
struct ObjPixel {
    color: u16,
    priority: u8,
    semi_transparent: bool,
}

pub struct Ppu {
    pub vram: Vec<u8>,
    pub palette: Vec<u8>,
    pub oam: Vec<u8>,

    pub dispcnt: u16,
    pub green_swap: u16,
    pub dispstat: u16,
    pub vcount: u16,
    pub bgcnt: [u16; 4],
    pub bghofs: [u16; 4],
    pub bgvofs: [u16; 4],
    /// Affine parameters for BG2 and BG3.
    pub bgpa: [i16; 2],
    pub bgpb: [i16; 2],
    pub bgpc: [i16; 2],
    pub bgpd: [i16; 2],
    pub bgx: [i32; 2],
    pub bgy: [i32; 2],
    bgx_internal: [i32; 2],
    bgy_internal: [i32; 2],
    pub winh: [u16; 2],
    pub winv: [u16; 2],
    pub winin: u16,
    pub winout: u16,
    pub mosaic: u16,
    pub bldcnt: u16,
    pub bldalpha: u16,
    pub bldy: u16,

    cycles: u32,
    /// The line the most recent hblank belonged to; `vcount` has moved on by
    /// the time the bus acts on the event.
    pub hblank_line: u16,
    pub framebuffer: Vec<u32>,

    bg_line: [[u16; WIDTH]; 4],
    obj_line: [ObjPixel; WIDTH],
    obj_window: [bool; WIDTH],
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

impl Ppu {
    pub fn new() -> Self {
        Self {
            vram: vec![0; 0x18000],
            palette: vec![0; 0x400],
            oam: vec![0; 0x400],
            dispcnt: 0x0080,
            green_swap: 0,
            dispstat: 0,
            vcount: 0,
            bgcnt: [0; 4],
            bghofs: [0; 4],
            bgvofs: [0; 4],
            bgpa: [0x100; 2],
            bgpb: [0; 2],
            bgpc: [0; 2],
            bgpd: [0x100; 2],
            bgx: [0; 2],
            bgy: [0; 2],
            bgx_internal: [0; 2],
            bgy_internal: [0; 2],
            winh: [0; 2],
            winv: [0; 2],
            winin: 0,
            winout: 0,
            mosaic: 0,
            bldcnt: 0,
            bldalpha: 0,
            bldy: 0,
            cycles: 0,
            hblank_line: 0,
            framebuffer: vec![0; WIDTH * HEIGHT],
            bg_line: [[TRANSPARENT; WIDTH]; 4],
            obj_line: [ObjPixel::default(); WIDTH],
            obj_window: [false; WIDTH],
        }
    }

    pub fn in_hblank(&self) -> bool {
        self.cycles >= HBLANK_CYCLES
    }

    /// Advances the dot clock. Returns (IRQ mask, event flags).
    pub fn tick(&mut self, cycles: u32) -> (u16, u8) {
        let mut irq = 0u16;
        let mut events = 0u8;
        self.cycles += cycles;

        if self.dispstat & 2 == 0 && self.cycles >= HBLANK_CYCLES && self.vcount < TOTAL_LINES {
            self.dispstat |= 2;
            self.hblank_line = self.vcount;
            events |= EVENT_HBLANK_ANY;
            if self.vcount < HEIGHT as u16 {
                self.render_scanline();
                events |= EVENT_HBLANK;
            }
            if self.dispstat & 1 << 4 != 0 {
                irq |= 1 << 1;
            }
        }

        while self.cycles >= CYCLES_PER_LINE {
            self.cycles -= CYCLES_PER_LINE;
            self.dispstat &= !2;
            if self.vcount < HEIGHT as u16 {
                // Affine backgrounds walk their reference point every line.
                for index in 0..2 {
                    self.bgx_internal[index] =
                        self.bgx_internal[index].wrapping_add(self.bgpb[index] as i32);
                    self.bgy_internal[index] =
                        self.bgy_internal[index].wrapping_add(self.bgpd[index] as i32);
                }
            }
            self.vcount += 1;
            if self.vcount >= TOTAL_LINES {
                self.vcount = 0;
                events |= EVENT_FRAME;
            }

            if self.vcount == HEIGHT as u16 {
                self.dispstat |= 1;
                events |= EVENT_VBLANK;
                if self.dispstat & 1 << 3 != 0 {
                    irq |= 1 << 0;
                }
                self.bgx_internal = self.bgx;
                self.bgy_internal = self.bgy;
            } else if self.vcount == TOTAL_LINES - 1 {
                // The vblank flag drops on the last line, not at line zero.
                self.dispstat &= !1;
            }

            let compare = (self.dispstat >> 8) & 0xff;
            if self.vcount == compare {
                self.dispstat |= 4;
                if self.dispstat & 1 << 5 != 0 {
                    irq |= 1 << 2;
                }
            } else {
                self.dispstat &= !4;
            }

            if self.cycles >= HBLANK_CYCLES {
                self.dispstat |= 2;
                self.hblank_line = self.vcount;
                events |= EVENT_HBLANK_ANY;
                if self.vcount < HEIGHT as u16 {
                    self.render_scanline();
                    events |= EVENT_HBLANK;
                }
                if self.dispstat & 1 << 4 != 0 {
                    irq |= 1 << 1;
                }
            }
        }
        (irq, events)
    }

    /// Cycles left before the next PPU state change.
    pub fn cycles_until_event(&self) -> u32 {
        if self.cycles < HBLANK_CYCLES {
            HBLANK_CYCLES - self.cycles
        } else {
            CYCLES_PER_LINE - self.cycles
        }
    }

    /// Re-renders the whole screen from the current register state. Only used
    /// by the debugging tools, since it ignores mid-frame register changes.
    pub fn render_all(&mut self) {
        let saved = self.vcount;
        self.bgx_internal = self.bgx;
        self.bgy_internal = self.bgy;
        for line in 0..HEIGHT as u16 {
            self.vcount = line;
            self.render_scanline();
            for index in 0..2 {
                self.bgx_internal[index] =
                    self.bgx_internal[index].wrapping_add(self.bgpb[index] as i32);
                self.bgy_internal[index] =
                    self.bgy_internal[index].wrapping_add(self.bgpd[index] as i32);
            }
        }
        self.vcount = saved;
    }

    pub fn set_bgx(&mut self, index: usize, value: i32) {
        self.bgx[index] = value;
        self.bgx_internal[index] = value;
    }

    pub fn set_bgy(&mut self, index: usize, value: i32) {
        self.bgy[index] = value;
        self.bgy_internal[index] = value;
    }

    #[inline]
    fn palette16(&self, index: usize) -> u16 {
        let index = (index * 2) & 0x3fe;
        u16::from_le_bytes([self.palette[index], self.palette[index + 1]]) & 0x7fff
    }

    #[inline]
    fn vram16(&self, offset: usize) -> u16 {
        if offset + 1 >= self.vram.len() {
            return 0;
        }
        u16::from_le_bytes([self.vram[offset], self.vram[offset + 1]])
    }

    fn render_scanline(&mut self) {
        let line = self.vcount as usize;
        if self.dispcnt & 0x0080 != 0 {
            // Forced blank shows white.
            for pixel in &mut self.framebuffer[line * WIDTH..(line + 1) * WIDTH] {
                *pixel = 0x00ff_ffff;
            }
            return;
        }

        for background in self.bg_line.iter_mut() {
            background.fill(TRANSPARENT);
        }
        self.obj_line = [ObjPixel {
            color: TRANSPARENT,
            priority: 4,
            semi_transparent: false,
        }; WIDTH];
        self.obj_window = [false; WIDTH];

        let mode = self.dispcnt & 7;
        match mode {
            0 => {
                for background in 0..4 {
                    if self.dispcnt & (0x0100 << background) != 0 {
                        self.render_text_bg(background, line);
                    }
                }
            }
            1 => {
                for background in 0..2 {
                    if self.dispcnt & (0x0100 << background) != 0 {
                        self.render_text_bg(background, line);
                    }
                }
                if self.dispcnt & 0x0400 != 0 {
                    self.render_affine_bg(2);
                }
            }
            2 => {
                for background in 2..4 {
                    if self.dispcnt & (0x0100 << background) != 0 {
                        self.render_affine_bg(background);
                    }
                }
            }
            3..=5 if self.dispcnt & 0x0400 != 0 => self.render_bitmap_bg(mode),
            _ => {}
        }

        if self.dispcnt & 0x1000 != 0 {
            self.render_sprites(line);
        }
        self.compose(line);
        if self.green_swap & 1 != 0 {
            self.swap_green(line);
        }
    }

    /// Exchanges the green channel between each pair of neighbouring pixels.
    fn swap_green(&mut self, line: usize) {
        let row = &mut self.framebuffer[line * WIDTH..(line + 1) * WIDTH];
        for pair in row.as_chunks_mut::<2>().0 {
            let left_green = pair[0] & 0x0000_ff00;
            let right_green = pair[1] & 0x0000_ff00;
            pair[0] = (pair[0] & !0x0000_ff00) | right_green;
            pair[1] = (pair[1] & !0x0000_ff00) | left_green;
        }
    }

    fn mosaic_x(&self, background: usize, x: usize) -> usize {
        if self.bgcnt[background] & 0x40 == 0 {
            return x;
        }
        let size = (self.mosaic & 0xf) as usize + 1;
        x - x % size
    }

    fn mosaic_y(&self, background: usize, y: usize) -> usize {
        if self.bgcnt[background] & 0x40 == 0 {
            return y;
        }
        let size = ((self.mosaic >> 4) & 0xf) as usize + 1;
        y - y % size
    }

    fn render_text_bg(&mut self, background: usize, line: usize) {
        let control = self.bgcnt[background];
        let char_base = ((control >> 2) & 3) as usize * 0x4000;
        let screen_base = ((control >> 8) & 0x1f) as usize * 0x800;
        let color256 = control & 0x80 != 0;
        let size = (control >> 14) & 3;
        let width = if size & 1 != 0 { 512 } else { 256 };
        let height = if size & 2 != 0 { 512 } else { 256 };

        let scroll_x = self.bghofs[background] as usize & 0x1ff;
        let scroll_y = self.bgvofs[background] as usize & 0x1ff;
        let source_y = (self.mosaic_y(background, line) + scroll_y) & (height - 1);

        for x in 0..WIDTH {
            let source_x = (self.mosaic_x(background, x) + scroll_x) & (width - 1);
            let block = (source_x / 256) + (source_y / 256) * (width / 256);
            let map = screen_base + block * 0x800;
            let tile_x = (source_x % 256) / 8;
            let tile_y = (source_y % 256) / 8;
            let entry = self.vram16(map + (tile_y * 32 + tile_x) * 2);
            let tile = (entry & 0x3ff) as usize;
            let mut inner_x = source_x & 7;
            let mut inner_y = source_y & 7;
            if entry & 0x0400 != 0 {
                inner_x = 7 - inner_x;
            }
            if entry & 0x0800 != 0 {
                inner_y = 7 - inner_y;
            }
            let index = if color256 {
                let offset = char_base + tile * 64 + inner_y * 8 + inner_x;
                if offset >= 0x10000 {
                    continue;
                }
                self.vram[offset] as usize
            } else {
                let offset = char_base + tile * 32 + inner_y * 4 + inner_x / 2;
                if offset >= 0x10000 {
                    continue;
                }
                let packed = self.vram[offset];
                let nibble = if inner_x & 1 == 0 {
                    packed & 0xf
                } else {
                    packed >> 4
                } as usize;
                if nibble == 0 {
                    0
                } else {
                    ((entry >> 12) as usize) * 16 + nibble
                }
            };
            if index == 0 {
                continue;
            }
            self.bg_line[background][x] = self.palette16(index);
        }
    }

    fn render_affine_bg(&mut self, background: usize) {
        let index = background - 2;
        let control = self.bgcnt[background];
        let char_base = ((control >> 2) & 3) as usize * 0x4000;
        let screen_base = ((control >> 8) & 0x1f) as usize * 0x800;
        let size_tiles = 16usize << ((control >> 14) & 3);
        let size_pixels = (size_tiles * 8) as i32;
        let wrap = control & 0x2000 != 0;

        let pa = self.bgpa[index] as i32;
        let pc = self.bgpc[index] as i32;
        let mut x = self.bgx_internal[index];
        let mut y = self.bgy_internal[index];

        for screen_x in 0..WIDTH {
            let mut sample_x = x >> 8;
            let mut sample_y = y >> 8;
            x = x.wrapping_add(pa);
            y = y.wrapping_add(pc);
            if wrap {
                sample_x = sample_x.rem_euclid(size_pixels);
                sample_y = sample_y.rem_euclid(size_pixels);
            } else if sample_x < 0 || sample_x >= size_pixels || sample_y < 0 || sample_y >= size_pixels
            {
                continue;
            }
            let tile_x = (sample_x / 8) as usize;
            let tile_y = (sample_y / 8) as usize;
            let map = screen_base + tile_y * size_tiles + tile_x;
            if map >= self.vram.len() {
                continue;
            }
            let tile = self.vram[map] as usize;
            let offset = char_base + tile * 64 + (sample_y as usize & 7) * 8 + (sample_x as usize & 7);
            if offset >= 0x10000 {
                continue;
            }
            let color = self.vram[offset] as usize;
            if color == 0 {
                continue;
            }
            self.bg_line[background][screen_x] = self.palette16(color);
        }
    }

    fn render_bitmap_bg(&mut self, mode: u16) {
        let pa = self.bgpa[0] as i32;
        let pc = self.bgpc[0] as i32;
        let mut x = self.bgx_internal[0];
        let mut y = self.bgy_internal[0];
        let (bitmap_width, bitmap_height) = if mode == 5 { (160, 128) } else { (240, 160) };
        let page = if self.dispcnt & 0x10 != 0 { 0xa000 } else { 0 };

        for screen_x in 0..WIDTH {
            let sample_x = x >> 8;
            let sample_y = y >> 8;
            x = x.wrapping_add(pa);
            y = y.wrapping_add(pc);
            if sample_x < 0 || sample_x >= bitmap_width || sample_y < 0 || sample_y >= bitmap_height {
                continue;
            }
            let position = (sample_y * bitmap_width + sample_x) as usize;
            let color = match mode {
                3 => {
                    let value = self.vram16(position * 2);
                    value & 0x7fff
                }
                4 => {
                    let index = self.vram[page + position] as usize;
                    if index == 0 {
                        continue;
                    }
                    self.palette16(index)
                }
                _ => self.vram16(page + position * 2) & 0x7fff,
            };
            self.bg_line[2][screen_x] = color;
        }
    }

    fn render_sprites(&mut self, line: usize) {
        const SHAPES: [[(usize, usize); 4]; 3] = [
            [(8, 8), (16, 16), (32, 32), (64, 64)],
            [(16, 8), (32, 8), (32, 16), (64, 32)],
            [(8, 16), (8, 32), (16, 32), (32, 64)],
        ];
        let one_dimensional = self.dispcnt & 0x40 != 0;
        let bitmap_mode = (self.dispcnt & 7) >= 3;
        let mosaic_x_size = ((self.mosaic >> 8) & 0xf) as usize + 1;
        let mosaic_y_size = ((self.mosaic >> 12) & 0xf) as usize + 1;

        for sprite in 0..128 {
            let base = sprite * 8;
            let attr0 = u16::from_le_bytes([self.oam[base], self.oam[base + 1]]);
            let attr1 = u16::from_le_bytes([self.oam[base + 2], self.oam[base + 3]]);
            let attr2 = u16::from_le_bytes([self.oam[base + 4], self.oam[base + 5]]);

            let affine = attr0 & 0x100 != 0;
            let double = attr0 & 0x200 != 0;
            if !affine && double {
                continue; // Disabled sprite.
            }
            let shape = ((attr0 >> 14) & 3) as usize;
            if shape == 3 {
                continue;
            }
            let size = ((attr1 >> 14) & 3) as usize;
            let (width, height) = SHAPES[shape][size];
            let (box_width, box_height) = if affine && double {
                (width * 2, height * 2)
            } else {
                (width, height)
            };

            let sprite_y = (attr0 & 0xff) as usize;
            let row = (line + 256 - sprite_y) & 0xff;
            if row >= box_height {
                continue;
            }
            let sprite_x = (((attr1 & 0x1ff) as u32) << 23) as i32 >> 23;

            let mode = (attr0 >> 10) & 3;
            let mosaic = attr0 & 0x1000 != 0;
            let color256 = attr0 & 0x2000 != 0;
            let priority = ((attr2 >> 10) & 3) as u8;
            let palette_bank = ((attr2 >> 12) & 0xf) as usize;
            let tile_number = (attr2 & 0x3ff) as usize;
            if bitmap_mode && tile_number < 512 {
                continue;
            }

            let (pa, pb, pc, pd) = if affine {
                let group = ((attr1 >> 9) & 0x1f) as usize * 32;
                (
                    i16::from_le_bytes([self.oam[group + 6], self.oam[group + 7]]) as i32,
                    i16::from_le_bytes([self.oam[group + 14], self.oam[group + 15]]) as i32,
                    i16::from_le_bytes([self.oam[group + 22], self.oam[group + 23]]) as i32,
                    i16::from_le_bytes([self.oam[group + 30], self.oam[group + 31]]) as i32,
                )
            } else {
                (0x100, 0, 0, 0x100)
            };

            let flip_x = !affine && attr1 & 0x1000 != 0;
            let flip_y = !affine && attr1 & 0x2000 != 0;
            let row = if mosaic { row - row % mosaic_y_size } else { row };

            let half_width = (box_width / 2) as i32;
            let half_height = (box_height / 2) as i32;
            let offset_y = row as i32 - half_height;

            for column in 0..box_width {
                let screen_x = sprite_x + column as i32;
                if screen_x < 0 || screen_x >= WIDTH as i32 {
                    continue;
                }
                let screen_x = screen_x as usize;
                let column = if mosaic {
                    column - column % mosaic_x_size
                } else {
                    column
                };
                let offset_x = column as i32 - half_width;

                let (mut texture_x, mut texture_y) = if affine {
                    (
                        ((pa * offset_x + pb * offset_y) >> 8) + (width / 2) as i32,
                        ((pc * offset_x + pd * offset_y) >> 8) + (height / 2) as i32,
                    )
                } else {
                    (offset_x + half_width, offset_y + half_height)
                };
                if texture_x < 0
                    || texture_x >= width as i32
                    || texture_y < 0
                    || texture_y >= height as i32
                {
                    continue;
                }
                if flip_x {
                    texture_x = width as i32 - 1 - texture_x;
                }
                if flip_y {
                    texture_y = height as i32 - 1 - texture_y;
                }
                let texture_x = texture_x as usize;
                let texture_y = texture_y as usize;

                let blocks = if color256 { 2 } else { 1 };
                let tile_row = texture_y / 8;
                let tile_column = texture_x / 8;
                let block = if one_dimensional {
                    tile_number + tile_row * (width / 8) * blocks + tile_column * blocks
                } else {
                    (tile_number & !(blocks - 1)) + tile_row * 32 + tile_column * blocks
                };
                let inner_y = texture_y & 7;
                let inner_x = texture_x & 7;
                let index = if color256 {
                    let address = 0x10000 + (block & 0x3ff) * 32 + inner_y * 8 + inner_x;
                    if address >= self.vram.len() {
                        continue;
                    }
                    self.vram[address] as usize
                } else {
                    let address = 0x10000 + (block & 0x3ff) * 32 + inner_y * 4 + inner_x / 2;
                    if address >= self.vram.len() {
                        continue;
                    }
                    let packed = self.vram[address];
                    let nibble = if inner_x & 1 == 0 {
                        packed & 0xf
                    } else {
                        packed >> 4
                    } as usize;
                    if nibble == 0 {
                        0
                    } else {
                        palette_bank * 16 + nibble
                    }
                };
                if index == 0 {
                    continue;
                }
                if mode == 2 {
                    self.obj_window[screen_x] = true;
                    continue;
                }
                let existing = self.obj_line[screen_x];
                if existing.color != TRANSPARENT && existing.priority <= priority {
                    continue;
                }
                self.obj_line[screen_x] = ObjPixel {
                    color: self.palette16(256 + index),
                    priority,
                    semi_transparent: mode == 1,
                };
            }
        }
    }

    /// Returns the enable bits that apply to this pixel: bits 0-3 backgrounds,
    /// bit 4 sprites, bit 5 colour effects.
    fn window_mask(&self, x: usize, line: usize) -> u16 {
        let any_window = self.dispcnt & 0xe000 != 0;
        if !any_window {
            return 0x3f;
        }
        for window in 0..2 {
            if self.dispcnt & (0x2000 << window) == 0 {
                continue;
            }
            let left = (self.winh[window] >> 8) as usize;
            let mut right = (self.winh[window] & 0xff) as usize;
            let top = (self.winv[window] >> 8) as usize;
            let mut bottom = (self.winv[window] & 0xff) as usize;
            if right > WIDTH || left > right {
                right = WIDTH;
            }
            if bottom > HEIGHT || top > bottom {
                bottom = HEIGHT;
            }
            let inside_x = x >= left && x < right;
            let inside_y = line >= top && line < bottom;
            if inside_x && inside_y {
                return (self.winin >> (window * 8)) & 0x3f;
            }
        }
        if self.dispcnt & 0x8000 != 0 && self.obj_window[x] {
            return (self.winout >> 8) & 0x3f;
        }
        self.winout & 0x3f
    }

    fn compose(&mut self, line: usize) {
        let backdrop = self.palette16(0);
        let effect = (self.bldcnt >> 6) & 3;
        let first_target = self.bldcnt & 0x3f;
        let second_target = (self.bldcnt >> 8) & 0x3f;
        let eva = (self.bldalpha & 0x1f).min(16) as u32;
        let evb = ((self.bldalpha >> 8) & 0x1f).min(16) as u32;
        let evy = (self.bldy & 0x1f).min(16) as u32;

        for x in 0..WIDTH {
            let mask = self.window_mask(x, line);

            // Find the two front-most layers. Layer 5 is the backdrop.
            let mut top_color = backdrop;
            let mut top_layer = 5usize;
            let mut second_color = backdrop;
            let mut second_layer = 5usize;
            let mut top_priority = 4u8;
            let mut found = false;

            for priority in 0..4u8 {
                if mask & 0x10 != 0
                    && self.obj_line[x].color != TRANSPARENT
                    && self.obj_line[x].priority == priority
                {
                    let color = self.obj_line[x].color;
                    if !found {
                        top_color = color;
                        top_layer = 4;
                        top_priority = priority;
                        found = true;
                    } else {
                        second_color = color;
                        second_layer = 4;
                        break;
                    }
                }
                let mut done = false;
                for background in 0..4usize {
                    if mask & (1 << background) == 0
                        || self.dispcnt & (0x0100 << background) == 0
                        || (self.bgcnt[background] & 3) as u8 != priority
                        || self.bg_line[background][x] == TRANSPARENT
                    {
                        continue;
                    }
                    let color = self.bg_line[background][x];
                    if !found {
                        top_color = color;
                        top_layer = background;
                        top_priority = priority;
                        found = true;
                    } else {
                        second_color = color;
                        second_layer = background;
                        done = true;
                        break;
                    }
                }
                if done {
                    break;
                }
            }
            let _ = top_priority;

            let top_bit = 1u16 << top_layer.min(5);
            let second_bit = 1u16 << second_layer.min(5);
            let semi = top_layer == 4 && self.obj_line[x].semi_transparent;
            let effects_allowed = mask & 0x20 != 0;

            let color = if semi && second_target & second_bit != 0 && effects_allowed {
                blend(top_color, second_color, eva, evb)
            } else if !effects_allowed || effect == 0 || first_target & top_bit == 0 {
                top_color
            } else {
                match effect {
                    1 => {
                        if second_target & second_bit != 0 && second_layer != top_layer {
                            blend(top_color, second_color, eva, evb)
                        } else {
                            top_color
                        }
                    }
                    2 => brighten(top_color, evy),
                    _ => darken(top_color, evy),
                }
            };
            self.framebuffer[line * WIDTH + x] = to_rgb(color);
        }
    }
}

#[inline]
fn blend(top: u16, bottom: u16, eva: u32, evb: u32) -> u16 {
    let mut result = 0u16;
    for channel in 0..3 {
        let a = ((top >> (channel * 5)) & 0x1f) as u32;
        let b = ((bottom >> (channel * 5)) & 0x1f) as u32;
        let value = ((a * eva + b * evb) >> 4).min(31);
        result |= (value as u16) << (channel * 5);
    }
    result
}

#[inline]
fn brighten(color: u16, evy: u32) -> u16 {
    let mut result = 0u16;
    for channel in 0..3 {
        let value = ((color >> (channel * 5)) & 0x1f) as u32;
        let value = value + (((31 - value) * evy) >> 4);
        result |= (value.min(31) as u16) << (channel * 5);
    }
    result
}

#[inline]
fn darken(color: u16, evy: u32) -> u16 {
    let mut result = 0u16;
    for channel in 0..3 {
        let value = ((color >> (channel * 5)) & 0x1f) as u32;
        let value = value - ((value * evy) >> 4);
        result |= ((value & 0x1f) as u16) << (channel * 5);
    }
    result
}

#[inline]
pub fn to_rgb(color: u16) -> u32 {
    let r = (color & 0x1f) as u32;
    let g = ((color >> 5) & 0x1f) as u32;
    let b = ((color >> 10) & 0x1f) as u32;
    let expand = |value: u32| (value << 3) | (value >> 2);
    (expand(r) << 16) | (expand(g) << 8) | expand(b)
}

impl crate::state::Snapshot for Ppu {
    fn save(&self, writer: &mut crate::state::Writer) {
        writer.bytes(&self.vram);
        writer.bytes(&self.palette);
        writer.bytes(&self.oam);
        writer.u16(self.dispcnt);
        writer.u16(self.green_swap);
        writer.u16(self.dispstat);
        writer.u16(self.vcount);
        for index in 0..4 {
            writer.u16(self.bgcnt[index]);
            writer.u16(self.bghofs[index]);
            writer.u16(self.bgvofs[index]);
        }
        for index in 0..2 {
            writer.i16(self.bgpa[index]);
            writer.i16(self.bgpb[index]);
            writer.i16(self.bgpc[index]);
            writer.i16(self.bgpd[index]);
            writer.i32(self.bgx[index]);
            writer.i32(self.bgy[index]);
            writer.i32(self.bgx_internal[index]);
            writer.i32(self.bgy_internal[index]);
            writer.u16(self.winh[index]);
            writer.u16(self.winv[index]);
        }
        writer.u16(self.winin);
        writer.u16(self.winout);
        writer.u16(self.mosaic);
        writer.u16(self.bldcnt);
        writer.u16(self.bldalpha);
        writer.u16(self.bldy);
        writer.u32(self.cycles);
        writer.u16(self.hblank_line);
        writer.usize(self.framebuffer.len());
        for pixel in &self.framebuffer {
            writer.u32(*pixel);
        }
    }

    fn load(&mut self, reader: &mut crate::state::Reader) -> Result<(), crate::state::StateError> {
        reader.into_bytes(&mut self.vram)?;
        reader.into_bytes(&mut self.palette)?;
        reader.into_bytes(&mut self.oam)?;
        self.dispcnt = reader.u16()?;
        self.green_swap = reader.u16()?;
        self.dispstat = reader.u16()?;
        self.vcount = reader.u16()?;
        for index in 0..4 {
            self.bgcnt[index] = reader.u16()?;
            self.bghofs[index] = reader.u16()?;
            self.bgvofs[index] = reader.u16()?;
        }
        for index in 0..2 {
            self.bgpa[index] = reader.i16()?;
            self.bgpb[index] = reader.i16()?;
            self.bgpc[index] = reader.i16()?;
            self.bgpd[index] = reader.i16()?;
            self.bgx[index] = reader.i32()?;
            self.bgy[index] = reader.i32()?;
            self.bgx_internal[index] = reader.i32()?;
            self.bgy_internal[index] = reader.i32()?;
            self.winh[index] = reader.u16()?;
            self.winv[index] = reader.u16()?;
        }
        self.winin = reader.u16()?;
        self.winout = reader.u16()?;
        self.mosaic = reader.u16()?;
        self.bldcnt = reader.u16()?;
        self.bldalpha = reader.u16()?;
        self.bldy = reader.u16()?;
        self.cycles = reader.u32()?;
        self.hblank_line = reader.u16()?;
        let pixels = reader.usize()?;
        for index in 0..pixels {
            let value = reader.u32()?;
            if let Some(pixel) = self.framebuffer.get_mut(index) {
                *pixel = value;
            }
        }
        Ok(())
    }
}
