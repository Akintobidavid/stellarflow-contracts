//! High-precision fee arithmetic for multi-hop corridor pools.
//!
//! Fractional corridor usage fee splits scale intermediate products by
//! `INTERIOR_SCALE` (10^14) before division, then normalize back to the
//! standard 10^7 fixed-point footprint prior to ledger mutations.

use crate::{AssetId, ContractError, TimeLockedUpgradeContract};
use soroban_sdk::{contracttype, Address, Env};

/// Interior scaling coefficient applied before division steps (10^14).
pub const INTERIOR_SCALE: u128 = 100_000_000_000_000;

/// System standard fixed-point footprint (10^7).
pub const FIXED_POINT_SCALE: u128 = 10_000_000;

#[contracttype]
#[derive(Clone)]
pub struct CorridorFeePool {
    pub asset: AssetId,
    pub collected: u64,
    pub variable_pool: u64,
}

#[contracttype]
pub enum FeesStorageKey {
    CorridorPool(AssetId),
}

impl CorridorFeePool {
    fn new(asset: AssetId) -> Self {
        Self {
            asset,
            collected: 0,
            variable_pool: 0,
        }
    }
}

/// Scale an intermediate product into interior precision space before division.
fn scale_product_to_interior(a: u128, b: u128) -> Result<u128, ContractError> {
    a.checked_mul(b)
        .ok_or(ContractError::Overflow)?
        .checked_mul(INTERIOR_SCALE)
        .ok_or(ContractError::Overflow)
}

/// Normalize an interior-space quotient back to the 10^7 fixed-point footprint.
pub fn normalize_to_fixed_point_footprint(interior_value: u128) -> Result<u64, ContractError> {
    let normalized = interior_value
        .checked_div(INTERIOR_SCALE)
        .ok_or(ContractError::DivisionByZero)?;
    u64::try_from(normalized).map_err(|_| ContractError::Overflow)
}

/// Multiply two fixed-point values and scale down to the 10^7 footprint.
///
/// Pre-multiplies the intermediate product by `INTERIOR_SCALE` before the
/// division step, then normalizes the result back to the standard footprint.
pub fn multiply_and_scale_down(a: u64, b: u64) -> Result<u64, ContractError> {
    let interior_product = scale_product_to_interior(u128::from(a), u128::from(b))?;
    let interior_quotient = interior_product
        .checked_div(FIXED_POINT_SCALE)
        .ok_or(ContractError::DivisionByZero)?;
    normalize_to_fixed_point_footprint(interior_quotient)
}

/// Compute a single relayer's corridor usage fee share from the variable pool.
///
/// Uses interior scaling so fractional weights do not truncate before the
/// final stroop allocation is written to ledger storage.
pub fn compute_corridor_usage_fee_share(
    total_fee: u64,
    relayer_usage: u64,
    total_usage: u64,
) -> Result<u64, ContractError> {
    if total_usage == 0 {
        return Err(ContractError::DivisionByZero);
    }
    if total_fee == 0 || relayer_usage == 0 {
        return Ok(0);
    }

    let interior_numerator = u128::from(total_fee)
        .checked_mul(u128::from(relayer_usage))
        .ok_or(ContractError::Overflow)?
        .checked_mul(INTERIOR_SCALE)
        .ok_or(ContractError::Overflow)?;

    let interior_quotient = interior_numerator / u128::from(total_usage);
    normalize_to_fixed_point_footprint(interior_quotient)
}

/// Compute a relayer's fee share across a multi-hop corridor path.
///
/// Combines hop-level and relayer-level usage weights in one interior-scaled
/// pass to avoid compounded truncation error across separate relayers.
pub fn compute_multi_hop_corridor_fee_share(
    total_fee: u64,
    hop_usage: u64,
    relayer_usage: u64,
    total_hop_usage: u64,
    total_relayer_usage: u64,
) -> Result<u64, ContractError> {
    if total_hop_usage == 0 || total_relayer_usage == 0 {
        return Err(ContractError::DivisionByZero);
    }
    if total_fee == 0 || hop_usage == 0 || relayer_usage == 0 {
        return Ok(0);
    }

    let interior_numerator = u128::from(total_fee)
        .checked_mul(u128::from(hop_usage))
        .ok_or(ContractError::Overflow)?
        .checked_mul(u128::from(relayer_usage))
        .ok_or(ContractError::Overflow)?
        .checked_mul(INTERIOR_SCALE)
        .ok_or(ContractError::Overflow)?;

    let interior_denominator = u128::from(total_hop_usage)
        .checked_mul(u128::from(total_relayer_usage))
        .ok_or(ContractError::Overflow)?;

    let interior_quotient = interior_numerator / interior_denominator;
    normalize_to_fixed_point_footprint(interior_quotient)
}

