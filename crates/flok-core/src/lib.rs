//! # flok-core
//!
//! Core library for flok. Contains the session engine, LLM provider
//! implementations, tool system, agent definitions, and configuration.
//!
//! This crate is the heart of flok — the binary crate and TUI crate
//! depend on it, but it knows nothing about rendering or CLI parsing.

pub mod agent;
pub mod bus;
pub mod compress;
pub mod config;
pub mod provider;
pub mod review;
pub mod session;
pub mod snapshot;
pub mod team;
pub mod token;
pub mod tool;
pub mod worktree;
