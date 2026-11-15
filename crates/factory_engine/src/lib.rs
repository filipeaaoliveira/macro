#![deny(missing_docs)]
//! The software factory's engine: runs a Flow as a durable Temporal workflow.
//!
//! A Flow (JSON) lists entries; each runs a Step, and each Step's outcome says
//! where the Run goes next. A Run is created over HTTP, recorded in MacroDB
//! (`factory_records`), and walked by one workflow that runs each Step as an
//! activity.
//!
//! # Architecture
//!
//! - **domain**: the Flow model, the interpreter, and the Run use cases.
//! - **inbound**: the HTTP API, and the Temporal workflow and activities.
//! - **outbound**: the Temporal scheduler and the built-in Steps.
//! - **temporal**: the names and types both Temporal adapters agree on.

pub mod domain;
pub mod inbound;
pub mod outbound;
pub mod temporal;
