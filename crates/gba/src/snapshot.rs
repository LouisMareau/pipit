//! Save states: the whole machine serialized with `serde` + `bincode`.
//!
//! Every hardware struct derives `Serialize`/`Deserialize`; this module adds the
//! helpers for the large fixed-size buffers serde cannot derive on its own, and
//! the file format with its header.

use serde::de::{Error as _, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

const MAGIC: [u8; 4] = *b"PIPT";
/// Bumped whenever the serialized layout changes; older states are refused.
/// 2: the serial port gained state (link cable).
const VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct Header {
    magic: [u8; 4],
    version: u32,
    game_code: String,
}

/// Why a state could not be restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    NotAState,
    UnsupportedVersion(u32),
    /// The state was taken from a different game.
    GameMismatch {
        expected: String,
        found: String,
    },
    Corrupt,
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAState => write!(f, "not a Pipit save state"),
            Self::UnsupportedVersion(v) => write!(f, "save state version {v} is not supported"),
            Self::GameMismatch { expected, found } => {
                write!(f, "save state is for {found}, not {expected}")
            }
            Self::Corrupt => write!(f, "save state is corrupt"),
        }
    }
}

impl std::error::Error for StateError {}

pub fn write_into<T: Serialize>(game_code: &str, state: &T, out: &mut Vec<u8>) {
    let header = Header { magic: MAGIC, version: VERSION, game_code: game_code.to_string() };
    out.clear();
    bincode::serialize_into(&mut *out, &header).expect("header serializes");
    bincode::serialize_into(&mut *out, state).expect("state serializes");
}

pub fn read<T: for<'de> Deserialize<'de>>(game_code: &str, data: &[u8]) -> Result<T, StateError> {
    if data.len() < 4 || data[..4] != MAGIC {
        return Err(StateError::NotAState);
    }
    let mut cursor = std::io::Cursor::new(data);
    let header: Header = bincode::deserialize_from(&mut cursor).map_err(|_| StateError::Corrupt)?;
    if header.version != VERSION {
        return Err(StateError::UnsupportedVersion(header.version));
    }
    if header.game_code != game_code {
        return Err(StateError::GameMismatch {
            expected: game_code.to_string(),
            found: header.game_code,
        });
    }
    bincode::deserialize_from(&mut cursor).map_err(|_| StateError::Corrupt)
}

/// `Box<[u8; N]>` as a single byte string.
pub mod bytes_box {
    use super::*;

    pub fn serialize<S: Serializer, const N: usize>(v: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&v[..])
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        d: D,
    ) -> Result<Box<[u8; N]>, D::Error> {
        struct V<const N: usize>;
        impl<'de, const N: usize> Visitor<'de> for V<N> {
            type Value = Box<[u8; N]>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "{N} bytes")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                if v.len() != N {
                    return Err(E::invalid_length(v.len(), &self));
                }
                let mut out = vec![0u8; N].into_boxed_slice();
                out.copy_from_slice(v);
                out.try_into().map_err(|_| E::custom("size"))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
                self.visit_bytes(&v)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = vec![0u8; N].into_boxed_slice();
                for (i, slot) in out.iter_mut().enumerate() {
                    *slot =
                        seq.next_element()?.ok_or_else(|| A::Error::invalid_length(i, &self))?;
                }
                out.try_into().map_err(|_| A::Error::custom("size"))
            }
        }
        d.deserialize_bytes(V::<N>)
    }
}

/// `Box<[u32; N]>` as little-endian bytes (the framebuffer).
pub mod words_box {
    use super::*;

    pub fn serialize<S: Serializer, const N: usize>(v: &[u32; N], s: S) -> Result<S::Ok, S::Error> {
        let bytes: Vec<u8> = v.iter().flat_map(|w| w.to_le_bytes()).collect();
        s.serialize_bytes(&bytes)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        d: D,
    ) -> Result<Box<[u32; N]>, D::Error> {
        struct V<const N: usize>;
        impl<'de, const N: usize> Visitor<'de> for V<N> {
            type Value = Box<[u32; N]>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "{} bytes", N * 4)
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                if v.len() != N * 4 {
                    return Err(E::invalid_length(v.len(), &self));
                }
                let words: Vec<u32> =
                    v.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
                words.into_boxed_slice().try_into().map_err(|_| E::custom("size"))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
                self.visit_bytes(&v)
            }
        }
        d.deserialize_bytes(V::<N>)
    }
}

/// Fixed-size arrays longer than serde's 32-element derive limit.
pub mod array {
    use super::*;

    pub fn serialize<S: Serializer, T: Serialize, const N: usize>(
        v: &[T; N],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        v[..].serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>, const N: usize>(
        d: D,
    ) -> Result<[T; N], D::Error> {
        let v: Vec<T> = Vec::deserialize(d)?;
        v.try_into().map_err(|v: Vec<T>| D::Error::invalid_length(v.len(), &"fixed array"))
    }
}
