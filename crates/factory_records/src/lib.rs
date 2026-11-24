#![deny(missing_docs)]
//! The software factory's records: each Run of a Flow, the Steps it took and
//! the Gates they asked people, kept in MacroDB (`factory_run`,
//! `factory_step`, `factory_gate`), and the merges Runs made with the Main
//! Holds on the branches they merge to (`factory_merge`,
//! `factory_main_hold`).
//!
//! `factory_engine` writes them while a Run goes and reads them for its API.
//! This crate owns those tables.
//!
//! # Architecture
//!
//! - **domain**: the record models and the repository port.
//! - **outbound**: the Postgres repository.

pub mod domain;
pub mod outbound;

// Fork CI on hosted runners: a change here selects the factory packages.
