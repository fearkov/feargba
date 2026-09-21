//! Reports what the APU produced, to check the sound path end to end.

use fear_gba::Gba;
use std::{env, fs};

fn main() {
    let mut arguments = env::args().skip(1);
    let path = arguments.next().expect("usage: audiostat <rom.gba> [frames]");
    let frames: u32 = arguments.next().and_then(|v| v.parse().ok()).unwrap_or(600);
    let mut gba = Gba::new(fs::read(&path).unwrap(), None);
    let mut total = 0usize;
    let mut peak = 0i32;
    let mut energy = 0f64;
    let mut nonzero_frames = 0;
    for _ in 0..frames {
        gba.run_frame();
        let samples = gba.take_audio();
        let frame_peak = samples.iter().map(|s| (*s as i32).abs()).max().unwrap_or(0);
        if frame_peak > 0 {
            nonzero_frames += 1;
        }
        peak = peak.max(frame_peak);
        for sample in &samples {
            energy += (*sample as f64) * (*sample as f64);
        }
        total += samples.len();
    }
    println!(
        "{total} samples over {frames} frames ({:.1} per frame), peak {peak}, rms {:.1}, frames with sound {nonzero_frames}",
        total as f32 / frames as f32,
        (energy / total.max(1) as f64).sqrt()
    );
    println!(
        "SOUNDCNT_L {:04x} SOUNDCNT_H {:04x} SOUNDCNT_X {:04x}",
        gba.bus.apu.control_l, gba.bus.apu.control_h, gba.bus.apu.control_x
    );
    for (index, timer) in gba.bus.timers.channel.iter().enumerate() {
        println!(
            "timer{index} control {:04x} reload {:04x}",
            timer.control, timer.reload
        );
    }
    for (index, dma) in gba.bus.dma.iter().enumerate() {
        println!(
            "dma{index} control {:04x} src {:08x} dst {:08x}",
            dma.control, dma.source, dma.destination
        );
    }
}
