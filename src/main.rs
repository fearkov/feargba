use fear_gba::{Bus, Cpu, HEIGHT, WIDTH};
use minifb::{Key, Scale, Window, WindowOptions};
use std::{
    env, fs,
    time::{Duration, Instant},
};

fn run_demo() -> Result<(), Box<dyn std::error::Error>> {
    let mut bus = Bus::new(vec![0; 4], None);
    bus.io[0] = 3;
    let mut window = Window::new(
        "fear-gba demo",
        WIDTH,
        HEIGHT,
        WindowOptions {
            scale: Scale::X2,
            resize: false,
            ..WindowOptions::default()
        },
    )?;
    window.set_target_fps(60);
    let mut frame = 0u16;
    while window.is_open() && !window.is_key_down(Key::Escape) {
        for pixel in 0..(WIDTH * HEIGHT) {
            let x = (pixel % WIDTH) as u16;
            let y = (pixel / WIDTH) as u16;
            let color = ((x.wrapping_add(frame) / 4) & 0x1f)
                | (((y.wrapping_add(frame / 2) / 4) & 0x1f) << 5)
                | (((x.wrapping_add(y).wrapping_add(frame) / 5) & 0x1f) << 10);
            bus.vram[pixel * 2..pixel * 2 + 2].copy_from_slice(&color.to_le_bytes());
        }
        let buffer = bus.framebuffer();
        window.update_with_buffer(&buffer, WIDTH, HEIGHT)?;
        frame = frame.wrapping_add(1);
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--demo") {
        return run_demo();
    }
    let rom_path = args
        .first()
        .ok_or("usage: fear-gba <game.gba> [bios.bin] [--diagnose] | --demo")?;
    let diagnose = args.iter().any(|arg| arg == "--diagnose");
    let rom = fs::read(&rom_path)?;
    let bios = args
        .get(1)
        .filter(|path| path.as_str() != "--diagnose")
        .map(fs::read)
        .transpose()?;
    if rom.is_empty() {
        return Err("ROM is empty".into());
    }
    let mut bus = Bus::new(rom, bios);
    let save_path = format!("{}.sav", rom_path);
    if let Ok(save) = fs::read(&save_path) {
        bus.load_save(&save);
    }
    let mut cpu = Cpu::new();
    cpu.fast_boot(&mut bus);
    if diagnose {
        for _ in 0..1_000_000 {
            cpu.step(&mut bus);
        }
        println!(
            "pc={:08x} cpsr={:08x} dispcnt={:04x} vram_nonzero={} last={:08x}:{:08x} previous_valid={:08x}:{:08x} first_thumb={:08x}:{:08x} first_arm_after_thumb={:08x}:{:08x} first_invalid={:08x}:{:08x} first_unsupported={:08x}:{:08x} unsupported={}",
            cpu.r[15],
            cpu.cpsr,
            bus.read16(0x0400_0000),
            bus.vram.iter().any(|byte| *byte != 0),
            cpu.last_pc,
            cpu.last_instruction,
            cpu.last_valid_pc,
            cpu.last_valid_instruction,
            cpu.first_thumb_pc.unwrap_or(0),
            cpu.first_thumb_instruction,
            cpu.first_arm_after_thumb_pc.unwrap_or(0),
            cpu.first_arm_after_thumb_instruction,
            cpu.first_invalid_pc.unwrap_or(0),
            cpu.first_invalid_instruction,
            cpu.first_unsupported_pc.unwrap_or(0),
            cpu.first_unsupported_instruction,
            cpu.unsupported
        );
        return Ok(());
    }
    let mut window = Window::new(
        "fear-gba",
        WIDTH,
        HEIGHT,
        WindowOptions {
            scale: Scale::X2,
            resize: false,
            ..WindowOptions::default()
        },
    )?;
    window.set_target_fps(60);
    let mut last_frame = Instant::now();
    while window.is_open() && !window.is_key_down(Key::Escape) {
        for _ in 0..280_896 {
            cpu.step(&mut bus);
        }
        let frame = bus.framebuffer();
        window.update_with_buffer(&frame, WIDTH, HEIGHT)?;
        let mut pressed = 0u16;
        if window.is_key_down(Key::Z) {
            pressed |= 1 << 0;
        }
        if window.is_key_down(Key::X) {
            pressed |= 1 << 1;
        }
        if window.is_key_down(Key::A) {
            pressed |= 1 << 2;
        }
        if window.is_key_down(Key::S) {
            pressed |= 1 << 3;
        }
        if window.is_key_down(Key::Up) {
            pressed |= 1 << 6;
        }
        if window.is_key_down(Key::Down) {
            pressed |= 1 << 7;
        }
        if window.is_key_down(Key::Left) {
            pressed |= 1 << 5;
        }
        if window.is_key_down(Key::Right) {
            pressed |= 1 << 4;
        }
        if window.is_key_down(Key::Enter) {
            pressed |= 1 << 3;
        }
        if window.is_key_down(Key::Backspace) {
            pressed |= 1 << 2;
        }
        bus.set_keypad(pressed);
        let elapsed = last_frame.elapsed();
        if elapsed < Duration::from_millis(16) {
            std::thread::sleep(Duration::from_millis(16) - elapsed);
        }
        last_frame = Instant::now();
    }
    fs::write(save_path, &bus.save)?;
    Ok(())
}
