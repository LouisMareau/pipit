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
use pipit_gba::Keys;

mod machine;
use machine::{AnyLink, Machine};

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
        /// Write the audio of the whole run as a WAV file (stereo, 32768 Hz).
        #[arg(long)]
        wav: Option<PathBuf>,
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
        /// Run this many frames before tracing starts.
        #[arg(long, default_value_t = 0)]
        trace_after: u32,
        /// Trace the DS's ARM7 instead of its ARM9.
        #[arg(long)]
        trace7: bool,
        /// Key script: `frame=KEYS` entries separated by commas, keys joined with `+`,
        /// e.g. `--keys 300=START,330=,600=A+RIGHT`. A key stays held until changed.
        #[arg(long)]
        keys: Option<String>,
        /// Print memory at the end: `ADDR[:LEN]` in hex, e.g. `--peek 03005DBC:4`.
        #[arg(long)]
        peek: Vec<String>,
        /// Like `--peek`, through the DS's ARM7 memory map.
        #[arg(long)]
        peek7: Vec<String>,
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
        /// Join the consoles with Wireless Adapters instead of a cable (GBA only).
        #[arg(long)]
        wireless: bool,
        /// Print every Wireless Adapter command and answer (`--wireless`).
        #[arg(long)]
        trace_rfu: bool,
        /// Print memory of every console at the end: `ADDR[:LEN]` in hex.
        #[arg(long)]
        peek: Vec<String>,
        /// Every N frames, snapshot the link, run on, restore and re-run the same
        /// frames, and report when the two runs disagree (what playing together
        /// does when it rolls back).
        #[arg(long, value_name = "N")]
        check_rollback: Option<u32>,
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
            wav,
            bios,
            save,
            regs,
            trace,
            trace_skip,
            trace_after,
            trace7,
            keys,
            peek,
            peek7,
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
            let script = keys.as_deref().map(parse_key_script).transpose()?.unwrap_or_default();
            let mut frame = 0u32;
            let mut audio: Vec<i16> = Vec::new();
            let mut run_to = |machine: &mut Machine, frame: &mut u32, end: u32| {
                while *frame < end {
                    if let Some((_, keys)) = script.iter().find(|(f, _)| *f == *frame) {
                        machine.set_keys(*keys);
                    }
                    machine.run_frame();
                    if wav.is_some() {
                        audio.extend(machine.drain_audio());
                    }
                    *frame += 1;
                }
            };
            if let Some(count) = trace {
                run_to(&mut machine, &mut frame, trace_after.min(frames));
                let step =
                    |machine: &mut Machine| if trace7 { machine.step7() } else { machine.step() };
                for _ in 0..trace_skip {
                    step(&mut machine);
                }
                for _ in 0..count {
                    let line = if trace7 { machine.trace_line7() } else { machine.trace_line() };
                    eprintln!("{line}");
                    step(&mut machine);
                }
            }
            run_to(&mut machine, &mut frame, frames);
            if regs {
                println!("{}", machine.registers());
            }
            for (arm7, spec) in
                peek.iter().map(|s| (false, s)).chain(peek7.iter().map(|s| (true, s)))
            {
                let (addr, len) = spec.split_once(':').unwrap_or((spec, "4"));
                let addr = u32::from_str_radix(addr.trim_start_matches("0x"), 16)
                    .context("bad --peek address")?;
                let len: u32 = len.parse().context("bad --peek length")?;
                let bytes: Vec<String> = (0..len)
                    .map(|i| {
                        let b = if arm7 { machine.peek7(addr + i) } else { machine.peek(addr + i) };
                        format!("{b:02X}")
                    })
                    .collect();
                println!("{}{addr:08X}: {}", if arm7 { "ARM7 " } else { "" }, bytes.join(" "));
            }
            if let Some(path) = screenshot {
                let (width, height) = machine.size();
                write_png(&path, machine.framebuffer(), width, height)?;
                println!("wrote {}", path.display());
            }
            if let Some(path) = &wav {
                write_wav(path, &audio, 32_768)?;
                println!("wrote {} ({:.1} s)", path.display(), audio.len() as f64 / 2.0 / 32_768.0);
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
        Command::Link {
            rom,
            players,
            frames,
            save,
            keys,
            screenshot,
            trace_sio,
            wireless,
            trace_rfu,
            peek,
            check_rollback,
            bios,
        } => {
            let roms = if rom.len() == 1 { vec![rom[0].clone(); players] } else { rom };
            anyhow::ensure!((2..=4).contains(&roms.len()), "a link joins two to four consoles");
            let bios_data = bios.map(fs::read).transpose().context("reading BIOS")?;
            let mut data = Vec::new();
            for path in &roms {
                data.push(fs::read(path).with_context(|| format!("reading {}", path.display()))?);
            }
            let mut link = AnyLink::open(&roms, data, bios_data, wireless)?;
            link.set_trace_rfu(trace_rfu);
            for i in 0..link.players() {
                if let Some(path) = save.get(i) {
                    if let Ok(bytes) = fs::read(path) {
                        link.load_save_data(i, &bytes);
                    }
                }
                println!("console {}: {}", i + 1, link.describe(i));
            }
            let scripts = keys.iter().map(|s| parse_key_script(s)).collect::<Result<Vec<_>>>()?;
            let step = |link: &mut AnyLink, frame: u32| {
                for (i, script) in scripts.iter().enumerate() {
                    if let Some((_, keys)) = script.iter().find(|(f, _)| *f == frame) {
                        link.set_keys(i, *keys);
                    }
                }
                link.run_frame();
                if trace_sio {
                    for words in link.take_transfers() {
                        let words: Vec<String> = words.iter().map(|w| format!("{w:04X}")).collect();
                        eprintln!("frame {frame}: {}", words.join(" "));
                    }
                }
                for line in link.take_rfu_log() {
                    eprintln!("frame {frame}: {line}");
                }
            };
            let mut frame = 0;
            let mut mismatches = 0;
            while frame < frames {
                let Some(every) = check_rollback.filter(|&n| n > 0) else {
                    step(&mut link, frame);
                    frame += 1;
                    continue;
                };
                let end = (frame + every).min(frames);
                let snapshot = link.save_state();
                for f in frame..end {
                    step(&mut link, f);
                }
                let first = link.state_hash();
                link.load_state(&snapshot)?;
                for f in frame..end {
                    step(&mut link, f);
                }
                if link.state_hash() != first {
                    eprintln!(
                        "rollback mismatch: frames {frame}..{end} ran differently the second time"
                    );
                    mismatches += 1;
                }
                frame = end;
            }
            println!("{} transfers over {frames} frames", link.transfers());
            if check_rollback.is_some() {
                println!("rollback check: {mismatches} mismatches");
            }
            for spec in &peek {
                let (addr, len) = spec.split_once(':').unwrap_or((spec, "1"));
                let addr = u32::from_str_radix(addr.trim_start_matches("0x"), 16)
                    .context("bad --peek address")?;
                let len: u32 = len.parse().context("bad --peek length")?;
                for i in 0..link.players() {
                    let bytes: Vec<String> =
                        (0..len).map(|o| format!("{:02X}", link.peek(i, addr + o))).collect();
                    println!("console {}: {addr:08X}: {}", i + 1, bytes.join(" "));
                }
            }
            if let Some(path) = screenshot {
                let (width, height) = link.size();
                for i in 0..link.players() {
                    let out = numbered(&path, i + 1);
                    write_png(&out, link.framebuffer(i), width, height)?;
                    println!("wrote {}", out.display());
                }
            }
            for i in 0..link.players() {
                if let (Some(path), Some(bytes)) = (save.get(i), link.save_data(i)) {
                    fs::write(path, bytes)?;
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

/// Writes interleaved stereo 16-bit samples as a WAV file.
fn write_wav(path: &PathBuf, samples: &[i16], rate: u32) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    fs::write(path, out)?;
    Ok(())
}
