#![deny(missing_docs)]
//! The software factory's records: each Run of a Flow and the Steps it took,
//! kept in MacroDB (`factory_run`, `factory_step`).
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
