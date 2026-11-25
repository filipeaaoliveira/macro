#![deny(missing_docs)]
//! CRAP scores of the Rust functions a change touched.
//!
//! CRAP (change risk anti-patterns) scores a function by its cyclomatic
//! complexity and how much of it its package's tests run. The check keeps the
//! functions whose lines meet the change's diff, pairs each with the same
//! function at the base commit, and fails only on one that is new or more
//! complex than at the base and scores above the threshold. Other touched
//! functions above it are reported without failing: the change did not make
//! them riskier.
//!
//! [`changed`] says which files to score, [`diff`] which of their lines
//! changed, [`packages`] which packages hold them, [`spans`] where each
//! function ends, [`nextest`] which tests failed, and [`rows`] joins
//! `cargo crap`'s scores into the rows the check prints.

pub mod changed;
pub mod diff;
pub mod nextest;
pub mod packages;
pub mod rows;
pub mod spans;
