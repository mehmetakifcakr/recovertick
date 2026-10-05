use crate::{Command, Model, Outcome, Snapshot};

/// One accepted command and its full before/after snapshots.
/// The generic values use Rust's in-memory layout, not a portable wire format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record<S, E, F, X> {
    /// Zero-based sequence number within this engine session.
    pub sequence: u64,
    /// Accepted input.
    pub command: Command<E>,
    /// Snapshot before evaluation.
    pub before: Snapshot<S, E, F>,
    /// Snapshot after evaluation.
    pub after: Snapshot<S, E, F>,
    /// Committed decision.
    pub outcome: Outcome<F, X>,
}

/// An accepted decision record with a model's associated types substituted.
pub type ModelRecord<M> =
    Record<<M as Model>::State, <M as Model>::Event, <M as Model>::Fault, <M as Model>::Effect>;

/// Fixed-capacity, append-only journal. Full logs apply backpressure;
/// records are never overwritten or silently dropped.
pub struct Journal<R: Copy, const N: usize> {
    entries: [Option<R>; N],
    len: usize,
}

impl<R: Copy, const N: usize> Journal<R, N> {
    pub(crate) const fn new() -> Self {
        Self {
            entries: [None; N],
            len: 0,
        }
    }
    /// Number of accepted commands recorded in this segment.
    pub const fn len(&self) -> usize {
        self.len
    }
    /// Whether no command has been recorded.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Configured maximum number of accepted commands per segment.
    pub const fn capacity(&self) -> usize {
        N
    }
    /// Whether dispatch must reject the next otherwise valid command.
    pub const fn is_full(&self) -> bool {
        self.len == N
    }
    /// Read a record by segment-local index (not its session sequence number).
    pub fn get(&self, index: usize) -> Option<&R> {
        self.entries.get(index).and_then(Option::as_ref)
    }
    /// Iterate records in accepted-command order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &R> {
        // The initialized prefix always contains Some; no public mutation exists.
        self.entries[..self.len]
            .iter()
            .map(|entry| entry.as_ref().expect("initialized prefix"))
    }
    pub(crate) fn push(&mut self, record: R) {
        // Only Engine calls this after checking capacity. No application code
        // runs between that check and this append; &mut prevents concurrent use.
        self.entries[self.len] = Some(record);
        self.len += 1;
    }

    pub(crate) fn discard_verified(&mut self) {
        self.entries[..self.len].fill(None);
        self.len = 0;
    }
}