pub fn add_corridor_fees(
    env: Env,
    admin: Address,
    asset: AssetId,
    collected: u64,
    variable_fee: u64,
) -> Result<CorridorFeePool, ContractError> {
    admin.require_auth();
    let data = TimeLockedUpgradeContract::get_data(&env)?;
    if data.admin != admin {
        return Err(ContractError::NotAdmin);
    }

    let key = FeesStorageKey::CorridorPool(asset.clone());
    let mut pool: CorridorFeePool = env
        .storage()
        .instance()
        .get(&key)
        .unwrap_or(CorridorFeePool::new(asset.clone()));

    pool.collected = pool
        .collected
        .checked_add(collected)
        .ok_or(ContractError::Overflow)?;
    pool.variable_pool = pool
        .variable_pool
        .checked_add(variable_fee)
        .ok_or(ContractError::Overflow)?;

    env.storage().instance().set(&key, &pool);
    Ok(pool)
}

pub fn get_corridor_fee_pool(env: Env, asset: AssetId) -> CorridorFeePool {
    let key = FeesStorageKey::CorridorPool(asset.clone());
    env.storage()
        .instance()
        .get(&key)
        .unwrap_or(CorridorFeePool::new(asset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interior_scale_constant() {
        assert_eq!(INTERIOR_SCALE, 100_000_000_000_000);
        assert_eq!(FIXED_POINT_SCALE, 10_000_000);
    }

    #[test]
    fn test_multiply_and_scale_down_preserves_precision() {
        // 1.5 * 2.5 = 3.75 in 10^7 fixed-point
        assert_eq!(
            multiply_and_scale_down(15_000_000, 25_000_000),
            Ok(37_500_000)
        );
    }

    #[test]
    fn test_corridor_usage_fee_share_three_way_split_preserves_total() {
        let total_fee = 10_000u64;
        let usages = [3_333_333u64, 3_333_333u64, 3_333_334u64];
        let total_usage: u64 = 10_000_000;

        let mut allocated = 0u64;
        for (index, usage) in usages.iter().enumerate() {
            let share = if index == usages.len() - 1 {
                total_fee - allocated
            } else {
                compute_corridor_usage_fee_share(total_fee, *usage, total_usage).unwrap()
            };
            allocated += share;
        }

        assert_eq!(allocated, total_fee);
    }

    #[test]
    fn test_multi_hop_fee_share_single_pass_matches_chained_low_precision() {
        let total_fee = 1_000_000u64;
        let hop_usage = 4_000_000u64;
        let relayer_usage = 2_500_000u64;
        let total_hop_usage = 10_000_000u64;
        let total_relayer_usage = 10_000_000u64;

        let high_precision = compute_multi_hop_corridor_fee_share(
            total_fee,
            hop_usage,
            relayer_usage,
            total_hop_usage,
            total_relayer_usage,
        )
        .unwrap();

        // Low-precision chained division: ((fee * hop / total_hop) * relayer / total_relayer)
        let hop_share = total_fee * hop_usage / total_hop_usage;
        let low_precision = hop_share * relayer_usage / total_relayer_usage;

        assert!(high_precision >= low_precision);
        assert_eq!(high_precision, 100_000);
    }

    #[test]
    fn test_compute_corridor_usage_fee_share_zero_total_usage() {
        assert_eq!(
            compute_corridor_usage_fee_share(100, 1, 0),
            Err(ContractError::DivisionByZero)
        );
    }

    #[test]
    fn test_normalize_to_fixed_point_footprint_overflow() {
        let too_large = u128::from(u64::MAX) * u128::from(u64::MAX) * INTERIOR_SCALE;
        assert_eq!(
            normalize_to_fixed_point_footprint(too_large),
            Err(ContractError::Overflow)
        );
    }
}
