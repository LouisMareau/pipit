//! `pipit` — headless runner for the emulator core.
//!
//! ```text
//! pipit run game.gba --frames 600 --screenshot build/shot.png
//! pipit run test.gba --frames 60 --regs
//! pipit bench game.gba
//! pipit link game.gba --save a.sav --save b.sav --frames 3600 --screenshot build/link.png
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use pipit_gba::{Gba, Keys, Link, SCREEN_HEIGHT, SCREEN_WIDTH};

mod machine;
use machine::Machine;

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
        /// Print memory at the end: `ADDR[:LEN]` in hex, e.g. `--peek 03005DBC:4`.
        #[arg(long)]
        peek: Vec<String>,
    },
    /// Measure emulation speed over a number of frames.
    Bench {
        rom: PathBuf,
        #[arg(long, default_value_t = 1800)]
        frames: u32,
    },
    /// Run two to four consoles joined by a link cable, in lockstep.
    Link {
        /// One ROM per console, or a single ROM shared by `--players` consoles.
        #[arg(required = true)]
        rom: Vec<PathBuf>,
        /// Number of consoles when a single ROM is given.
        #[arg(long, default_value_t = 2)]
        players: usize,
        /// Frames to emulate.
        #[arg(long, default_value_t = 600)]
        frames: u32,
        /// Save files, one per console in order; each is written back afterwards.
        #[arg(long)]
        save: Vec<PathBuf>,
        /// Key scripts, one per console in order (same syntax as `run --keys`).
        #[arg(long)]
        keys: Vec<String>,
        /// Write each console's final frame as `<name>-<n>.png`.
        #[arg(long)]
        screenshot: Option<PathBuf>,
        /// Print every transfer (frame number, then the word from each console).
        #[arg(long)]
        trace_sio: bool,
        /// Optional BIOS image (uses the built-in replacement otherwise).
        #[arg(long)]
        bios: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Run {
            rom,
            frames,
            screenshot,
            bios,
            save,
            regs,
            trace,
            trace_skip,
            keys,
            peek,
        } => {
            let rom_data = fs::read(&rom).with_context(|| format!("reading {}", rom.display()))?;
            let bios_data = bios.map(fs::read).transpose().context("reading BIOS")?;
            let mut machine = Machine::open(&rom, rom_data, bios_data);
            println!("{}", machine.describe());
            if let Some(path) = &save {
                if let Ok(data) = fs::read(path) {
                    machine.load_save_data(&data);
                }
            }
            if let Some(count) = trace {
                for _ in 0..trace_skip {
                    machine.step();
                }
                for _ in 0..count {
                    eprintln!("{}", machine.trace_line());
                    machine.step();
                }
            }
            let script = keys.as_deref().map(parse_key_script).transpose()?.unwrap_or_default();
            for frame in 0..frames {
                if let Some((_, keys)) = script.iter().find(|(f, _)| *f == frame) {
                    machine.set_keys(*keys);
                }
                machine.run_frame();
            }
            if regs {
                println!("{}", machine.registers());
            }
            for spec in &peek {
                let (addr, len) = spec.split_once(':').unwrap_or((spec, "4"));
                let addr = u32::from_str_radix(addr.trim_start_matches("0x"), 16)
                    .context("bad --peek address")?;
                let len: u32 = len.parse().context("bad --peek length")?;
                let bytes: Vec<String> =
                    (0..len).map(|i| format!("{:02X}", machine.peek(addr + i))).collect();
                println!("{addr:08X}: {}", bytes.join(" "));
            }
            if let Some(path) = screenshot {
                let (width, height) = machine.size();
                write_png(&path, machine.framebuffer(), width, height)?;
                println!("wrote {}", path.display());
            }
            if let (Some(path), Some(data)) = (save, machine.save_data()) {
                fs::write(&path, data)?;
            }
        }
        Command::Bench { rom, frames } => {
            let rom_data = fs::read(&rom)?;
            let mut machine = Machine::open(&rom, rom_data, None);
            let start = Instant::now();
            for _ in 0..frames {
                machine.run_frame();
            }
            let secs = start.elapsed().as_secs_f64();
            let fps = f64::from(frames) / secs;
            let per_frame = machine.instructions() / u64::from(frames);
            println!("{per_frame} instructions per frame");
            println!(
                "{frames} frames in {secs:.2}s = {fps:.0} fps ({:.1}x real time)",
                fps / 59.73
            );
        }
        Command::Link { rom, players, frames, save, keys, screenshot, trace_sio, bios } => {
            let roms = if rom.len() == 1 { vec![rom[0].clone(); players] } else { rom };
            anyhow::ensure!((2..=4).contains(&roms.len()), "a link joins two to four consoles");
            let game_boy = roms.iter().any(|p| {
                matches!(
                    p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
                    Some("gb" | "gbc")
                )
            });
            anyhow::ensure!(!game_boy, "Game Boy games cannot be linked yet");
            let bios_data = bios.map(fs::read).transpose().context("reading BIOS")?;
            let mut nodes = Vec::new();
            for (i, path) in roms.iter().enumerate() {
                let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
                let mut gba = Gba::new(data, bios_data.clone());
                if let Some(path) = save.get(i) {
                    if let Ok(data) = fs::read(path) {
                        gba.load_save_data(&data);
                    }
                }
                println!("console {}: {} [{}]", i + 1, gba.title(), gba.game_code());
                nodes.push(gba);
            }
            let scripts = keys.iter().map(|s| parse_key_script(s)).collect::<Result<Vec<_>>>()?;
            let mut link = Link::new(nodes);
            for frame in 0..frames {
                for (i, script) in scripts.iter().enumerate() {
                    if let Some((_, keys)) = script.iter().find(|(f, _)| *f == frame) {
                        link.nodes_mut()[i].set_keys(*keys);
                    }
                }
                link.run_frame();
                if trace_sio {
                    for w in link.take_transfers() {
                        eprintln!(
                            "frame {frame}: {:04X} {:04X} {:04X} {:04X}",
                            w[0], w[1], w[2], w[3]
                        );
                    }
                }
            }
            println!("{} transfers over {frames} frames", link.transfers());
            if let Some(path) = screenshot {
                for (i, node) in link.nodes().iter().enumerate() {
                    let out = numbered(&path, i + 1);
                    write_png(&out, node.framebuffer(), SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32)?;
                    println!("wrote {}", out.display());
                }
            }
            for (i, node) in link.nodes().iter().enumerate() {
                if let (Some(path), Some(data)) = (save.get(i), node.save_data()) {
                    fs::write(path, data)?;
                }
            }
        }
    }
    Ok(())
}

/// `build/link.png` → `build/link-2.png`.
fn numbered(path: &Path, n: usize) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("png");
    path.with_file_name(format!("{stem}-{n}.{ext}"))
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

fn write_png(path: &PathBuf, pixels: &[u32], width: u32, height: u32) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = fs::File::create(path)?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let rgb: Vec<u8> =
        pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    writer.write_image_data(&rgb)?;
    Ok(())
}
