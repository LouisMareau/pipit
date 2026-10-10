//! The bus: memory map, I/O dispatch, DMA, and the clock everything else runs
//! on. Each CPU access first ticks one M-cycle, during which the timer, picture,
//! sound and serial port advance; in double speed they advance half as far per
//! M-cycle, which is what the speed switch amounts to.

use pipit_common::snapshot;
use serde::{Deserialize, Serialize};

use crate::audio::Audio;
use crate::cartridge::Cartridge;
use crate::joypad::Joypad;
use crate::serial::Serial;
use crate::timer::Timer;
use crate::video::Video;
use crate::CLOCK_HZ;

#[derive(Serialize, Deserialize)]
pub struct Bus {
    pub cart: Cartridge,
    pub video: Video,
    pub audio: Audio,
    pub timer: Timer,
    pub joypad: Joypad,
    pub serial: Serial,
    #[serde(with = "snapshot::bytes_box")]
    wram: Box<[u8; 0x8000]>,
    #[serde(with = "snapshot::array")]
    hram: [u8; 0x7F],
    /// WRAM bank at 0xD000 (colour hardware), 1-7.
    wram_bank: u8,
    pub ie: u8,
    pub if_: u8,
    cgb: bool,
    double_speed: bool,
    /// KEY1 bit 0: the next STOP switches speed.
    speed_armed: bool,
    /// OAM DMA in progress: source page and next byte.
    oam_dma: Option<(u16, u8)>,
    hdma_src: u16,
    hdma_dst: u16,
    /// Blocks of 16 bytes left minus one; 0xFF when no HBlank DMA is running.
    hdma_blocks: u8,
    hdma_active: bool,
    /// Clocks since power-on, at the normal rate.
    pub cycles: u64,
    unix_base: i64,
    unix_base_cycles: u64,
}

impl Bus {
    pub fn new(cart: Cartridge, cgb: bool) -> Self {
        Self {
            cart,
            video: Video::new(cgb),
            audio: Audio::new(),
            timer: Timer::new(),
            joypad: Joypad::new(),
            serial: Serial::new(),
            wram: Box::new([0; 0x8000]),
            hram: [0; 0x7F],
            wram_bank: 1,
            ie: 0,
            if_: 0xE1,
            cgb,
            double_speed: false,
            speed_armed: false,
            oam_dma: None,
            hdma_src: 0,
            hdma_dst: 0,
            hdma_blocks: 0xFF,
            hdma_active: false,
            cycles: 0,
            unix_base: 0,
            unix_base_cycles: 0,
        }
    }

    pub fn is_color(&self) -> bool {
        self.cgb
    }

    pub fn interrupts_pending(&self) -> u8 {
        self.ie & self.if_ & 0x1F
    }

    /// Wall-clock time as the game sees it: what was last set, plus emulated time since.
    pub fn unix_now(&self) -> i64 {
        self.unix_base + ((self.cycles - self.unix_base_cycles) / u64::from(CLOCK_HZ)) as i64
    }

    pub fn set_time(&mut self, unix_seconds: i64) {
        self.unix_base = unix_seconds;
        self.unix_base_cycles = self.cycles;
    }

    /// STOP with a speed switch armed: switches and reports that it did.
    pub fn switch_speed(&mut self) -> bool {
        if !self.cgb || !self.speed_armed {
            return false;
        }
        self.speed_armed = false;
        self.double_speed = !self.double_speed;
        self.timer.reset_div();
        true
    }

    /// One M-cycle.
    pub fn tick(&mut self) {
        let t = if self.double_speed { 2 } else { 4 };
        self.cycles += u64::from(t);
        self.timer.step(&mut self.if_);
        self.video.step(t, &mut self.if_);
        if self.video.take_hblank_started() && self.hdma_active {
            self.hdma_block();
        }
        self.audio.step(t);
        self.serial.step(t, self.cgb, &mut self.if_);
        if let Some((source, index)) = self.oam_dma {
            let value = self.peek(source + u16::from(index));
            self.video.write_oam(index, value);
            self.oam_dma = if index == 0x9F { None } else { Some((source, index + 1)) };
        }
    }

    pub fn read(&mut self, addr: u16) -> u8 {
        self.tick();
        self.peek(addr)
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        self.tick();
        self.poke(addr, value);
    }

