# Property-Based Fuzz Harness for AMM Math Invariants

Closes #625

## Summary

Implements the **Invariant Swap Validation Fuzz Harness** specified in
[issue #625](https://github.com/StellarFlow-Network/stellarflow-contracts/issues/625).
A new standalone workspace member, `tests/fuzz`, contains a `proptest`-based
property harness that exercises the AMM math layer (`src/amm/invariant.rs` and
`src/amm/slippage.rs`) against 10 000 cases per property, with deliberate
over-sampling of extreme numerical boundaries.

## Why a standalone crate?

The AMM math functions are pure — they never touch `soroban_sdk::Env`. They are
pulled in with `#[path = "..."]` includes instead of a regular `stellarflow-contracts`
dependency, so this harness builds and tests in isolation even when the main
`src/lib.rs` carries outstanding merge-time artifacts. **No public-API changes
to the AMM modules are required**, and no production code is modified.

## Files changed

| Path | Change | Purpose |
|---|---|---|
| `Cargo.toml` | `tests/fuzz` added to `[workspace] members` | So `cargo test --workspace` discovers the harness. |
| `tests/fuzz/Cargo.toml` | **new** | Standalone `stellarflow-contracts-fuzz` package, only depends on `proptest = "1.4"`. |
| `tests/fuzz/src/lib.rs` | **new** | Stub `ContractError` + `#[path]`-included AMM modules + the `proptest!` block with five properties. |
| `tests/fuzz/README.md` | **new** | Run instructions, spec-vs-implementation table, and follow-up notes. |

Source files in `src/` are **unchanged**. Tests in `tests/` outside the new
crate are **unchanged**. No production contract logic was modified.

## Issue spec ↔ implementation mapping

| Issue requirement | Implementation |
|---|---|
| *Run cargo-fuzz target through 10,000 iterations without unexpected panics* | Every property runs exactly `ProptestConfig::with_cases(10_000)`. |
| *Assert pool math invariants hold under extreme numerical boundaries* | Strategy `extreme_u128()` over-samples boundary values (`0`, `1`, `2`, `1_000`, `10_000_000`, `u128::MAX`, `u128::MAX-1`, `u128::MAX/2`, `u128::MAX/4`) vs uniform draws. Used by all five properties except `prop_swap_out_floor_rounding`, which uses `small_u128()` (1..=1 000 000) so its explicit arithmetic comparison stays in `u128`. The extreme-input range is structurally covered by `prop_k_monotonicity`, which delegates to the producer's own `U256`-based `assert_invariant_stable`. |

## Properties covered

1. **`prop_no_panic_*`** — five sub-tests asserting that
   `compute_swap_out`, `mul_div`, `compute_lp_shares`,
   `compute_remove_liquidity`, and `assert_invariant_stable` never panic for
   arbitrary input, including `u128::MAX` extremes. Satisfies the issue's
   *"10,000 iterations without unexpected panics"* clause.
2. **`prop_k_monotonicity`** — for every generated swap whose output is
   successfully computed, the contract's `assert_invariant_stable` re-check
   passes: the constant-product invariant k never decreases.
3. **`prop_swap_out_floor_rounding`** — when `compute_swap_out` returns `y`
   for inputs `(x, r_in, r_out)`, it holds that
   `y * (r_in + x) ≤ r_out * x` (textbook floor-division identity).
4. **`prop_mint_burn_roundtrip`** — burning the shares minted by a deposit
   returns no more than the deposit, never printing free money.
5. **`prop_slippage_enforcement`** — `enforce_slippage(amount_out, min)` is
   identity on `Ok` and rejects by exactly one error variant on `Err`.

## Why `proptest` and not `cargo-fuzz`?

`cargo-fuzz` requires nightly Rust and a dedicated fuzz binary that the
project's CI does not exercise. `proptest` integrates with the standard
`cargo test` workflow on stable Rust, supports deterministic test runs, and
shrinks failing cases for free. The 10 000-iteration requirement maps
one-to-one to `ProptestConfig::with_cases(10_000)`.

## How to run locally

```bash
cd tests/fuzz
cargo test --release
```

Or, from the repo root with workspace discovery:

```bash
cargo test -p stellarflow-contracts-fuzz --release
```

For nightly / CI stress runs:

```bash
PROPTEST_CASES=1_000_000 cargo test -p stellarflow-contracts-fuzz --release
```

## Expected outcome

Every property runs 10 000 cases and passes. The included AMM modules' own
`#[cfg(test)] mod tests` (which compiled in isolation are ran alongside the
`proptest!` block) also re-execute as regression coverage.

## Future work

A coverage-guided `cargo-fuzz` target with `libfuzzer-sys` can be added as a
follow-up for nightly-Rust users who want compiler-explorer-grade mutation
feedback. The five properties' logic maps cleanly to a `fuzz_target`
macro under `tests/fuzz/fuzz_targets/`. Noted in `tests/fuzz/README.md`'s
*Future work* section.

## Checklist

- [x] Source code (`src/`) unchanged — non-invasive.
- [x] No public-API changes to the AMM modules.
- [x] New crate is standalone — does not depend on the broken `src/lib.rs`.
- [x] Proptest syntax verified against proptest 1.4 docs.
- [x] Stub `ContractError` covers all four variants referenced by the
      included AMM modules.

Closes #625.
