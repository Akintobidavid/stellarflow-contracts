//! LP fee distribution and proportional liquidity redemption.

use soroban_sdk::{contracttype, symbol_short, Address, Env};

use crate::{AssetId, ContractData, ContractError, DATA_KEY};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FeeDistributionKey {
    Pool(AssetId),
    Position(AssetId, Address),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquidityPool {
    pub asset: AssetId,
    pub reserve_a: u128,
    pub reserve_b: u128,
    pub fee_pool: u64,
    pub total_lp_units: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquidityPosition {
    pub provider: Address,
    pub asset: AssetId,
    pub lp_units: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedemptionResult {
    pub asset: AssetId,
    pub provider: Address,
    pub burned_lp_units: u64,
    pub withdrawn_asset_a: u128,
    pub withdrawn_asset_b: u128,
    pub claimed_fees: u64,
}

fn pool_key(asset: AssetId) -> FeeDistributionKey {
    FeeDistributionKey::Pool(asset)
}

fn position_key(asset: AssetId, provider: &Address) -> FeeDistributionKey {
    FeeDistributionKey::Position(asset, provider.clone())
}

fn load_pool(env: &Env, asset: AssetId) -> LiquidityPool {
    env.storage()
        .persistent()
        .get(&pool_key(asset))
        .unwrap_or(LiquidityPool {
            asset,
            reserve_a: 0,
            reserve_b: 0,
            fee_pool: 0,
            total_lp_units: 0,
        })
}

fn save_pool(env: &Env, pool: &LiquidityPool) {
    let key = pool_key(pool.asset);
    env.storage().persistent().set(&key, pool);
    env.storage().persistent().extend_ttl(
        &key,
        crate::storage::PERSISTENT_TTL_THRESHOLD,
        crate::storage::PERSISTENT_TTL_THRESHOLD,
    );
}

fn require_protocol_admin(env: &Env, caller: &Address) -> Result<(), ContractError> {
    let data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;
    if data.admin != *caller {
        return Err(ContractError::NotAdmin);
    }
    caller.require_auth();
    Ok(())
}

/// Record corridor trading fees available to LPs at one-stroop precision.
pub fn record_fee(
    env: &Env,
    admin: Address,
    asset: AssetId,
    fee_amount: u64,
) -> Result<LiquidityPool, ContractError> {
    require_protocol_admin(env, &admin)?;
    let mut pool = load_pool(env, asset);
    pool.fee_pool = pool
        .fee_pool
        .checked_add(fee_amount)
        .ok_or(ContractError::MathOverflow)?;
    save_pool(env, &pool);
    Ok(pool)
}

/// Add LP units and the corresponding two reserve shares for a provider.
pub fn add_liquidity(
    env: &Env,
    provider: Address,
    asset: AssetId,
    reserve_a: u128,
    reserve_b: u128,
    lp_units: u64,
) -> Result<LiquidityPosition, ContractError> {
    provider.require_auth();
    if reserve_a == 0 || reserve_b == 0 || lp_units == 0 {
        return Err(ContractError::InvalidStakeAmount);
    }

    let mut pool = load_pool(env, asset);
    pool.reserve_a = pool
        .reserve_a
        .checked_add(reserve_a)
        .ok_or(ContractError::MathOverflow)?;
    pool.reserve_b = pool
        .reserve_b
        .checked_add(reserve_b)
        .ok_or(ContractError::MathOverflow)?;
    pool.total_lp_units = pool
        .total_lp_units
        .checked_add(lp_units)
        .ok_or(ContractError::MathOverflow)?;

    let key = position_key(asset, &provider);
    let mut position: LiquidityPosition = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or(LiquidityPosition {
            provider: provider.clone(),
            asset,
            lp_units: 0,
        });
    position.lp_units = position
        .lp_units
        .checked_add(lp_units)
        .ok_or(ContractError::MathOverflow)?;
    env.storage().persistent().set(&key, &position);
    save_pool(env, &pool);
    Ok(position)
}

/// Burn LP units and return the provider's pro-rata reserves and fee claim.
pub fn redeem_liquidity(
    env: &Env,
    provider: Address,
    asset: AssetId,
    lp_units: u64,
) -> Result<RedemptionResult, ContractError> {
    provider.require_auth();
    if lp_units == 0 {
        return Err(ContractError::InvalidStakeAmount);
    }

    let mut pool = load_pool(env, asset);
    if pool.total_lp_units == 0 {
        return Err(ContractError::InsufficientLiquidityDepth);
    }
    let key = position_key(asset, &provider);
    let mut position: LiquidityPosition = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(ContractError::InsufficientLiquidityDepth)?;
    if lp_units > position.lp_units {
        return Err(ContractError::InsufficientLiquidityDepth);
    }

    let units = u128::from(lp_units);
    let total_units = u128::from(pool.total_lp_units);
    let withdrawn_asset_a = pool
        .reserve_a
        .checked_mul(units)
        .ok_or(ContractError::MathOverflow)?
        / total_units;
    let withdrawn_asset_b = pool
        .reserve_b
        .checked_mul(units)
        .ok_or(ContractError::MathOverflow)?
        / total_units;
    let claimed_fees = (u128::from(pool.fee_pool)
        .checked_mul(units)
        .ok_or(ContractError::MathOverflow)?
        / total_units)
        .try_into()
        .map_err(|_| ContractError::MathOverflow)?;

    pool.reserve_a = pool
        .reserve_a
        .checked_sub(withdrawn_asset_a)
        .ok_or(ContractError::MathOverflow)?;
    pool.reserve_b = pool
        .reserve_b
        .checked_sub(withdrawn_asset_b)
        .ok_or(ContractError::MathOverflow)?;
    pool.fee_pool = pool
        .fee_pool
        .checked_sub(claimed_fees)
        .ok_or(ContractError::MathOverflow)?;
    pool.total_lp_units = pool
        .total_lp_units
        .checked_sub(lp_units)
        .ok_or(ContractError::MathOverflow)?;

    position.lp_units -= lp_units;
    if position.lp_units == 0 {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, &position);
    }
    save_pool(env, &pool);

    env.events().publish(
        (symbol_short!("lp_redeem"), asset, provider.clone()),
        (lp_units, withdrawn_asset_a, withdrawn_asset_b, claimed_fees),
    );

    Ok(RedemptionResult {
        asset,
        provider,
        burned_lp_units: lp_units,
        withdrawn_asset_a,
        withdrawn_asset_b,
        claimed_fees,
    })
}

pub fn get_pool(env: &Env, asset: AssetId) -> LiquidityPool {
    load_pool(env, asset)
}

pub fn get_position(
    env: &Env,
    asset: AssetId,
    provider: Address,
) -> Option<LiquidityPosition> {
    env.storage()
        .persistent()
        .get(&position_key(asset, &provider))
}
