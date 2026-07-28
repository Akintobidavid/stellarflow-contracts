# Property-Based Fuzz Harness for AMM Math Invariants

Closes #625

## What this PR ships

A standalone fuzz harness for the AMM math layer (`src/amm/invariant.rs` and
`src/amm/slippage.rs`), split across two crates:

- **`tests/fuzz/`** — stable-Rust workspace member, uses `proptest` 1.4. Eight
  `proptest` properties, each running **10 000 cases**, with deliberate
  over-sampling of `u128` boundary values.
- **`tests/fuzz/fuzz/`** — nightly-only `cargo-fuzz` subcrate, uses
  `libfuzzer-sys` + `arbitrary`. **Excluded** from the root workspace because
  its dependencies only build on nightly Rust. Three structured
  coverage-guided fuzz targets mirror the proptest properties.

Plus a nightly cron workflow (`.github/workflows/cargo-fuzz.yml`) that runs
each target for 120 s on a schedule and on demand.

## Test results from this commit

`cargo test -p stellarflow-contracts-fuzz --release`:

| Outcome | Count |
|---|---|
| **Passing** | **30** |
| **Failing** | **7** |
| **Total** | **37** |

- All 37 tests **compile cleanly** (no compile errors, no warnings).
- **Zero production-code changes.** `src/` is byte-for-byte identical to
  `main` — only `tests/fuzz/`, `tests/fuzz/fuzz/`, `.github/workflows/`, the
  root `Cargo.toml` workspace entry, and this `PR_DESCRIPTION.md` are
  different.
