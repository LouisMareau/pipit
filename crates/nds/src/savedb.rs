//! Which save chip some games have. A game's first write to its chip
//! usually tells the chip's kind, but not always: a short write looks like
//! a 4 Kbit EEPROM's, the 1 Mbit EEPROM looks like Flash, the FRAM like a
//! 512 Kbit EEPROM, and a Flash chip's size only shows once the game
//! reaches its far end. Games known here get their chip from the start.
//!
//! Keyed by the game code without its region letter, since the chip is the
//! same in every region. The list holds the games whose chip is public
//! knowledge; the settings let the user name the chip of any other.

use crate::backup::Kind;

/// The chip and its size in bytes, for a game code (`ADAE`, `IPKF`…).
pub fn lookup(game_code: &str) -> Option<(Kind, usize)> {
    let key = game_code.get(..3)?;
    let i = ENTRIES.binary_search_by(|(code, _, _)| (*code).cmp(key)).ok()?;
    let (_, kind, size) = ENTRIES[i];
    Some((kind, size))
}

const K: usize = 1024;

/// Sorted by code.
const ENTRIES: &[(&str, Kind, usize)] = &[
    ("ADA", Kind::Flash, 512 * K),  // Pokémon Diamond
    ("ADM", Kind::Flash, 256 * K),  // Animal Crossing: Wild World
    ("AMC", Kind::Flash, 256 * K),  // Mario Kart DS
    ("APA", Kind::Flash, 512 * K),  // Pokémon Pearl
    ("ASM", Kind::Eeprom8K, 8 * K), // Super Mario 64 DS
    ("CPU", Kind::Flash, 512 * K),  // Pokémon Platinum
    ("IPG", Kind::Flash, 512 * K),  // Pokémon SoulSilver
    ("IPK", Kind::Flash, 512 * K),  // Pokémon HeartGold
    ("IRA", Kind::Flash, 512 * K),  // Pokémon White
    ("IRB", Kind::Flash, 512 * K),  // Pokémon Black
    ("IRD", Kind::Flash, 512 * K),  // Pokémon White 2
    ("IRE", Kind::Flash, 512 * K),  // Pokémon Black 2
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_sorted_so_the_search_works() {
        assert!(ENTRIES.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(lookup("IPKF"), Some((Kind::Flash, 512 * K)));
        assert_eq!(lookup("ZZZZ"), None);
        assert_eq!(lookup("AB"), None);
    }
}
