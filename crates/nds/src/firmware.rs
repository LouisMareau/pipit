//! The firmware: a 256 KB serial flash on the ARM7's SPI bus holding the
//! console's settings. Without a user's image Pipit makes one with a header,
//! no WiFi access points, and user settings in English with a touch screen
//! calibrated as ADC = 16 × pixel; the user settings also go to main memory
//! at boot, as the BIOS would copy them.

pub const SIZE: usize = 0x4_0000;
/// Where the two user settings slots are in a 256 KB image.
pub const USER_SETTINGS: usize = 0x3_FE00;

/// CRC-16 as the firmware and the BIOS compute it (polynomial A001h, reflected).
pub fn crc16(init: u16, bytes: &[u8]) -> u16 {
    let mut crc = init;
    for &b in bytes {
        crc ^= u16::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

/// The user settings block: 70h bytes of settings, an update counter and
/// their CRC. Version 5, English, nickname "PIPIT", a 1 January birthday.
pub fn default_user_settings() -> [u8; 0x74] {
    let mut s = [0u8; 0x74];
    s[0..2].copy_from_slice(&5u16.to_le_bytes());
    s[3] = 1;
    s[4] = 1;
    for (i, c) in "PIPIT".encode_utf16().enumerate() {
        s[6 + i * 2..8 + i * 2].copy_from_slice(&c.to_le_bytes());
    }
    s[0x1A..0x1C].copy_from_slice(&5u16.to_le_bytes());
    // Touch calibration: ADC (100h, 100h) is pixel (10h, 10h), (F00h, B00h) is (F0h, B0h).
    let calibration: [u8; 12] =
        [0x00, 0x01, 0x00, 0x01, 0x10, 0x10, 0x00, 0x0F, 0x00, 0x0B, 0xF0, 0xB0];
    s[0x58..0x64].copy_from_slice(&calibration);
    s[0x64..0x66].copy_from_slice(&0x0021u16.to_le_bytes());
    let crc = crc16(0xFFFF, &s[..0x70]);
    s[0x72..0x74].copy_from_slice(&crc.to_le_bytes());
    s
}

/// The firmware image standing in for a console's own.
pub fn default_image() -> Vec<u8> {
    let mut f = vec![0xFF; SIZE];
    f[0x08..0x0C].copy_from_slice(b"MACP");
    // A DS Lite: the backlight register of its power management chip exists.
    f[0x1D] = 0x20;
    f[0x20..0x22].copy_from_slice(&((USER_SETTINGS / 8) as u16).to_le_bytes());
    // WiFi settings: their length and CRC, a locally administered MAC
    // address, channels 1–13, and three unused access points.
    f[0x2E..0x30].copy_from_slice(&0x138u16.to_le_bytes());
    f[0x36..0x3C].copy_from_slice(&[0x02, 0x50, 0x49, 0x50, 0x49, 0x54]);
    f[0x3C..0x3E].copy_from_slice(&0x3FFEu16.to_le_bytes());
    let crc = crc16(0, &f[0x2E..0x2E + 0x138]);
    f[0x2C..0x2E].copy_from_slice(&crc.to_le_bytes());
    for ap in 0..3 {
        let at = USER_SETTINGS - 0x400 + ap * 0x100;
        f[at..at + 0x100].fill(0);
        f[at + 0xE7] = 0xFF;
        let crc = crc16(0, &f[at..at + 0xFE]);
        f[at + 0xFE..at + 0x100].copy_from_slice(&crc.to_le_bytes());
    }
    let settings = default_user_settings();
    for slot in [USER_SETTINGS, USER_SETTINGS + 0x100] {
        f[slot..slot + 0x74].copy_from_slice(&settings);
    }
    f
}

/// The user settings an image holds: the newer of its two valid slots, or
/// the defaults when neither checks out.
pub fn user_settings_of(image: &[u8]) -> [u8; 0x74] {
    let offset =
        image.get(0x20..0x22).map_or(0, |b| usize::from(u16::from_le_bytes([b[0], b[1]])) * 8);
    let slot = |at: usize| -> Option<[u8; 0x74]> {
        let s: [u8; 0x74] = image.get(at..at + 0x74)?.try_into().ok()?;
        (crc16(0xFFFF, &s[..0x70]) == u16::from_le_bytes([s[0x72], s[0x73]])).then_some(s)
    };
    match (slot(offset), slot(offset + 0x100)) {
        (Some(a), Some(b)) => {
            let (ca, cb) = (a[0x70] & 0x7F, b[0x70] & 0x7F);
            if (cb.wrapping_sub(ca)) & 0x7F == 1 {
                b
            } else {
                a
            }
        }
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => default_user_settings(),
    }
}

/// The wireless hardware's MAC address, as the image stores it.
pub fn mac_of(image: &[u8]) -> [u8; 6] {
    image
        .get(0x36..0x3C)
        .and_then(|m| m.try_into().ok())
        .unwrap_or([0x02, 0x50, 0x49, 0x50, 0x49, 0x54])
}

/// Gives the image another MAC address (consoles within range of each other
/// must differ), keeping the wireless settings' CRC right.
pub fn set_mac(image: &mut [u8], mac: [u8; 6]) {
    if image.len() < 0x2E + 0x138 {
        return;
    }
    image[0x36..0x3C].copy_from_slice(&mac);
    let crc = crc16(0, &image[0x2E..0x2E + 0x138]);
    image[0x2C..0x2E].copy_from_slice(&crc.to_le_bytes());
}
