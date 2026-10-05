//! Report actual Rust layouts for this host ABI, not MCU RAM/flash guarantees.
use recovertick::{
    demo::{Motor, MotorEffect, MotorEvent, MotorFault, MotorState},
    Engine, Record,
};
use std::mem::size_of;
fn main() {
    println!("target_arch={}", std::env::consts::ARCH);
    println!(
        "record_bytes={}",
        size_of::<Record<MotorState, MotorEvent, MotorFault, MotorEffect>>()
    );
    println!("engine_capacity_1_bytes={}", size_of::<Engine<Motor, 1>>());
    println!(
        "engine_capacity_16_bytes={}",
        size_of::<Engine<Motor, 16>>()
    );
    println!(
        "engine_capacity_64_bytes={}",
        size_of::<Engine<Motor, 64>>()
    );
}
