use core::fmt;

/// A deterministic application reducer. All external observations belong in
/// event/state values. Interior mutability, clocks, RNG and I/O break replay.
/// Rust's shared borrow cannot enforce this purity contract.
pub trait Model {
    /// Complete domain state, copied by value at commit boundaries.
    type State: Copy + Eq;
    /// Immutable domain input retained while recovering.
    type Event: Copy + Eq;
    /// Typed application fault.
    type Fault: Copy + Eq;
    /// Intent for the outer driver to execute after a successful commit.
    type Effect: Copy + Eq;

    /// Propose a transition. A fault leaves the domain state unchanged.
    /// On retry, the same event is evaluated at the new logical tick.
    fn transition(
        &self,
        state: Self::State,
        event: Self::Event,
        tick: u64,
    ) -> Result<Transition<Self::State, Self::Effect>, Self::Fault>;
}

/// A proposed state and optional driver effect, with no I/O inside the core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition<S, X> {
    /// State to commit.
    pub state: S,
    /// Effect intent to return once for this accepted command.
    pub effect: Option<X>,
}

impl<S, X> Transition<S, X> {
    /// Propose a transition without an effect.
    pub const fn silent(state: S) -> Self {
        Self {
            state,
            effect: None,
        }
    }
    /// Propose a transition with one effect intent.
    pub const fn with_effect(state: S, effect: X) -> Self {
        Self {
            state,
            effect: Some(effect),
        }
    }
}

/// An externally timestamped input. Ticks are nondecreasing, not wall time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<E> {
    /// Submit a fresh domain event. Rejected while recovery is pending.
    Event {
        /// Logical time assigned by the driver.
        tick: u64,
        /// Immutable domain observation.
        event: E,
    },
    /// Reevaluate the retained event once, at or after its retry deadline.
    Retry {
        /// Logical time assigned by the driver.
        tick: u64,
    },
}

impl<E> Command<E> {
    /// Construct a new event command.
    pub const fn event(tick: u64, event: E) -> Self {
        Self::Event { tick, event }
    }
    /// Construct a retry command.
    pub const fn retry(tick: u64) -> Self {
        Self::Retry { tick }
    }
    /// Read the command's logical time.
    pub const fn tick(&self) -> u64 {
        match self {
            Self::Event { tick, .. } | Self::Retry { tick } => *tick,
        }
    }
}

/// Recovery bounds. `max_retries` excludes the first evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    max_retries: u16,
    backoff_ticks: u64,
}

impl Policy {
    /// Require a positive delay, preventing same-tick retry spin loops.
    pub const fn new(max_retries: u16, backoff_ticks: u64) -> Result<Self, ConfigError> {
        if backoff_ticks == 0 {
            return Err(ConfigError::ZeroBackoff);
        }
        Ok(Self {
            max_retries,
            backoff_ticks,
        })
    }
    /// Maximum additional reducer evaluations after the original failure.
    pub const fn max_retries(self) -> u16 {
        self.max_retries
    }
    /// Fixed logical delay between failed evaluations.
    pub const fn backoff_ticks(self) -> u64 {
        self.backoff_ticks
    }
}

/// Why the fallback was committed and further commands were disabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HaltReason {
    /// The configured number of retry evaluations has been used.
    RetryBudgetExhausted,
    /// Scheduling a future retry would wrap `u64` logical time.
    DeadlineOverflow,
}

/// Engine lifecycle, including all information needed for exact replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status<E, F> {
    /// Fresh events may be submitted.
    Active,
    /// Domain state is unchanged; a failed event awaits reevaluation.
    Recovering {
        /// Original failed event.
        event: E,
        /// Most recent application fault.
        fault: F,
        /// Retry evaluations already performed (original attempt excluded).
        attempts: u16,
        /// Earliest accepted logical tick for the next retry.
        retry_at: u64,
    },
    /// Terminal fallback; construct a new engine to begin another session.
    Halted {
        /// Most recent application fault.
        fault: F,
        /// Number of retries actually performed.
        attempts: u16,
        /// Reason for terminal fallback.
        reason: HaltReason,
    },
}

/// Complete engine state at a command boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot<S, E, F> {
    /// Domain state.
    pub state: S,
    /// Last accepted logical tick; rejected commands never advance it.
    pub tick: u64,
    /// Recovery lifecycle.
    pub status: Status<E, F>,
}

/// A model's complete snapshot with its associated types substituted.
pub type ModelSnapshot<M> = Snapshot<<M as Model>::State, <M as Model>::Event, <M as Model>::Fault>;

/// A committed decision, including typed errors and returned effect intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome<F, X> {
    /// Transition accepted; the driver may execute its effect intent.
    Applied {
        /// Optional driver intent.
        effect: Option<X>,
    },
    /// Failure was recorded and a later retry was scheduled.
    Scheduled {
        /// Application fault.
        fault: F,
        /// Retries already performed.
        attempts: u16,
        /// Next eligible logical tick.
        retry_at: u64,
    },
    /// Fallback was committed; no further commands will be accepted.
    Halted {
        /// Application fault.
        fault: F,
        /// Retries already performed.
        attempts: u16,
        /// Terminal reason.
        reason: HaltReason,
        /// Optional fallback intent (e.g. stop actuator).
        effect: Option<X>,
    },
}

/// Invalid construction parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// Retry delay must be positive.
    ZeroBackoff,
    /// At least one journal record is required.
    ZeroCapacity,
}
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl core::error::Error for ConfigError {}

/// A rejected command. Every variant preserves the complete engine and log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchError {
    /// Input logical tick preceded the last accepted tick.
    TimeWentBackwards {
        /// Last accepted tick.
        current: u64,
        /// Rejected input tick.
        supplied: u64,
    },
    /// Journal is full; no reducer call or state mutation occurred.
    TraceFull,
    /// Session sequence identifiers can no longer be incremented without wrap.
    SequenceExhausted,
    /// A fresh event cannot replace a pending failed event.
    RecoveryPending,
    /// No event is waiting for retry.
    NotRecovering,
    /// The retry deadline has not been reached.
    RetryTooEarly {
        /// Earliest allowed retry tick.
        retry_at: u64,
    },
    /// Fallback already halted the engine.
    Halted,
}
impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl core::error::Error for DispatchError {}
