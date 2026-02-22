//! Shared utilities for packet radio platforms that have `std` available.
//!
//! This crate provides:
//! - APRS-IS client (TCP connection to APRS internet servers)
//! - IGate logic (bridging RF ↔ APRS-IS)
//! - Configuration file parsing

pub mod igate;
pub mod config;
