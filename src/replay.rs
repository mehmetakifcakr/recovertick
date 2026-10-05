use crate::journal::ModelRecord;
use crate::{ConfigError, DispatchError, Engine, Model, ModelSnapshot, Policy, Transition};
use core::fmt;

/// Failure to validate an accepted-command trace from a known initial state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// Invalid replay engine capacity.
    Config(ConfigError),
    /// Sequence, before/after state or outcome did not match recomputation.
    Diverged {
        /// Zero-based position of the first mismatch.
        index: u64,
    },
    /// A supposedly accepted input was rejected by the replay engine.
    Rejected {
        /// Position of the rejected input.
        index: u64,
        /// Rejection reason.
        error: DispatchError,
    },
}
impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl core::error::Error for ReplayError {}

/// Verify records against the same pure model, policy, fallback and initial
/// state. Missing prefixes, gaps, changed outcomes and altered snapshots fail.
/// A valid prefix is allowed; the caller must provide an expected record count
/// to detect tail truncation. Archived segments can be chained as one iterator;
/// replay reuses bounded journal storage. This is a semantic verifier, not
/// authentication or a portable serialized file format.
pub fn verify_replay<'a, M: Model, I, const N: usize>(
    model: M,
    initial: M::State,
    policy: Policy,
    fallback: Transition<M::State, M::Effect>,
    records: I,
) -> Result<ModelSnapshot<M>, ReplayError>
where
    I: IntoIterator<Item = &'a ModelRecord<M>>,
    M::State: 'a,
    M::Event: 'a,
    M::Fault: 'a,
    M::Effect: 'a,
{
    let mut engine =
        Engine::<M, N>::new(model, initial, policy, fallback).map_err(ReplayError::Config)?;
    for expected in records {
        let index = engine.next_sequence_for_replay();
        if expected.sequence != index || expected.before != engine.snapshot() {
            return Err(ReplayError::Diverged { index });
        }
        let actual = engine
            .dispatch(expected.command)
            .map_err(|error| ReplayError::Rejected { index, error })?;
        if actual != *expected {
            return Err(ReplayError::Diverged { index });
        }
        // Verification only needs the last decision; caller owns the archive.
        engine.discard_verified_record();
    }
    Ok(engine.snapshot())
}
