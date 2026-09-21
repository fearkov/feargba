use fear_gba::{png, Gba, HEIGHT, WIDTH};
use minifb::{Key, Scale, Window, WindowOptions};
use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

struct Options {
    rom: PathBuf,
    bios: Option<PathBuf>,
    headless_frames: Option<u32>,
    screenshot: Option<PathBuf>,
    report_registers: bool,
    scale: Scale,
    mute: bool,
    every: Option<u32>,
    capture_from: u32,
    save: Option<PathBuf>,
    /// Scripted button presses: (frame, keys, frames to hold).
    input: Vec<(u32, u16, u32)>,
}

fn parse_arguments() -> Result<Options, String> {
    let mut options = Options {
        rom: PathBuf::new(),
        bios: None,
        headless_frames: None,
        screenshot: None,
        report_registers: false,
        scale: Scale::X2,
        mute: false,
        every: None,
        capture_from: 0,
        save: None,
        input: Vec::new(),
    };
    let mut arguments = env::args().skip(1);
    let mut rom = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bios" => options.bios = arguments.next().map(PathBuf::from),
            "--headless" => {
                options.headless_frames = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .or(Some(600))
            }
            "--screenshot" => options.screenshot = arguments.next().map(PathBuf::from),
            "--registers" => options.report_registers = true,
            "--every" => {
                options.every = arguments.next().and_then(|value| value.parse().ok())
            }
            "--from" => {
                options.capture_from = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0)
            }
            "--input" => {
                let script = arguments.next().unwrap_or_default();
                options.input = parse_input_script(&script)?;
            }
            "--mute" => options.mute = true,
            "--save" => options.save = arguments.next().map(PathBuf::from),
            "--scale" => {
                options.scale = match arguments.next().as_deref() {
                    Some("1") => Scale::X1,
                    Some("2") => Scale::X2,
                    Some("4") => Scale::X4,
                    _ => Scale::X2,
                }
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => rom = Some(PathBuf::from(other)),
        }
    }
    options.rom = rom.ok_or_else(|| {
        "usage: fear-gba <rom.gba> [--bios bios.bin] [--scale 1|2|4] [--mute]\n\
         \x20      [--save file.sav] [--headless FRAMES] [--screenshot out.png]\n\
         \x20      [--every N] [--from FRAME] [--input SCRIPT] [--registers]"
            .to_string()
    })?;
    Ok(options)
}

