//! Dependency-free host benchmark. Percentiles are batch averages, not
//! individual dispatch latency or embedded worst-case execution-time bounds.
use recovertick::{Command, Engine, Model, Policy, Transition};
use std::{hint::black_box, time::Instant};

struct Counter;
impl Model for Counter {
    type State = u64;
    type Event = u64;
    type Fault = ();
    type Effect = u64;
    fn transition(&self, state: u64, event: u64, _: u64) -> Result<Transition<u64, u64>, ()> {
        let next = state.wrapping_add(event);
        Ok(Transition::with_effect(next, next))
    }
}

fn main() {
    const BATCH: usize = 256;
    const SAMPLES: usize = 1000;
    let policy = Policy::new(2, 3).unwrap();
    let mut samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES + 10 {
        let mut engine =
            Engine::<Counter, BATCH>::new(Counter, 0, policy, Transition::silent(0)).unwrap();
        let start = Instant::now();
        for tick in 0..BATCH as u64 {
            let record = engine
                .dispatch(Command::event(black_box(tick), black_box(1)))
                .unwrap();
            black_box(record);
        }
        let ns = start.elapsed().as_nanos() as f64 / BATCH as f64;
        black_box(engine.snapshot());
        if sample >= 10 {
            samples.push(ns);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "host={} samples={} batch={} (pure Counter + full record copies)",
        std::env::consts::ARCH,
        SAMPLES,
        BATCH
    );
    println!(
        "batch_average_ns_per_dispatch p50={:.2} p99={:.2}",
        samples[SAMPLES / 2],
        samples[SAMPLES * 99 / 100]
    );
}
