use soroban_sdk::{contracttype, Address, Env};

use crate::ContractError;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultPosition {
    pub owner: Address,
    pub collateral_value: u128,
    pub liquidation_threshold_bps: u32,
    pub borrowed_value: u128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquidationResult {
    pub liquidated: bool,
    pub health_factor: u128,
    pub liquidator_reward: u128,
    pub protocol_reserve: u128,
}

pub fn health_factor(position: &VaultPosition) -> Result<u128, ContractError> {
    if position.borrowed_value == 0 {
        return Ok(u128::MAX);
    }
    let numerator = position
        .collateral_value
        .checked_mul(position.liquidation_threshold_bps as u128)
        .ok_or(ContractError::MathOverflow)?;
    Ok(numerator / position.borrowed_value / 10_000)
}

pub fn liquidate(
    _env: &Env,
    position: &VaultPosition,
    purchase_collateral: u128,
) -> Result<LiquidationResult, ContractError> {
    let hf = health_factor(position)?;
    if hf >= 1 {
        return Ok(LiquidationResult {
            liquidated: false,
            health_factor: hf,
            liquidator_reward: 0,
            protocol_reserve: 0,
        });
    }

    let reward = purchase_collateral / 20;
    Ok(LiquidationResult {
        liquidated: true,
        health_factor: hf,
        liquidator_reward: reward,
        protocol_reserve: purchase_collateral - reward,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn calculates_health_factor_and_liquidation() {
        let env = Env::default();
        let position = VaultPosition {
            owner: Address::generate(&env),
            collateral_value: 100,
            liquidation_threshold_bps: 5_000,
            borrowed_value: 60,
        };
        assert!(health_factor(&position).unwrap() < 1);
        let result = liquidate(&env, &position, 100).unwrap();
        assert!(result.liquidated);
        assert_eq!(result.liquidator_reward, 5);
        assert_eq!(result.protocol_reserve, 95);
    }
}
