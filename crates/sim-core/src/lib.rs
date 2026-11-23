#![deny(clippy::float_arithmetic)]

//! Reusable deterministic simulation building blocks.

pub mod behavior;
pub mod entity;
pub mod intent;
pub mod rng;
pub mod schedule;
pub mod space;
pub mod state;
pub mod topology;

#[cfg(test)]
mod tests;
