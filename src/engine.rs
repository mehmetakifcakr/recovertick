use crate::journal::ModelRecord;
use crate::{
    Command, ConfigError, DispatchError, HaltReason, Journal, Model, ModelSnapshot, Outcome,
    Policy, Record, Snapshot, Status, Transition,
};
use core::future::Future;

type FailureDecision<M> = (
    ModelSnapshot<M>,
    Outcome<<M as Model>::Fault, <M as Model>::Effect>,
);

/// A single-owner engine. No allocations, runtime, lock, clock or background task.
pub struct Engine<M: Model, const N: usize> {
    model: M,
    policy: Policy,
    fallback: Transition<M::State, M::Effect>,
    snapshot: Snapshot<M::State, M::Event, M::Fault>,
    journal: Journal<ModelRecord<M>, N>,
    next_sequence: u64,
}

impl<M: Model, const N: usize> Engine<M, N> {
    /// Start an active session at logical tick zero. Fallback is a domain choice;
    /// its effect is an intent, not a guarantee that an actuator has stopped.
    pub fn new(
        model: M,
        initial: M::State,
        policy: Policy,
        fallback: Transition<M::State, M::Effect>,
    ) -> Result<Self, ConfigError> {
        if N == 0 {
            return Err(ConfigError::ZeroCapacity);
        }
        Ok(Self {
            model,
            policy,
            fallback,
            snapshot: Snapshot {
                state: initial,
                tick: 0,
                status: Status::Active,
            },
            journal: Journal::new(),
            next_sequence: 0,
        })
    }

    /// Copy the complete current snapshot.
    pub fn snapshot(&self) -> Snapshot<M::State, M::Event, M::Fault> {
        self.snapshot
    }
    /// Inspect the immutable accepted-command journal.
    pub fn journal(&self) -> &Journal<ModelRecord<M>, N> {
        &self.journal
    }
    /// Transfer the current segment to the caller and resume recording into an
    /// empty segment. Archive the returned records before dropping them if full
    /// session replay is required. State, tick and global sequence are preserved.
    /// Ownership transfer is not persistence or an archive acknowledgement.
    pub fn take_journal(&mut self) -> Journal<ModelRecord<M>, N> {
        core::mem::replace(&mut self.journal, Journal::new())
    }
    /// Read the configured recovery policy.
    pub const fn policy(&self) -> Policy {
        self.policy
    }

    pub(crate) const fn next_sequence_for_replay(&self) -> u64 {
        self.next_sequence
    }

    pub(crate) fn discard_verified_record(&mut self) {
        self.journal.discard_verified();
    }

    /// Validate, evaluate and commit one command. Rejections mutate nothing.
    /// Application faults are accepted decisions (Scheduled/Halted), not
    /// dispatch errors. The reducer must not panic or perform side effects.
    pub fn dispatch(
        &mut self,
        command: Command<M::Event>,
    ) -> Result<ModelRecord<M>, DispatchError> {
        let before = self.snapshot;
        let tick = command.tick();
        if tick < before.tick {
            return Err(DispatchError::TimeWentBackwards {
                current: before.tick,
                supplied: tick,
            });
        }
        let (event, attempts) = match (before.status, command) {
            (Status::Halted { .. }, _) => return Err(DispatchError::Halted),
            (Status::Recovering { .. }, Command::Event { .. }) => {
                return Err(DispatchError::RecoveryPending)
            }
            (Status::Active, Command::Retry { .. }) => return Err(DispatchError::NotRecovering),
            (Status::Active, Command::Event { event, .. }) => (event, 0),
            (
                Status::Recovering {
                    event,
                    attempts,
                    retry_at,
                    ..
                },
                Command::Retry { .. },
            ) => {
                if tick < retry_at {
                    return Err(DispatchError::RetryTooEarly { retry_at });
                }
                // Every scheduled retry has attempts < max_retries <= u16::MAX.
                (event, attempts + 1)
            }
        };
        if self.journal.is_full() {
            return Err(DispatchError::TraceFull);
        }
        let next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(DispatchError::SequenceExhausted)?;

        let (after, outcome) = match self.model.transition(before.state, event, tick) {
            Ok(proposed) => (
                Snapshot {
                    state: proposed.state,
                    tick,
                    status: Status::Active,
                },
                Outcome::Applied {
                    effect: proposed.effect,
                },
            ),
            Err(fault) => self.failure(event, fault, attempts, tick),
        };
        let record = Record {
            sequence: self.next_sequence,
            command,
            before,
            after,
            outcome,
        };
        // All validation and user code have completed. Append and commit have
        // no await/cancellation boundary and require one exclusive engine borrow.
        self.journal.push(record);
        self.snapshot = after;
        self.next_sequence = next_sequence;
        Ok(record)
    }

    /// Await one externally produced command, then commit synchronously.
    /// Dropping while input is Pending leaves this engine unchanged. The input
    /// source itself must define whether cancellation consumes a queued event.
    /// This future performs no hidden timer, spawning or busy loop.
    pub async fn dispatch_async<F>(&mut self, input: F) -> Result<ModelRecord<M>, DispatchError>
    where
        F: Future<Output = Command<M::Event>>,
    {
        self.dispatch(input.await)
    }

    fn failure(
        &self,
        event: M::Event,
        fault: M::Fault,
        attempts: u16,
        tick: u64,
    ) -> FailureDecision<M> {
        let reason = if attempts >= self.policy.max_retries() {
            HaltReason::RetryBudgetExhausted
        } else if let Some(retry_at) = tick.checked_add(self.policy.backoff_ticks()) {
            return (
                Snapshot {
                    state: self.snapshot.state,
                    tick,
                    status: Status::Recovering {
                        event,
                        fault,
                        attempts,
                        retry_at,
                    },
                },
                Outcome::Scheduled {
                    fault,
                    attempts,
                    retry_at,
                },
            );
        } else {
            HaltReason::DeadlineOverflow
        };
        (
            Snapshot {
                state: self.fallback.state,
                tick,
                status: Status::Halted {
                    fault,
                    attempts,
                    reason,
                },
            },
            Outcome::Halted {
                fault,
                attempts,
                reason,
                effect: self.fallback.effect,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::{Motor, MotorEvent, MotorState};

    #[test]
    fn invalid_configuration_is_rejected() {
        assert_eq!(Policy::new(1, 0), Err(ConfigError::ZeroBackoff));
        assert_eq!(
            Engine::<Motor, 0>::new(
                Motor,
                MotorState::Idle,
                Policy::new(1, 1).unwrap(),
                Transition::silent(MotorState::Stopped)
            )
            .err(),
            Some(ConfigError::ZeroCapacity)
        );
    }

    #[test]
    fn session_sequence_never_wraps() {
        let mut engine = Engine::<Motor, 1>::new(
            Motor,
            MotorState::Idle,
            Policy::new(1, 1).unwrap(),
            Transition::silent(MotorState::Stopped),
        )
        .unwrap();
        engine.next_sequence = u64::MAX;
        let before = engine.snapshot();
        assert_eq!(
            engine.dispatch(Command::event(1, MotorEvent::Start)),
            Err(DispatchError::SequenceExhausted)
        );
        assert_eq!(engine.snapshot(), before);
        assert!(engine.journal().is_empty());
        assert_eq!(engine.next_sequence, u64::MAX);
    }
}
