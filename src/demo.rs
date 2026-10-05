//! Small motor decision model used by the runnable examples and benchmarks.
//! This is a simulated controller, not firmware or a hardware safety guarantee.
use crate::{Model, Transition};

/// Simulated motor domain state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotorState {
    /// Awaiting a command.
    Idle,
    /// Start decision committed.
    Running,
    /// Stop/fallback decision committed.
    Stopped,
}
/// Inputs with a deterministic fault injection window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotorEvent {
    /// Start immediately.
    Start,
    /// Start only at or after this logical tick.
    StartWhen {
        /// First eligible tick.
        ready_at: u64,
    },
    /// Stop immediately.
    Stop,
}
/// External driver intent; never executed by the core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotorEffect {
    /// Drive the motor.
    Drive,
    /// Disable drive.
    Disable,
}
/// Faults returned by the pure motor reducer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotorFault {
    /// Fault injection window is still active.
    Unavailable,
    /// This session's stopped state is terminal.
    AlreadyStopped,
}
/// Stateless, pure example reducer.
#[derive(Clone, Copy, Debug, Default)]
pub struct Motor;
impl Model for Motor {
    type State = MotorState;
    type Event = MotorEvent;
    type Fault = MotorFault;
    type Effect = MotorEffect;
    fn transition(
        &self,
        state: MotorState,
        event: MotorEvent,
        tick: u64,
    ) -> Result<Transition<MotorState, MotorEffect>, MotorFault> {
        if state == MotorState::Stopped && event != MotorEvent::Stop {
            return Err(MotorFault::AlreadyStopped);
        }
        match event {
            MotorEvent::Start => Ok(Transition::with_effect(
                MotorState::Running,
                MotorEffect::Drive,
            )),
            MotorEvent::StartWhen { ready_at } if tick < ready_at => Err(MotorFault::Unavailable),
            MotorEvent::StartWhen { .. } => Ok(Transition::with_effect(
                MotorState::Running,
                MotorEffect::Drive,
            )),
            MotorEvent::Stop => Ok(Transition::with_effect(
                MotorState::Stopped,
                MotorEffect::Disable,
            )),
        }
    }
}
