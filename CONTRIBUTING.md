# Contributing

Start with an issue containing a minimal failing trace, original model version,
policy, fallback and initial state. For timing claims include the target, clock,
compiler, optimization flags and measurement method.

Keep the core `no_std`, allocation-free, safe Rust and dependency-free. Submit
transport, persistence and executor integration as separate adapters so callers
can choose their ecosystem. Do not add hidden time, randomness or side effects
to reducer evaluation. Include a meaningful regression test when behavior changes.

Run the verification commands in README before opening a pull request. Explain
the observed fault, resulting behavior and any changed public contracts. Public
API changes require documentation and a changelog entry. New effect-delivery or
safety guarantees require explicit designs and evidence.

By contributing, you agree to license your contribution under MIT or Apache-2.0,
at the recipient's option, unless explicitly agreed otherwise.