/// Parses "frame:KEY+KEY[:hold],..." into scheduled button presses.
fn parse_input_script(script: &str) -> Result<Vec<(u32, u16, u32)>, String> {
    let mut events = Vec::new();
    for entry in script.split(',').filter(|part| !part.trim().is_empty()) {
        let mut fields = entry.trim().split(':');
        let frame: u32 = fields
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| format!("bad frame in {entry:?}"))?;
        let mut keys = 0u16;
        for name in fields.next().unwrap_or("").split('+') {
            keys |= match name.trim().to_ascii_uppercase().as_str() {
                "A" => fear_gba::KEY_A,
                "B" => fear_gba::KEY_B,
                "SELECT" => fear_gba::KEY_SELECT,
                "START" => fear_gba::KEY_START,
                "RIGHT" => fear_gba::KEY_RIGHT,
                "LEFT" => fear_gba::KEY_LEFT,
                "UP" => fear_gba::KEY_UP,
                "DOWN" => fear_gba::KEY_DOWN,
                "R" => fear_gba::KEY_R,
                "L" => fear_gba::KEY_L,
                "" => 0,
                other => return Err(format!("unknown key {other:?}")),
            };
        }
        let hold = fields
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(10);
        events.push((frame, keys, hold));
    }
    Ok(events)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_arguments()?;
    let rom = fs::read(&options.rom)?;
    let bios = options.bios.as_ref().map(fs::read).transpose()?;
    let mut gba = Gba::new(rom, bios);

    let save_path = options
        .save
        .clone()
        .unwrap_or_else(|| options.rom.with_extension("sav"));
    if let Ok(data) = fs::read(&save_path) {
        gba.bus.cart.load_save(&data);
    }

    if let Some(frames) = options.headless_frames {
        let start = Instant::now();
        for frame in 0..frames {
            let mut keys = 0u16;
            for (start, pressed, hold) in &options.input {
                if frame >= *start && frame < start + hold {
                    keys |= pressed;
                }
            }
            gba.set_keys(keys);
            gba.run_frame();
            gba.bus.apu.buffer.clear();
            if let (Some(every), Some(path)) = (options.every, options.screenshot.as_ref()) {
                if every > 0 && frame + 1 >= options.capture_from && (frame + 1) % every == 0 {
                    let numbered = path.with_file_name(format!(
                        "{}_{:05}.png",
                        path.file_stem().unwrap_or_default().to_string_lossy(),
                        frame + 1
                    ));
                    fs::write(&numbered, png::encode(gba.framebuffer(), WIDTH, HEIGHT))?;
                }
            }
        }
        let elapsed = start.elapsed();
        let written = gba
            .bus
            .cart
            .save
            .iter()
            .filter(|byte| **byte != 0xff)
            .count();
        eprintln!(
            "{frames} frames in {:.2}s ({:.1} fps), save {:?} dirty={} written_bytes={}",
            elapsed.as_secs_f32(),
            frames as f32 / elapsed.as_secs_f32(),
            gba.bus.cart.kind,
            gba.bus.cart.save_dirty,
            written
        );
        if options.report_registers {
            let cpu = &gba.cpu;
            eprintln!(
                "pc={:08x} cpsr={:08x} r12={} dispcnt={:04x}",
                cpu.r[15],
                cpu.cpsr(),
                cpu.r[12],
                gba.bus.ppu.dispcnt
            );
            for (index, value) in cpu.r.iter().enumerate() {
                eprint!("r{index}={value:08x} ");
            }
            eprintln!();
        }
        if let Some(path) = options.screenshot {
            fs::write(&path, png::encode(gba.framebuffer(), WIDTH, HEIGHT))?;
            eprintln!("wrote {}", path.display());
        }
        if gba.bus.cart.save_dirty && options.save.is_some() {
            fs::write(&save_path, &gba.bus.cart.save)?;
            eprintln!("wrote {}", save_path.display());
        }
        return Ok(());
    }

    let title = format!("fear-gba - {}", options.rom.display());
    let mut window = Window::new(
        &title,
        WIDTH,
        HEIGHT,
        WindowOptions {
            scale: options.scale,
            resize: true,
            ..WindowOptions::default()
        },
    )?;
    window.set_target_fps(60);

    let mut audio = if options.mute {
        None
    } else {
        audio::Output::start().ok()
    };

    let mut frames_shown = 0u32;
    let mut last_report = Instant::now();
    let mut save_due: Option<Instant> = None;

    while window.is_open() && !window.is_key_down(Key::Escape) {
        gba.set_keys(read_keys(&window));
        gba.run_frame();
        window.update_with_buffer(gba.framebuffer(), WIDTH, HEIGHT)?;
        let samples = gba.take_audio();
        match audio.as_mut() {
            Some(output) => output.push(&samples),
            None => drop(samples),
        }

        // Flush backup memory a moment after the game stops writing to it.
        if gba.bus.cart.save_dirty {
            gba.bus.cart.save_dirty = false;
            save_due = Some(Instant::now() + Duration::from_secs(1));
        }
        if save_due.is_some_and(|due| Instant::now() >= due) {
            save_due = None;
            fs::write(&save_path, &gba.bus.cart.save)?;
        }

        frames_shown += 1;
        if last_report.elapsed() >= Duration::from_secs(1) {
            let fps = frames_shown as f32 / last_report.elapsed().as_secs_f32();
            window.set_title(&format!("{title} - {fps:.0} fps"));
            frames_shown = 0;
            last_report = Instant::now();
        }
    }

    if save_due.is_some() || gba.bus.cart.save_dirty {
        fs::write(&save_path, &gba.bus.cart.save)?;
    }
    Ok(())
}

fn read_keys(window: &Window) -> u16 {
    let mut keys = 0u16;
    let bindings = [
        (Key::X, fear_gba::KEY_A),
        (Key::Z, fear_gba::KEY_B),
        (Key::Backspace, fear_gba::KEY_SELECT),
        (Key::Enter, fear_gba::KEY_START),
        (Key::Right, fear_gba::KEY_RIGHT),
        (Key::Left, fear_gba::KEY_LEFT),
        (Key::Up, fear_gba::KEY_UP),
        (Key::Down, fear_gba::KEY_DOWN),
        (Key::S, fear_gba::KEY_R),
        (Key::A, fear_gba::KEY_L),
    ];
    for (key, bit) in bindings {
        if window.is_key_down(key) {
            keys |= bit;
        }
    }
    keys
}

mod audio {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::{Arc, Mutex};

    pub struct Output {
        queue: Arc<Mutex<Vec<i16>>>,
        _stream: cpal::Stream,
    }

    impl Output {
        pub fn start() -> Result<Self, Box<dyn std::error::Error>> {
            let host = cpal::default_host();
            let device = host
                .default_output_device()
                .ok_or("no audio output device")?;
            let config = cpal::StreamConfig {
                channels: 2,
                sample_rate: cpal::SampleRate(fear_gba::apu::SAMPLE_RATE),
                buffer_size: cpal::BufferSize::Default,
            };
            let queue: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::new()));
            let consumer = Arc::clone(&queue);
            let stream = device.build_output_stream(
                &config,
                move |output: &mut [f32], _| {
                    let mut buffer = consumer.lock().unwrap();
                    let available = buffer.len().min(output.len());
                    for (slot, sample) in output.iter_mut().zip(buffer.drain(..available)) {
                        *slot = sample as f32 / 32768.0;
                    }
                    for slot in output.iter_mut().skip(available) {
                        *slot = 0.0;
                    }
                },
                |error| eprintln!("audio error: {error}"),
                None,
            )?;
            stream.play()?;
            Ok(Self {
                queue,
                _stream: stream,
            })
        }

        pub fn push(&mut self, samples: &[i16]) {
            let mut buffer = self.queue.lock().unwrap();
            // Drop backlog so audio never drifts behind the picture.
            if buffer.len() > 8192 {
                buffer.clear();
            }
            buffer.extend_from_slice(samples);
        }
    }
}
