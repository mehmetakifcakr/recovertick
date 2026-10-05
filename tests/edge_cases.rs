//! Behavioral boundaries for atomic dispatch, recovery and segmented replay.

use std::cell::Cell;

use recovertick::demo::{Motor, MotorEffect, MotorEvent, MotorFault, MotorState};
use recovertick::{
    verify_replay, Command, ConfigError, DispatchError, Engine, HaltReason, Model, Outcome, Policy,
    ReplayError, Status, Transition,
};

fn motor<const N: usize>(max_retries: u16, backoff: u64) -> Engine<Motor, N> {
    Engine::new(
        Motor,
        MotorState::Idle,
        Policy::new(max_retries, backoff).expect("positive test backoff"),
        Transition::with_effect(MotorState::Stopped, MotorEffect::Disable),
    )
    .expect("nonzero test capacity")
}

#[derive(Clone, Copy)]
struct AlwaysFault;

impl Model for AlwaysFault {
    type State = MotorState;
    type Event = MotorEvent;
    type Fault = MotorFault;
    type Effect = MotorEffect;

    fn transition(
        &self,
        _state: MotorState,
        _event: MotorEvent,
        _tick: u64,
    ) -> Result<Transition<MotorState, MotorEffect>, MotorFault> {
        Err(MotorFault::Unavailable)
    }
}

#[derive(Clone, Copy)]
struct Counter;

impl Model for Counter {
    type State = u32;
    type Event = u32;
    type Fault = ();
    type Effect = ();

    fn transition(&self, state: u32, event: u32, _tick: u64) -> Result<Transition<u32, ()>, ()> {
        state.checked_add(event).map(Transition::silent).ok_or(())
    }
}

// Interior mutability is intentionally limited to test instrumentation. A
// production reducer must not observe a counter or any other hidden state.
struct CountingModel<'a> {
    calls: &'a Cell<usize>,
}

impl Model for CountingModel<'_> {
    type State = u32;
    type Event = u32;
    type Fault = ();
    type Effect = ();

    fn transition(&self, state: u32, event: u32, tick: u64) -> Result<Transition<u32, ()>, ()> {
        self.calls.set(self.calls.get() + 1);
        Counter.transition(state, event, tick)
    }
}

#[test]
fn invalid_configuration_is_rejected_before_a_session_exists() {
    assert_eq!(Policy::new(1, 0), Err(ConfigError::ZeroBackoff));
    assert!(matches!(
        Engine::<Motor, 0>::new(
            Motor,
            MotorState::Idle,
            Policy::new(1, 1).unwrap(),
            Transition::silent(MotorState::Stopped),
        ),
        Err(ConfigError::ZeroCapacity)
    ));
}

#[test]
fn backward_tick_preserves_snapshot_and_journal() {
    let mut engine = motor::<4>(2, 3);
    engine
        .dispatch(Command::event(7, MotorEvent::Start))
        .unwrap();
    let before = engine.snapshot();
    let records: Vec<_> = engine.journal().iter().copied().collect();

    assert_eq!(
        engine.dispatch(Command::event(6, MotorEvent::Stop)),
        Err(DispatchError::TimeWentBackwards {
            current: 7,
            supplied: 6,
        })
    );
    assert_eq!(engine.snapshot(), before);
    assert_eq!(
        engine.journal().iter().copied().collect::<Vec<_>>(),
        records
    );
}

#[test]
fn equal_ticks_are_accepted_in_input_order() {
    let mut engine = motor::<4>(2, 3);
    let start = engine
        .dispatch(Command::event(4, MotorEvent::Start))
        .unwrap();
    let stop = engine
        .dispatch(Command::event(4, MotorEvent::Stop))
        .unwrap();

    assert_eq!(start.sequence, 0);
    assert_eq!(stop.sequence, 1);
    assert_eq!(stop.before, start.after);
    assert_eq!(engine.snapshot().tick, 4);
    assert_eq!(engine.snapshot().state, MotorState::Stopped);
    assert_eq!(engine.journal().len(), 2);
}

