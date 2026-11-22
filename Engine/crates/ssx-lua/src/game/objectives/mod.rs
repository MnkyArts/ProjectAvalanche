pub mod binder;
pub mod node;
mod runner;
#[cfg(test)]
mod tests;
mod values;
mod world;

use std::sync::Arc;

pub use node::{Do, Node, Wait};
pub use runner::*;
pub use values::*;
pub use world::*;

pub const fn fnv1(s: &[u8]) -> u32 {
    fnv1_continue(0x811C_9DC5, s)
}

pub const fn fnv1_continue(mut h: u32, s: &[u8]) -> u32 {
    let mut i = 0;
    while i < s.len() {
        h = h.wrapping_mul(0x0100_0193) ^ s[i] as u32;
        i += 1;
    }
    h
}