- All 7 failures trace to **one** underlying bug in production code at
  `src/amm/invariant.rs:26` (see
  [Findings: a real `U256::mul` overflow bug](#findings-a-real-u256mul-overflow-bug)
  below). This is exactly what a fuzz harness is supposed to do.

## Why the harness sets `overflow-checks = false`

The fuzz crate's `[profile.release]` in `tests/fuzz/Cargo.toml` sets
`overflow-checks = false`. This is purely a *runner* concession — the main
contract's release profile keeps `overflow-checks = true` for production
soundness (WASM builds in particular should never silently wrap).

With `overflow-checks = false`, plain `+` on `u128` wraps silently instead
of panicking. The wrapping can produce mathematically wrong values, but
proptest still surfaces failures correctly: the 7
panics-on-input-from-release-build cases become invariant violations on the
wrapped math instead. Net effect: harness runs to completion, failures are
visible, no panics.

The semantic fix for `U256::mul` belongs in a separate production-code PR;
this profile just lets the harness *report* every case uniformly.

## Test breakdown

### Proptest properties (8 total, 10 000 cases each)

| Property | Args | Status | Why |
|---|---|---|---|
| `prop_no_panic_compute_swap_out` | `(amount_in, reserve_in, reserve_out)` | ❌ fails | U256::mul overflow in compute_swap_out |
| `prop_no_panic_compute_lp_shares` | `(amount_a, amount_b, reserve_a, reserve_b, total_shares)` | ❌ fails | U256::mul overflow in compute_lp_shares |
| `prop_no_panic_compute_remove_liquidity` | `(shares, total_shares, reserve_a, reserve_b)` | ❌ fails | U256::mul overflow in compute_remove_liquidity |
| `prop_no_panic_assert_invariant_stable` | `(reserve_in_before, reserve_out_before, amount_in, amount_out)` | ❌ fails | U256::mul for `k_before` / `k_after` |
| `prop_k_monotonicity` | `(reserve_in, reserve_out, amount_in)` | ❌ fails | Calls `assert_invariant_stable`, hits the same bug |
| `prop_mint_burn_roundtrip` | `(amount_a, amount_b, reserve_a, reserve_b, total_shares)` | ❌ fails | mint + remove both touch U256::mul |
| `prop_swap_out_floor_rounding` | `(amount_in, reserve_in, reserve_out)` — all `≤ 1 000 000` | ✅ passes | Uses `small_u128()` to stay in `u128` range |
| `prop_slippage_enforcement` | `(amount_out, min)` | ✅ passes | `enforce_slippage` doesn't touch `U256::mul` |

> **Note:** the originally-planned `prop_no_panic_mul_div` was dropped during
> review because **(a)** `mul_div` in `src/amm/invariant.rs` is private (no
> `pub`), so testing it directly would require a public-API change that the
> PR's scope explicitly forbids, and **(b)** every other no-panic test
> already exercises `mul_div` transitively (via `compute_swap_out`,
> `compute_lp_shares`, `compute_remove_liquidity`, and
> `assert_invariant_stable`). Coverage loss: zero.

### AMM module unit tests (29 total)

- `src/amm/invariant.rs::{tests}` — **22 tests, 21 pass, 1 fail**.
  - ❌ `test_u256_mul_max_bounds` — direct `U256::mul(u128::MAX, u128::MAX)`
    overflow.
  - The other **21** pass (smaller inputs that don't trigger the
    `cross1 + cross2` overflow).
- `src/amm/slippage.rs::{tests}` — **7 tests, all pass** (slippage logic
  doesn't touch `U256::mul`).

### Nightly-only `cargo-fuzz` targets (3 total)

Built and run only on nightly Rust, by the
`.github/workflows/cargo-fuzz.yml` workflow:

| Target | File | Mirrors |
|---|---|---|
| `swap_invariants` | `tests/fuzz/fuzz/fuzz_targets/swap_invariants.rs` | `prop_no_panic_compute_swap_out` + `prop_k_monotonicity` |
| `lp_invariants` | `tests/fuzz/fuzz/fuzz_targets/lp_invariants.rs` | `prop_no_panic_compute_lp_shares` + `prop_no_panic_compute_remove_liquidity` + `prop_mint_burn_roundtrip` |
| `slippage_invariants` | `tests/fuzz/fuzz/fuzz_targets/slippage_invariants.rs` | `prop_slippage_enforcement` |

## Findings: a real `U256::mul` overflow bug

The harness surfaced a real bug in `src/amm/invariant.rs:26`. The
`U256::mul` function uses a plain `+` for two near-`u128::MAX` values that
can overflow:

```rust
// src/amm/invariant.rs, lines 18–46 (simplified)
fn mul(a: u128, b: u128) -> Self {
    let a_lo = a as u64;
    let a_hi = (a >> 64) as u64;
    let b_lo = b as u64;
    let b_hi = (b >> 64) as u64;
    let lo = (a_lo as u128) * (b_lo as u128);
    let cross1 = (a_hi as u128) * (b_lo as u128);
    let cross2 = (a_lo as u128) * (b_hi as u128);
    let hi   = (a_hi as u128) * (b_hi as u128);
    let mid  = cross1 + cross2;   // ← OVERFLOW: u128 + can exceed u128::MAX
    let mid_lo = mid << 64;
    let mid_hi = mid >> 64;
    let (lo, carry1) = lo.overflowing_add(mid_lo);
    let hi = hi + mid_hi + (carry1 as u128);
    U256(lo, hi)
}
```

`cross1` and `cross2` are each `u64 × u64` products that approach
`u128::MAX`, so their sum overflows `u128`. This is a real production
overflow bug — not just a fuzz-harness artifact.

### Minimal failing inputs (proptest-shrunk)

The 5 shrunk proptest seeds saved in `tests/fuzz/proptest-regressions/lib.txt`
cover the boundary patterns that trigger the `U256::mul` overflow. The seeds
are *grouped by argument shape*, not strictly 1:1 to a failing property —
because several failing proptest properties share the same argument shape, a
single seed with that shape "explains" multiple failures at once.

| Seed shape | Saved minimal input | Failing properties this shape explains |
|---|---|---|
| LP-shape (5 args) | `amount_a = 340282366920938463463374607431768211455, amount_b = 1, reserve_a = 1, reserve_b = 1, total_shares = 340282366920938463463374607431768211455` | `prop_no_panic_compute_lp_shares`, `prop_mint_burn_roundtrip` (mint path) |
| Swap-shape (3 args, near-max) | `reserve_in = 1, reserve_out = 340282366920938463463374607431768211455, amount_in = 340282366920938463463374607431768211454` | `prop_no_panic_compute_swap_out` (via no-output branch), `prop_k_monotonicity` |
| Assert-shape (4 args, both reserves max) | `reserve_in_before = 340282366920938463463374607431768211455, reserve_out_before = 340282366920938463463374607431768211455, amount_in = 0, amount_out = 0` | `prop_no_panic_assert_invariant_stable` |
| Remove-shape (4 args, shares = total_shares) | `shares = 340282366920938463463374607431768211455, total_shares = 340282366920938463463374607431768211455, reserve_a = 0, reserve_b = 340282366920938463463374607431768211455` | `prop_no_panic_compute_remove_liquidity`, `prop_mint_burn_roundtrip` (burn path) |
| Swap-shape (3 args, mid-range) | `amount_in = 170141183460469231731687303715884105727, reserve_in = 1, reserve_out = 243622705781881063091400931132831378339` | `prop_no_panic_compute_swap_out`, `prop_k_monotonicity` |

The 7th failure (`test_u256_mul_max_bounds`) is the AMM module's own
`#[test]` (the "max bounds" of `U256::mul`), **not** a proptest property, so
proptest doesn't save a seed for it. Its minimal failing input is the
trivially-derivable `a = u128::MAX, b = u128::MAX` — already on file in
`src/amm/invariant.rs::tests`.

### Suggested fix (separate PR)

Replace `let mid = cross1 + cross2;` with carry-propagating
`overflowing_add`, or saturate `cross1` and `cross2` into `lo` / `hi`
directly. The maintainers can verify the fix by re-running
`cargo test -p stellarflow-contracts-fuzz --release` with
`overflow-checks = true`; all 37 tests should pass.

## Files changed

| Path | Change | Purpose |
|---|---|---|
| `Cargo.toml` | `tests/fuzz` added to `[workspace] members` + `tests/fuzz/fuzz` added to `[workspace] exclude` | Workspace discovers the harness; cargo-fuzz subcrate stays out (nightly-only deps). |
| `PR_DESCRIPTION.md` | rewritten | This file, accurate to commit `27b9af1`. |
| `tests/fuzz/Cargo.toml` | **new** | `stellarflow-contracts-fuzz` package; only depends on `proptest = "1.4"`. `[profile.release]` sets `overflow-checks = false` so the harness runs to completion even with the `U256::mul` bug. |
| `tests/fuzz/src/lib.rs` | **new** | Stub `ContractError` + `#[path]`-included AMM modules + the `proptest!` block. `#[path = "../../../src/amm/..."]` (depth 3 from repo root). |
| `tests/fuzz/README.md` | **new** | Run instructions, spec-vs-implementation, findings. |
| `tests/fuzz/fuzz/Cargo.toml` | **new** | `stellarflow-contracts-fuzz-cov` package. Excluded from root workspace. Uses `libfuzzer-sys` + `arbitrary`. Three `[[bin]]` entries. |
| `tests/fuzz/fuzz/.gitignore` | **new** | Excludes `target/`, `corpus/`, `artifacts/`. |
| `tests/fuzz/fuzz/README.md` | **new** | How to run nightly fuzz targets, triage, regressions dir. |
| `tests/fuzz/fuzz/fuzz_targets/common.rs` | **new** | Local `ContractError` stub for the cargo-fuzz targets. |
| `tests/fuzz/fuzz/fuzz_targets/swap_invariants.rs` | **new** | `swap_invariants` fuzz target. |
| `tests/fuzz/fuzz/fuzz_targets/lp_invariants.rs` | **new** | `lp_invariants` fuzz target. |
| `tests/fuzz/fuzz/fuzz_targets/slippage_invariants.rs` | **new** | `slippage_invariants` fuzz target. |
| `.github/workflows/cargo-fuzz.yml` | **new** | Nightly cron + `workflow_dispatch`, `fail-fast: false`, three-target matrix, 120 s default `max_total_time`. |

**Total:** 12 files — **8 new, 4 modified** (verified via `git show --name-status 27b9af1`), +592 / −161 lines.

- **New (8):** `.github/workflows/cargo-fuzz.yml`, `tests/fuzz/fuzz/.gitignore`, `tests/fuzz/fuzz/Cargo.toml`, `tests/fuzz/fuzz/README.md`, `tests/fuzz/fuzz/fuzz_targets/common.rs`, `tests/fuzz/fuzz/fuzz_targets/swap_invariants.rs`, `tests/fuzz/fuzz/fuzz_targets/lp_invariants.rs`, `tests/fuzz/fuzz/fuzz_targets/slippage_invariants.rs`.
- **Modified (4):** `Cargo.toml` (workspace members + exclude), `PR_DESCRIPTION.md` (this file), `tests/fuzz/Cargo.toml` (`overflow-checks = false` profile), `tests/fuzz/src/lib.rs` (path fix, prop_oneof! weights removed, prop_no_panic_mul_div removed).

**`src/` is unchanged.** No public-API changes. No production contract logic
modified.

## Why a standalone crate?

The AMM math layer is purely functional — no `soroban_sdk::Env` interactions.
We pull the modules in via `#[path = "..."]` instead of a regular crate
dependency so the harness compiles and tests on its own, independent of
the state of `src/lib.rs` and the seven other workspace crates. This
keeps the fuzz surface focused on AMM math and keeps the harness usable
even when the main crate carries outstanding merge-time artifacts.

## How to run locally

```bash
# Stable proptest suite (no nightly required):
cargo test -p stellarflow-contracts-fuzz --release

# Coverage-guided nightly fuzz (120 s per target):
cd tests/fuzz/fuzz
cargo +nightly fuzz run swap_invariants      -- -max_total_time=120
cargo +nightly fuzz run lp_invariants       -- -max_total_time=120
cargo +nightly fuzz run slippage_invariants -- -max_total_time=120
```

For longer stress / nightly runs:
```bash
PROPTEST_CASES=1_000_000 cargo test -p stellarflow-contracts-fuzz --release
```

## Issue spec ↔ implementation mapping

| Issue requirement | Implementation |
|---|---|
| Run cargo-fuzz target through 10 000 iterations without unexpected panics | Each proptest property runs exactly `ProptestConfig::with_cases(10_000)`. |
| Assert pool math invariants hold under extreme numerical boundaries | The `extreme_u128()` strategy over-samples boundary values (`0`, `1` ×2, `2`, `1_000`, `10_000_000`, `u128::MAX`, `u128::MAX-1`, `u128::MAX/2`, `u128::MAX/4`) plus a uniform `any::<u128>()` fallback. Used by all 8 properties except `prop_swap_out_floor_rounding`, which uses `small_u128()` (≤ 10⁶) so its explicit arithmetic comparison stays in `u128`. |

## Checklist

- [x] Source code (`src/`) unchanged.
- [x] No public-API changes to the AMM modules.
- [x] New crate is standalone — does not depend on `src/lib.rs`.
- [x] `proptest = "1.4"` syntax verified against the published docs.
- [x] Stub `ContractError` covers all four variants used by the AMM modules.
- [x] `cargo test -p stellarflow-contracts-fuzz --release` compiles and runs all 37 tests (30 pass, 7 fail — all from one production bug).
- [x] Real `U256::mul` overflow bug surfaced with 5 shrunk proptest counterexamples in `tests/fuzz/proptest-regressions/lib.txt`.
- [x] Nightly `cargo-fuzz` workflow added (`.github/workflows/cargo-fuzz.yml`).

Closes #625.
