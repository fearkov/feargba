//! Dumps PPU state at a given frame and renders each layer on its own.

use fear_gba::{png, Gba, HEIGHT, WIDTH};
use std::{env, fs};

fn main() {
    let mut arguments = env::args().skip(1);
    let path = arguments.next().expect("usage: layers <rom.gba> [frames] [out_dir] [script]");
    let frames: u32 = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(600);
    let directory = arguments.next().unwrap_or_else(|| ".".to_string());
    let script = arguments.next().unwrap_or_default();

    let mut events: Vec<(u32, u16, u32)> = Vec::new();
    for entry in script.split(',').filter(|part| !part.trim().is_empty()) {
        let fields: Vec<&str> = entry.split(':').collect();
        let frame = fields[0].parse().unwrap_or(0);
        let keys = match fields.get(1).copied().unwrap_or("") {
            "A" => fear_gba::KEY_A,
            "B" => fear_gba::KEY_B,
            "START" => fear_gba::KEY_START,
            "UP" => fear_gba::KEY_UP,
            "DOWN" => fear_gba::KEY_DOWN,
            "LEFT" => fear_gba::KEY_LEFT,
            "RIGHT" => fear_gba::KEY_RIGHT,
            _ => 0,
        };
        let hold = fields.get(2).and_then(|v| v.parse().ok()).unwrap_or(10);
        events.push((frame, keys, hold));
    }

    let rom = fs::read(&path).unwrap();
    let mut gba = Gba::new(rom, None);
    for frame in 0..frames {
        let mut keys = 0;
        for (start, pressed, hold) in &events {
            if frame >= *start && frame < start + hold {
                keys |= pressed;
            }
        }
        gba.set_keys(keys);
        gba.run_frame();
        gba.bus.apu.buffer.clear();
    }

    let ppu = &gba.bus.ppu;
    println!("DISPCNT {:04x}  DISPSTAT {:04x}", ppu.dispcnt, ppu.dispstat);
    for index in 0..4 {
        println!(
            "BG{index}CNT {:04x}  hofs {:3}  vofs {:3}  priority {}  charbase {:x} screenbase {:x} size {} 256col {}",
            ppu.bgcnt[index],
            ppu.bghofs[index],
            ppu.bgvofs[index],
            ppu.bgcnt[index] & 3,
            (ppu.bgcnt[index] >> 2) & 3,
            (ppu.bgcnt[index] >> 8) & 0x1f,
            (ppu.bgcnt[index] >> 14) & 3,
            ppu.bgcnt[index] & 0x80 != 0,
        );
    }
    println!(
        "WIN0H {:04x} WIN1H {:04x} WIN0V {:04x} WIN1V {:04x} WININ {:04x} WINOUT {:04x}",
        ppu.winh[0], ppu.winh[1], ppu.winv[0], ppu.winv[1], ppu.winin, ppu.winout
    );
    println!(
        "BLDCNT {:04x} BLDALPHA {:04x} BLDY {:04x} MOSAIC {:04x}",
        ppu.bldcnt, ppu.bldalpha, ppu.bldy, ppu.mosaic
    );
    println!(
        "BG2 affine pa={:04x} pb={:04x} pc={:04x} pd={:04x} x={:08x} y={:08x}",
        ppu.bgpa[0] as u16, ppu.bgpb[0] as u16, ppu.bgpc[0] as u16, ppu.bgpd[0] as u16,
        ppu.bgx[0], ppu.bgy[0]
    );

    let original = gba.bus.ppu.dispcnt;
    let names = ["bg0", "bg1", "bg2", "bg3", "obj"];
    for (index, name) in names.iter().enumerate() {
        let bit = if index == 4 { 0x1000 } else { 0x0100 << index };
        if original & bit == 0 {
            continue;
        }
        gba.bus.ppu.dispcnt = (original & !0x1f00) | bit;
        gba.bus.ppu.render_all();
        let file = format!("{directory}/layer_{name}.png");
        fs::write(&file, png::encode(&gba.bus.ppu.framebuffer, WIDTH, HEIGHT)).unwrap();
        println!("wrote {file}");
    }
    gba.bus.ppu.dispcnt = original;
    gba.bus.ppu.render_all();
    fs::write(
        format!("{directory}/layer_all.png"),
        png::encode(&gba.bus.ppu.framebuffer, WIDTH, HEIGHT),
    )
    .unwrap();
}
