//! `pipit` — headless runner for the emulator core.
//!
//! ```text
//! pipit run game.gba --frames 600 --screenshot build/shot.png
//! pipit run test.gba --frames 60 --regs
//! pipit bench game.gba
//! ```

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use pipit_gba::{Gba, Keys, SCREEN_HEIGHT, SCREEN_WIDTH};

#[derive(Parser)]
#[command(name = "pipit", version, about = "Headless GBA emulator runner")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a ROM for a number of frames.
    Run {
        rom: PathBuf,
        /// Frames to emulate.
        #[arg(long, default_value_t = 600)]
        frames: u32,
        /// Write the final frame as a PNG.
        #[arg(long)]
        screenshot: Option<PathBuf>,
        /// Optional BIOS image (uses the built-in replacement otherwise).
        #[arg(long)]
        bios: Option<PathBuf>,
        /// Load this save file before running and write it back afterwards.
        #[arg(long)]
        save: Option<PathBuf>,
        /// Print the CPU registers at the end.
        #[arg(long)]
        regs: bool,
        /// Trace N instructions (address, opcode, registers) to stderr.
        #[arg(long)]
        trace: Option<u64>,
        /// Skip this many instructions before tracing starts.
        #[arg(long, default_value_t = 0)]
        trace_skip: u64,
        /// Key script: `frame=KEYS` entries separated by commas, keys joined with `+`,
        /// e.g. `--keys 300=START,330=,600=A+RIGHT`. A key stays held until changed.
        #[arg(long)]
        keys: Option<String>,
    },
    /// Measure emulation speed over a number of frames.
    Bench {
        rom: PathBuf,
        #[arg(long, default_value_t = 1800)]
        frames: u32,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Run { rom, frames, screenshot, bios, save, regs, trace, trace_skip, keys } => {
            let rom_data = fs::read(&rom).with_context(|| format!("reading {}", rom.display()))?;
            let bios_data = bios.map(fs::read).transpose().context("reading BIOS")?;
            let mut gba = Gba::new(rom_data, bios_data);
            println!("{} [{}] save: {:?}", gba.title(), gba.game_code(), gba.bus.cart.save_type());
            if let Some(path) = &save {
                if let Ok(data) = fs::read(path) {
                    gba.load_save_data(&data);
                }
            }
            if let Some(count) = trace {
                for _ in 0..trace_skip {
                    gba.step();
                }
                for _ in 0..count {
                    let r = &gba.cpu.regs;
                    eprintln!(
                        "{:08X} {:08X} [{}] r0={:08X} r1={:08X} r2={:08X} r3={:08X} r12={:08X} sp={:08X} lr={:08X} cpsr={:08X}",
                        gba.cpu.pc(),
                        gba.cpu.next_opcode(),
                        if gba.cpu.is_thumb() { "T" } else { "A" },
                        r[0], r[1], r[2], r[3], r[12], r[13], r[14], gba.cpu.cpsr
                    );
                    gba.step();
                }
            }
            let script = keys.as_deref().map(parse_key_script).transpose()?.unwrap_or_default();
            for frame in 0..frames {
                if let Some((_, keys)) = script.iter().find(|(f, _)| *f == frame) {
                    gba.set_keys(*keys);
                }
                gba.run_frame();
            }
            if regs {
                print_registers(&gba);
            }
            if let Some(path) = screenshot {
                write_png(&path, gba.framebuffer())?;
                println!("wrote {}", path.display());
            }
            if let (Some(path), Some(data)) = (save, gba.save_data()) {
                fs::write(&path, data)?;
            }
        }
        Command::Bench { rom, frames } => {
            let rom_data = fs::read(&rom)?;
            let mut gba = Gba::new(rom_data, None);
            let start = Instant::now();
            for _ in 0..frames {
                gba.run_frame();
            }
            let secs = start.elapsed().as_secs_f64();
            let fps = f64::from(frames) / secs;
            println!(
                "{frames} frames in {secs:.2}s = {fps:.0} fps ({:.1}x real time)",
                fps / 59.73
            );
        }
    }
    Ok(())
}

fn parse_key_script(script: &str) -> Result<Vec<(u32, Keys)>> {
    let mut out = Vec::new();
    for entry in script.split(',').filter(|e| !e.trim().is_empty()) {
        let (frame, names) =
            entry.split_once('=').context("key script entries look like 300=START")?;
        let frame: u32 = frame.trim().parse().context("bad frame number in key script")?;
        let mut keys = Keys::NONE;
        for name in names.split('+').map(str::trim).filter(|n| !n.is_empty()) {
            keys |= match name.to_ascii_uppercase().as_str() {
                "A" => Keys::A,
                "B" => Keys::B,
                "SELECT" => Keys::SELECT,
                "START" => Keys::START,
                "RIGHT" => Keys::RIGHT,
                "LEFT" => Keys::LEFT,
                "UP" => Keys::UP,
                "DOWN" => Keys::DOWN,
                "R" => Keys::R,
                "L" => Keys::L,
                other => anyhow::bail!("unknown key {other}"),
            };
        }
        out.push((frame, keys));
    }
    Ok(out)
}

fn print_registers(gba: &Gba) {
    let r = &gba.cpu.regs;
    for (i, v) in r.iter().enumerate() {
        print!("r{i:<2}={v:08X} ");
        if i % 4 == 3 {
            println!();
        }
    }
    println!(
        "cpsr={:08X} pc={:08X} cycles={}",
        gba.cpu.cpsr,
        gba.cpu.pc(),
        gba.bus.scheduler.now()
    );
}

fn write_png(path: &PathBuf, pixels: &[u32]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = fs::File::create(path)?;
    let mut encoder = png::Encoder::new(file, SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let rgb: Vec<u8> =
        pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    writer.write_image_data(&rgb)?;
    Ok(())
}