#[test]
fn early_retry_preserves_tick_state_and_retained_fault() {
    let mut engine = motor::<4>(2, 3);
    engine
        .dispatch(Command::event(10, MotorEvent::StartWhen { ready_at: 100 }))
        .unwrap();
    let before = engine.snapshot();
    let records: Vec<_> = engine.journal().iter().copied().collect();

    assert_eq!(
        engine.dispatch(Command::retry(12)),
        Err(DispatchError::RetryTooEarly { retry_at: 13 })
    );
    assert_eq!(engine.snapshot(), before);
    assert_eq!(
        engine.journal().iter().copied().collect::<Vec<_>>(),
        records
    );
}

#[test]
fn new_event_cannot_replace_the_event_waiting_for_recovery() {
    let mut engine = motor::<4>(2, 3);
    engine
        .dispatch(Command::event(1, MotorEvent::StartWhen { ready_at: 4 }))
        .unwrap();
    let before = engine.snapshot();

    assert_eq!(
        engine.dispatch(Command::event(4, MotorEvent::Stop)),
        Err(DispatchError::RecoveryPending)
    );
    assert_eq!(engine.snapshot(), before);
    assert_eq!(engine.journal().len(), 1);
    assert_eq!(
        engine.dispatch(Command::retry(4)).unwrap().outcome,
        Outcome::Applied {
            effect: Some(MotorEffect::Drive),
        }
    );
}

#[test]
fn retry_without_a_retained_event_is_atomic() {
    let mut engine = motor::<4>(2, 3);
    let before = engine.snapshot();

    assert_eq!(
        engine.dispatch(Command::retry(1)),
        Err(DispatchError::NotRecovering)
    );
    assert_eq!(engine.snapshot(), before);
    assert!(engine.journal().is_empty());
}

#[test]
fn zero_retry_budget_emits_fallback_once_on_the_initial_failure() {
    let mut engine = motor::<4>(0, 3);
    let terminal = engine
        .dispatch(Command::event(1, MotorEvent::StartWhen { ready_at: 100 }))
        .unwrap();

    assert_eq!(
        terminal.outcome,
        Outcome::Halted {
            fault: MotorFault::Unavailable,
            attempts: 0,
            reason: HaltReason::RetryBudgetExhausted,
            effect: Some(MotorEffect::Disable),
        }
    );
    assert_eq!(terminal.after.state, MotorState::Stopped);
    let before = engine.snapshot();
    assert_eq!(
        engine.dispatch(Command::retry(2)),
        Err(DispatchError::Halted)
    );
    assert_eq!(
        engine.dispatch(Command::event(2, MotorEvent::Stop)),
        Err(DispatchError::Halted)
    );
    assert_eq!(engine.snapshot(), before);
    assert_eq!(engine.journal().len(), 1);
}

#[test]
fn retry_budget_counts_only_additional_reducer_evaluations() {
    let mut engine = motor::<4>(2, 3);
    let original = engine
        .dispatch(Command::event(1, MotorEvent::StartWhen { ready_at: 100 }))
        .unwrap();
    let first_retry = engine.dispatch(Command::retry(4)).unwrap();
    let last_retry = engine.dispatch(Command::retry(7)).unwrap();

    assert_eq!(
        original.outcome,
        Outcome::Scheduled {
            fault: MotorFault::Unavailable,
            attempts: 0,
            retry_at: 4,
        }
    );
    assert_eq!(
        first_retry.outcome,
        Outcome::Scheduled {
            fault: MotorFault::Unavailable,
            attempts: 1,
            retry_at: 7,
        }
    );
    assert_eq!(
        last_retry.outcome,
        Outcome::Halted {
            fault: MotorFault::Unavailable,
            attempts: 2,
            reason: HaltReason::RetryBudgetExhausted,
            effect: Some(MotorEffect::Disable),
        }
    );
    assert_eq!(engine.journal().len(), 3);
}

