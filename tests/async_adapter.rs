//! Executor-free checks of the adapter's poll and cancellation boundaries.

use std::cell::Cell;
use std::future::{pending, ready, Future};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use recovertick::demo::{Motor, MotorEffect, MotorEvent, MotorFault, MotorState};
use recovertick::{Command, DispatchError, Engine, Outcome, Policy, Transition};

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn noop_waker() -> Waker {
    Waker::from(Arc::new(NoopWake))
}

fn motor<const N: usize>() -> Engine<Motor, N> {
    Engine::new(
        Motor,
        MotorState::Idle,
        Policy::new(2, 3).unwrap(),
        Transition::with_effect(MotorState::Stopped, MotorEffect::Disable),
    )
    .unwrap()
}

// This source observes polling independently of the borrowed engine. It lets
// the tests prove the adapter never pulls input before its first poll.
struct PollProbe {
    command: Command<MotorEvent>,
    polls: Rc<Cell<usize>>,
}

impl Future for PollProbe {
    type Output = Command<MotorEvent>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.polls.set(self.polls.get() + 1);
        Poll::Ready(self.command)
    }
}

#[test]
fn cancelling_a_pending_input_preserves_the_complete_engine() {
    let mut engine = motor::<4>();
    engine
        .dispatch(Command::event(7, MotorEvent::Start))
        .unwrap();
    let before = engine.snapshot();
    let records: Vec<_> = engine.journal().iter().copied().collect();
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);

    let mut operation = Box::pin(engine.dispatch_async(pending::<Command<MotorEvent>>()));
    assert!(matches!(
        operation.as_mut().poll(&mut context),
        Poll::Pending
    ));
    drop(operation);

    assert_eq!(engine.snapshot(), before);
    assert_eq!(
        engine.journal().iter().copied().collect::<Vec<_>>(),
        records
    );
}

#[test]
fn dropping_an_unpolled_adapter_does_not_poll_or_consume_input() {
    let mut engine = motor::<4>();
    let before = engine.snapshot();
    let polls = Rc::new(Cell::new(0));
    let source = PollProbe {
        command: Command::event(1, MotorEvent::Start),
        polls: Rc::clone(&polls),
    };

    let operation = Box::pin(engine.dispatch_async(source));
    drop(operation);

    assert_eq!(polls.get(), 0);
    assert_eq!(engine.snapshot(), before);
    assert!(engine.journal().is_empty());
}

#[test]
fn a_ready_input_commits_in_the_same_poll_without_another_await() {
    let mut engine = motor::<4>();
    let polls = Rc::new(Cell::new(0));
    let source = PollProbe {
        command: Command::event(1, MotorEvent::Start),
        polls: Rc::clone(&polls),
    };
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    let mut operation = Box::pin(engine.dispatch_async(source));

    let record = match operation.as_mut().poll(&mut context) {
        Poll::Ready(Ok(record)) => record,
        other => panic!("a ready source must finish this poll: {other:?}"),
    };
    drop(operation);

    assert_eq!(polls.get(), 1);
    assert_eq!(record.sequence, 0);
    assert_eq!(record.after.state, MotorState::Running);
    assert_eq!(engine.snapshot(), record.after);
    assert_eq!(engine.journal().len(), 1);
}

#[test]
fn ready_input_propagates_dispatch_error_without_mutation() {
    let mut engine = motor::<4>();
    engine
        .dispatch(Command::event(7, MotorEvent::Start))
        .unwrap();
    let before = engine.snapshot();
    let records: Vec<_> = engine.journal().iter().copied().collect();
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    let mut operation = Box::pin(engine.dispatch_async(ready(Command::event(6, MotorEvent::Stop))));

    assert_eq!(
        operation.as_mut().poll(&mut context),
        Poll::Ready(Err(DispatchError::TimeWentBackwards {
            current: 7,
            supplied: 6,
        }))
    );
    drop(operation);

    assert_eq!(engine.snapshot(), before);
    assert_eq!(
        engine.journal().iter().copied().collect::<Vec<_>>(),
        records
    );
}

#[test]
fn async_backpressure_propagates_trace_full_without_committing_input() {
    let mut engine = motor::<1>();
    engine
        .dispatch(Command::event(1, MotorEvent::Start))
        .unwrap();
    let before = engine.snapshot();
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    let mut operation = Box::pin(engine.dispatch_async(ready(Command::event(2, MotorEvent::Stop))));

    assert_eq!(
        operation.as_mut().poll(&mut context),
        Poll::Ready(Err(DispatchError::TraceFull))
    );
    drop(operation);

    assert_eq!(engine.snapshot(), before);
    assert_eq!(engine.journal().len(), 1);
}

#[test]
fn an_application_fault_is_an_accepted_async_decision() {
    let mut engine = motor::<4>();
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    let mut operation = Box::pin(engine.dispatch_async(ready(Command::event(
        1,
        MotorEvent::StartWhen { ready_at: 4 },
    ))));

    let record = match operation.as_mut().poll(&mut context) {
        Poll::Ready(Ok(record)) => record,
        other => panic!("application faults must be committed decisions: {other:?}"),
    };
    drop(operation);

    assert_eq!(
        record.outcome,
        Outcome::Scheduled {
            fault: MotorFault::Unavailable,
            attempts: 0,
            retry_at: 4,
        }
    );
    assert_eq!(engine.snapshot(), record.after);
    assert_eq!(engine.journal().len(), 1);
}
