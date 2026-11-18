# Agent Task: Rust Unsafe-to-Safe and Assurance Improvement

## Objective

Improve the repository's Rust safety guarantees. Reduce unsafe usage where possible, but never optimize for the raw count of `unsafe` keywords. The target is a small, explicit, auditable trusted computing base with safe public APIs, stronger types, and evidence appropriate to each safety obligation.

## Mandatory constraints

- Do not modify code before inventorying the relevant unsafe site and its callers.
- Do not delete `unsafe` or change an unsafe function to safe without proving that callers can no longer violate its preconditions.
- Do not invent invariants, safety comments, test outcomes, or proof results.
- Do not weaken assertions, validation, error handling, API semantics, permissions, or tests.
- Do not assume `Drop` always runs. Consider `mem::forget`, panic, process termination, power loss, and device failure where relevant.
- Treat FFI, MMIO, DMA, inline assembly, interrupt code, allocators, page tables, `Send`/`Sync`, and lock-free algorithms as high or critical risk.
- Preserve public API and ABI unless the task explicitly authorizes a breaking change.
- If a property is not verified, label it `Assumed` or `Unverified`.

## Required process

1. Inventory the unsafe site, callers, dependency and platform context.
2. Extract preconditions, invariants, postconditions and failure paths.
3. Identify whether a safe standard API or mature abstraction can replace it.
4. If not, design the smallest private unsafe implementation with a safe public interface.
5. Strengthen types using Newtype, Typestate, Capability handles, private constructors and explicit state transitions where useful.
6. Select verification tools based on the property: Miri for execution-time UB checks, Kani for bounded model checking, Loom for concurrency exploration, Verus for supported functional specifications, Flux for supported refinement constraints, or research verifiers when applicable.
7. Add regression and negative tests. Preserve behavior.
8. Produce the required report and request independent review for high-risk changes.

## Required output

- Original behavior and affected call sites
- Safety obligations and their sources
- Proposed safe abstraction or reason unsafe remains
- New type-level constraints
- Runtime enforcement relied upon
- Tests and verification evidence, with tool versions and target
- Unverified assumptions and limitations
- Remaining unsafe locations
- API/ABI or performance changes
- Disposition: `Replaced | Encapsulated | Verified | Assumption-Bounded | Unresolved`

## Stop conditions

Stop and report rather than guessing when:

- hardware behavior or vendor documentation is insufficient;
- FFI ownership or lifetime is unknown;
- concurrency invariants cannot be reconstructed;
- safe replacement changes observable semantics;
- proof tooling does not support the relevant language feature;
- a timeout/cancel path cannot establish that hardware stopped accessing memory.

A successful build or test run is not by itself proof of soundness.