#[test]
fn a_retry_at_the_exact_deadline_can_recover() {
    let mut engine = motor::<4>(2, 3);
    let failed = engine
        .dispatch(Command::event(1, MotorEvent::StartWhen { ready_at: 4 }))
        .unwrap();
    let recovered = engine.dispatch(Command::retry(4)).unwrap();

    assert_eq!(failed.after.state, MotorState::Idle);
    assert_eq!(recovered.before, failed.after);
    assert_eq!(recovered.after.state, MotorState::Running);
    assert_eq!(recovered.after.status, Status::Active);
    assert_eq!(
        recovered.outcome,
        Outcome::Applied {
            effect: Some(MotorEffect::Drive),
        }
    );
}

#[test]
fn initial_deadline_overflow_commits_a_typed_terminal_fallback() {
    let mut engine = Engine::<AlwaysFault, 4>::new(
        AlwaysFault,
        MotorState::Idle,
        Policy::new(2, 3).unwrap(),
        Transition::with_effect(MotorState::Stopped, MotorEffect::Disable),
    )
    .unwrap();
    let record = engine
        .dispatch(Command::event(u64::MAX - 1, MotorEvent::Start))
        .unwrap();

    assert_eq!(
        record.outcome,
        Outcome::Halted {
            fault: MotorFault::Unavailable,
            attempts: 0,
            reason: HaltReason::DeadlineOverflow,
            effect: Some(MotorEffect::Disable),
        }
    );
    assert_eq!(record.after.tick, u64::MAX - 1);
    assert_eq!(record.after.state, MotorState::Stopped);
    assert_eq!(engine.journal().len(), 1);
}

#[test]
fn overflow_after_a_retry_preserves_the_number_of_attempts_performed() {
    let mut engine = Engine::<AlwaysFault, 4>::new(
        AlwaysFault,
        MotorState::Idle,
        Policy::new(2, 3).unwrap(),
        Transition::silent(MotorState::Stopped),
    )
    .unwrap();
    engine
        .dispatch(Command::event(u64::MAX - 4, MotorEvent::Start))
        .unwrap();
    let terminal = engine.dispatch(Command::retry(u64::MAX - 1)).unwrap();

    assert_eq!(
        terminal.outcome,
        Outcome::Halted {
            fault: MotorFault::Unavailable,
            attempts: 1,
            reason: HaltReason::DeadlineOverflow,
            effect: None,
        }
    );
    assert_eq!(engine.journal().len(), 2);
}

