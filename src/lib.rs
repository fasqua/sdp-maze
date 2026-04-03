//! SDP Maze - Stealth Diffusion Protocol with Maze Pattern
//!
//! Privacy-enhanced transfer protocol using maze topology
//! instead of linear 3-hop pattern.

pub mod core;
pub mod relay;
pub mod error;
pub mod config;

// Re-export commonly used types
pub use config::Config;
pub use core::{
    MetaAddress, StealthAddress, StealthKeys,
    create_stealth_address, generate_stealth_keys,
    lamports_to_sol, sol_to_lamports,
};
pub use error::{MazeError, Result};
