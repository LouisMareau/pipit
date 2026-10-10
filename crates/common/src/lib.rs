//! Shared between the emulation cores. Deliberately tiny: the cores are
//! independent, this only holds what would otherwise be copied.

pub mod snapshot;
