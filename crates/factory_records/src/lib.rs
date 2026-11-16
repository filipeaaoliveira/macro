#![deny(missing_docs)]
//! The software factory's records: each Run of a Flow, the Steps it took and
//! the Gates they asked people, kept in MacroDB (`factory_run`,
//! `factory_step`, `factory_gate`).
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
