#![doc = include_str!("../README.md")]

mod client;
mod error;
pub mod time_portal;
mod wire;

pub use chrono;
pub use client::{Client, ClientBuilder, MAX_WAIT};
pub use error::Error;
pub use serde_json;