#[test]
fn full_journal_rejects_before_calling_the_reducer() {
    let calls = Cell::new(0);
    let mut engine = Engine::<_, 1>::new(
        CountingModel { calls: &calls },
        0,
        Policy::new(2, 3).unwrap(),
        Transition::silent(0),
    )
    .unwrap();
    engine.dispatch(Command::event(1, 1)).unwrap();
    let before = engine.snapshot();
    let records: Vec<_> = engine.journal().iter().copied().collect();

    assert_eq!(
        engine.dispatch(Command::event(2, 2)),
        Err(DispatchError::TraceFull)
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(engine.snapshot(), before);
    assert_eq!(
        engine.journal().iter().copied().collect::<Vec<_>>(),
        records
    );

    // Archiving restores capacity without changing state or session sequence.
    let archive = engine.take_journal();
    assert_eq!(archive.len(), 1);
    let accepted = engine.dispatch(Command::event(2, 2)).unwrap();
    assert_eq!(accepted.sequence, 1);
    assert_eq!(calls.get(), 2);
}

#[test]
fn taking_journal_preserves_state_and_global_sequence_across_segments() {
    let policy = Policy::new(2, 3).unwrap();
    let fallback = Transition::silent(0);
    let mut engine = Engine::<Counter, 2>::new(Counter, 0, policy, fallback).unwrap();
    engine.dispatch(Command::event(1, 1)).unwrap();
    engine.dispatch(Command::event(2, 2)).unwrap();
    let before = engine.snapshot();

    let first = engine.take_journal();
    assert_eq!(engine.snapshot(), before);
    assert!(engine.journal().is_empty());
    assert_eq!(first.get(0).unwrap().sequence, 0);
    assert_eq!(first.get(1).unwrap().sequence, 1);
    assert!(engine.take_journal().is_empty());

    let next = engine.dispatch(Command::event(3, 4)).unwrap();
    assert_eq!(next.sequence, 2);
    assert_eq!(next.before, before);
    let second = engine.take_journal();
    assert_eq!(second.get(0).unwrap().sequence, 2);
    let replayed = verify_replay::<_, _, 1>(
        Counter,
        0,
        policy,
        fallback,
        first.iter().chain(second.iter()),
    )
    .unwrap();
    assert_eq!(replayed, engine.snapshot());
}

#[test]
fn replay_streams_a_full_recovery_trace_with_one_record_of_workspace() {
    let mut engine = motor::<4>(2, 2);
    engine
        .dispatch(Command::event(1, MotorEvent::StartWhen { ready_at: 5 }))
        .unwrap();
    engine.dispatch(Command::retry(3)).unwrap();
    engine.dispatch(Command::retry(5)).unwrap();

    let replayed = verify_replay::<_, _, 1>(
        Motor,
        MotorState::Idle,
        engine.policy(),
        Transition::with_effect(MotorState::Stopped, MotorEffect::Disable),
        engine.journal().iter(),
    )
    .unwrap();
    assert_eq!(replayed, engine.snapshot());
}

#[test]
fn replay_rejects_changed_effect_intent_even_if_snapshots_match() {
    let mut engine = motor::<4>(2, 3);
    engine
        .dispatch(Command::event(1, MotorEvent::Start))
        .unwrap();
    let mut records: Vec<_> = engine.journal().iter().copied().collect();
    records[0].outcome = Outcome::Applied {
        effect: Some(MotorEffect::Disable),
    };

    assert_eq!(
        verify_replay::<_, _, 1>(
            Motor,
            MotorState::Idle,
            engine.policy(),
            Transition::with_effect(MotorState::Stopped, MotorEffect::Disable),
            &records,
        ),
        Err(ReplayError::Diverged { index: 0 })
    );
}

#[test]
fn replay_rejects_a_missing_prefix_and_a_gap_between_segments() {
    let policy = Policy::new(2, 3).unwrap();
    let fallback = Transition::silent(0);
    let mut engine = Engine::<Counter, 4>::new(Counter, 0, policy, fallback).unwrap();
    for tick in 1..=3 {
        engine.dispatch(Command::event(tick, 1)).unwrap();
    }
    let records: Vec<_> = engine.journal().iter().copied().collect();

    assert_eq!(
        verify_replay::<_, _, 1>(Counter, 0, policy, fallback, &records[1..]),
        Err(ReplayError::Diverged { index: 0 })
    );
    let missing_middle = [records[0], records[2]];
    assert_eq!(
        verify_replay::<_, _, 1>(Counter, 0, policy, fallback, &missing_middle),
        Err(ReplayError::Diverged { index: 1 })
    );
}

#[test]
fn replay_reports_an_input_that_cannot_have_been_accepted() {
    let policy = Policy::new(2, 3).unwrap();
    let fallback = Transition::silent(0);
    let mut engine = Engine::<Counter, 4>::new(Counter, 0, policy, fallback).unwrap();
    engine.dispatch(Command::event(1, 1)).unwrap();
    engine.dispatch(Command::event(2, 1)).unwrap();
    let mut records: Vec<_> = engine.journal().iter().copied().collect();
    records[1].command = Command::event(0, 1);

    assert_eq!(
        verify_replay::<_, _, 1>(Counter, 0, policy, fallback, &records),
        Err(ReplayError::Rejected {
            index: 1,
            error: DispatchError::TimeWentBackwards {
                current: 1,
                supplied: 0,
            },
        })
    );
}
