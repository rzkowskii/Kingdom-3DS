//! Pure game logic. Must not depend on any platform crate.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod animation;
#[cfg(feature = "save")]
pub mod assetfile;
pub mod autopilot;
pub mod bag;
pub mod boat;
pub mod build;
pub mod coins;
pub mod controls;
pub mod data;
pub mod director;
pub mod dog;
pub mod farm;
pub mod flight;
pub mod grass;
pub mod greed;
pub mod hermits;
pub mod kingdom;
pub mod level;
pub mod lighting;
pub mod merchant;
pub mod monarch;
pub mod mounts;
pub mod payable;
pub mod payment;
pub mod prof;
pub mod progress;
pub mod recoil;
pub mod rng;
#[cfg(feature = "save")]
pub mod save;
pub mod shrines;
pub mod siege;
pub mod state;
pub mod teleport;
pub mod time;
pub mod treasure;
pub mod tutorial;
pub mod units;
pub mod wildlife;
