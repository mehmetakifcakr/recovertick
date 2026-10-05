//! Deterministic, allocation-free state transitions with bounded recovery.
//!
//! The application supplies a pure [`Model`]. The engine commits one decision
//! per accepted command and records it before returning its effect intent.
//! Effects are values; the engine never performs hardware I/O.
//!
//! ```
//! use recovertick::{demo::{Motor, MotorEvent, MotorState}, Command, Engine,
//!     Policy, Transition};
//! let policy = Policy::new(2, 3)?;
//! let mut engine = Engine::<Motor, 16>::new(Motor, MotorState::Idle, policy,
//!     Transition::silent(MotorState::Stopped))?;
//! let decision = engine.dispatch(Command::event(1, MotorEvent::Start))?;
//! assert_eq!(decision.after.state, MotorState::Running);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
#![no_std]
#![forbid(unsafe_code)]

pub mod demo;
mod engine;
mod journal;
mod model;
mod replay;

pub use engine::Engine;
pub use journal::{Journal, ModelRecord, Record};
pub use model::{
    Command, ConfigError, DispatchError, HaltReason, Model, ModelSnapshot, Outcome, Policy,
    Snapshot, Status, Transition,
};
pub use replay::{verify_replay, ReplayError};
