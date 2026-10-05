//! An entire simulated fault/recovery session with verified replay.
use recovertick::{
    demo::{Motor, MotorEffect, MotorEvent, MotorState},
    verify_replay, Command, Engine, Policy, Transition,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let policy = Policy::new(2, 3)?;
    let fallback = Transition::with_effect(MotorState::Stopped, MotorEffect::Disable);
    let mut engine = Engine::<Motor, 8>::new(Motor, MotorState::Idle, policy, fallback)?;
    // The simulated observation remains unavailable until logical tick 5.
    for command in [
        Command::event(1, MotorEvent::StartWhen { ready_at: 5 }),
        Command::retry(4),
        Command::retry(7),
    ] {
        let record = engine.dispatch(command)?;
        println!("tick {}: {:?}", record.after.tick, record.outcome);
    }
    let replayed = verify_replay::<Motor, _, 1>(
        Motor,
        MotorState::Idle,
        policy,
        fallback,
        engine.journal().iter(),
    )?;
    assert_eq!(replayed, engine.snapshot());
    println!("replay verified: {:?}", replayed.state);
    Ok(())
}