    /// A read with no time passing.
    pub fn peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF | 0xA000..=0xBFFF => self.cart.read(addr),
            0x8000..=0x9FFF => self.video.read_vram(addr),
            0xC000..=0xCFFF | 0xE000..=0xEFFF => self.wram[usize::from(addr & 0x0FFF)],
            0xD000..=0xDFFF | 0xF000..=0xFDFF => {
                self.wram[usize::from(self.wram_bank) * 0x1000 + usize::from(addr & 0x0FFF)]
            }
            0xFE00..=0xFE9F => self.video.read_oam((addr - 0xFE00) as u8),
            0xFEA0..=0xFEFF => 0xFF,
            0xFF00..=0xFF7F => self.read_io(addr),
            0xFF80..=0xFFFE => self.hram[usize::from(addr - 0xFF80)],
            0xFFFF => self.ie,
        }
    }

    fn poke(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x7FFF | 0xA000..=0xBFFF => self.cart.write(addr, value, self.cycles),
            0x8000..=0x9FFF => self.video.write_vram(addr, value),
            0xC000..=0xCFFF | 0xE000..=0xEFFF => self.wram[usize::from(addr & 0x0FFF)] = value,
            0xD000..=0xDFFF | 0xF000..=0xFDFF => {
                self.wram[usize::from(self.wram_bank) * 0x1000 + usize::from(addr & 0x0FFF)] =
                    value;
            }
            0xFE00..=0xFE9F => self.video.write_oam((addr - 0xFE00) as u8, value),
            0xFEA0..=0xFEFF => {}
            0xFF00..=0xFF7F => self.write_io(addr, value),
            0xFF80..=0xFFFE => self.hram[usize::from(addr - 0xFF80)] = value,
            0xFFFF => self.ie = value,
        }
    }

    fn read_io(&self, addr: u16) -> u8 {
        match addr {
            0xFF00 => self.joypad.read(),
            0xFF01 | 0xFF02 => self.serial.read(addr),
            0xFF04..=0xFF07 => self.timer.read(addr),
            0xFF0F => self.if_ | 0xE0,
            0xFF10..=0xFF3F => self.audio.read(addr),
            0xFF40..=0xFF45 | 0xFF47..=0xFF4B | 0xFF4F | 0xFF68..=0xFF6C => {
                self.video.read_io(addr)
            }
            0xFF46 => 0xFF,
            0xFF4D if self.cgb => {
                (u8::from(self.double_speed) << 7) | 0x7E | u8::from(self.speed_armed)
            }
            0xFF55 if self.cgb => (u8::from(!self.hdma_active) << 7) | (self.hdma_blocks & 0x7F),
            0xFF70 if self.cgb => self.wram_bank | 0xF8,
            _ => 0xFF,
        }
    }

    fn write_io(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF00 => self.joypad.write(value),
            0xFF01 | 0xFF02 => self.serial.write(addr, value),
            0xFF04..=0xFF07 => self.timer.write(addr, value),
            0xFF0F => self.if_ = value & 0x1F,
            0xFF10..=0xFF3F => self.audio.write(addr, value),
            0xFF46 => self.oam_dma = Some((u16::from(value) << 8, 0)),
            0xFF40..=0xFF45 | 0xFF47..=0xFF4B | 0xFF4F | 0xFF68..=0xFF6C => {
                self.video.write_io(addr, value)
            }
            0xFF4D if self.cgb => self.speed_armed = value & 1 != 0,
            0xFF51 if self.cgb => {
                self.hdma_src = (self.hdma_src & 0x00FF) | (u16::from(value) << 8)
            }
            0xFF52 if self.cgb => {
                self.hdma_src = (self.hdma_src & 0xFF00) | u16::from(value & 0xF0)
            }
            0xFF53 if self.cgb => {
                self.hdma_dst = (self.hdma_dst & 0x00FF) | (u16::from(value & 0x1F) << 8)
            }
            0xFF54 if self.cgb => {
                self.hdma_dst = (self.hdma_dst & 0xFF00) | u16::from(value & 0xF0)
            }
            0xFF55 if self.cgb => self.start_hdma(value),
            0xFF70 if self.cgb => self.wram_bank = (value & 7).max(1),
            _ => {}
        }
    }

    /// FF55: a general-purpose transfer runs at once; an HBlank one goes a block
    /// per HBlank. Writing with bit 7 clear while one runs stops it.
    fn start_hdma(&mut self, value: u8) {
        if self.hdma_active && value & 0x80 == 0 {
            self.hdma_active = false;
            return;
        }
        self.hdma_blocks = value & 0x7F;
        if value & 0x80 != 0 {
            self.hdma_active = true;
        } else {
            while self.hdma_blocks != 0xFF {
                self.hdma_block();
            }
        }
    }

    /// Copies one 16-byte block into VRAM, taking the time the hardware does.
    fn hdma_block(&mut self) {
        for _ in 0..16 {
            let value = self.peek(self.hdma_src);
            self.video.write_vram(0x8000 | (self.hdma_dst & 0x1FFF), value);
            self.hdma_src = self.hdma_src.wrapping_add(1);
            self.hdma_dst = self.hdma_dst.wrapping_add(1);
        }
        for _ in 0..8 {
            self.tick_without_hdma();
        }
        self.hdma_blocks = self.hdma_blocks.wrapping_sub(1);
        if self.hdma_blocks == 0xFF {
            self.hdma_active = false;
        }
    }

    /// `tick` minus the HBlank DMA check, for the cycles a DMA block itself takes.
    fn tick_without_hdma(&mut self) {
        let active = self.hdma_active;
        self.hdma_active = false;
        self.tick();
        self.hdma_active = active;
    }
}
