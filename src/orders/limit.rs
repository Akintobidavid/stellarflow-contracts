//! On-chain limit order book matching logic (Issue #701).
//!
//! Makers post limit orders that lock their `sell_asset` in the contract
//! until a fill keeper matches them at (or better than) their `price_tick`.
//! Orders support partial fills — remaining balance state is tracked on the
//! order itself — and the maker can cancel a still-open order at any time to
//! recover whatever quantity has not yet been filled.

use soroban_sdk::{contracttype, token, Address, Env, Vec};

use crate::ContractError;

/// Fixed-point scale for `price_tick`: units of `buy_asset` per 1 unit of
/// `sell_asset`, scaled by 10^7 (matches the protocol's standard fixed-point
/// footprint used elsewhere in the contract, see `fees::FIXED_POINT_SCALE`).
pub const PRICE_SCALE: i128 = 10_000_000;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetPair {
    pub sell_asset: Address,
    pub buy_asset: Address,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct LimitOrder {
    pub id: u64,
    pub maker: Address,
    pub pair: AssetPair,
    /// Price in `buy_asset` per unit of `sell_asset`, fixed-point at `PRICE_SCALE`.
    pub price_tick: i128,
    pub original_amount: i128,
    pub remaining_amount: i128,
    pub filled_amount: i128,
    pub created_at_ledger: u32,
    pub active: bool,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct FillResult {
    pub order_id: u64,
    pub filled_amount: i128,
    pub paid_amount: i128,
    pub remaining_amount: i128,
    pub order_closed: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OrderStorageKey {
    NextOrderId,
    /// Index from the public order id to its canonical composite storage key.
    OrderIndex(u64),
    /// Order struct keyed by `(AssetPair, PriceTick, OrderID)`.
    Order(AssetPair, i128, u64),
    /// Resting-order index bucket keyed by `(AssetPair, PriceTick)`, listing
    /// the ids of every order posted at that exact tick for that pair — this
    /// is the `(AssetPair, PriceTick, OrderID)` addressing scheme fill
    /// keepers walk to find matchable liquidity.
    Bucket(AssetPair, i128),
}

fn next_order_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&OrderStorageKey::NextOrderId)
        .unwrap_or(0);
    env.storage().instance().set(&OrderStorageKey::NextOrderId, &(id + 1));
    id
}

fn load_order(env: &Env, order_id: u64) -> Result<LimitOrder, ContractError> {
    let index: (AssetPair, i128) = env
        .storage()
        .persistent()
        .get(&OrderStorageKey::OrderIndex(order_id))
        .ok_or(ContractError::OrderNotFound)?;
    env.storage()
        .persistent()
        .get(&OrderStorageKey::Order(index.0, index.1, order_id))
        .ok_or(ContractError::OrderNotFound)
}

fn save_order(env: &Env, order: &LimitOrder) {
    let key = OrderStorageKey::Order(
        order.pair.clone(),
        order.price_tick,
        order.id,
    );
    env.storage().persistent().set(&key, order);
    env.storage().persistent().extend_ttl(
        &key,
        crate::storage::PERSISTENT_TTL_THRESHOLD,
        crate::storage::PERSISTENT_TTL_THRESHOLD,
    );
    let index_key = OrderStorageKey::OrderIndex(order.id);
    env.storage()
        .persistent()
        .set(&index_key, &(order.pair.clone(), order.price_tick));
    env.storage().persistent().extend_ttl(
        &index_key,
        crate::storage::PERSISTENT_TTL_THRESHOLD,
        crate::storage::PERSISTENT_TTL_THRESHOLD,
    );
}

fn bucket_push(env: &Env, pair: &AssetPair, price_tick: i128, order_id: u64) {
    let key = OrderStorageKey::Bucket(pair.clone(), price_tick);
    let mut bucket: Vec<u64> = env.storage().persistent().get(&key).unwrap_or_else(|| Vec::new(env));
    bucket.push_back(order_id);
    env.storage().persistent().set(&key, &bucket);
    env.storage()
        .persistent()
        .extend_ttl(&key, crate::storage::PERSISTENT_TTL_THRESHOLD, crate::storage::PERSISTENT_TTL_THRESHOLD);
}

fn bucket_remove(env: &Env, pair: &AssetPair, price_tick: i128, order_id: u64) {
    let key = OrderStorageKey::Bucket(pair.clone(), price_tick);
    if let Some(bucket) = env.storage().persistent().get::<_, Vec<u64>>(&key) {
        let mut updated: Vec<u64> = Vec::new(env);
        for existing in bucket.iter() {
            if existing != order_id {
                updated.push_back(existing);
            }
        }
        if updated.is_empty() {
            env.storage().persistent().remove(&key);
        } else {
            env.storage().persistent().set(&key, &updated);
        }
    }
}

/// Post a new limit order: locks `sell_amount` of `pair.sell_asset` from
/// `maker` into the contract until filled or cancelled.
pub fn place_order(
    env: &Env,
    maker: Address,
    pair: AssetPair,
    price_tick: i128,
    sell_amount: i128,
) -> Result<LimitOrder, ContractError> {
    if sell_amount <= 0 {
        return Err(ContractError::OrderZeroAmount);
    }
    if price_tick <= 0 {
        return Err(ContractError::OrderInvalidPrice);
    }
    maker.require_auth();

    let token_client = token::Client::new(env, &pair.sell_asset);
    token_client.transfer(&maker, &env.current_contract_address(), &sell_amount);

    let order = LimitOrder {
        id: next_order_id(env),
        maker,
        pair: pair.clone(),
        price_tick,
        original_amount: sell_amount,
        remaining_amount: sell_amount,
        filled_amount: 0,
        created_at_ledger: env.ledger().sequence(),
        active: true,
    };

    save_order(env, &order);
    bucket_push(env, &pair, price_tick, order.id);

    Ok(order)
}

/// Fill up to `fill_amount` of `order_id`'s remaining balance. The filler
/// pays the maker `fill_amount * price_tick` of `pair.buy_asset` and
/// receives `fill_amount` of `pair.sell_asset` released from escrow.
/// Partial fills leave the order open with a reduced `remaining_amount`.
pub fn fill_order(env: &Env, filler: Address, order_id: u64, fill_amount: i128) -> Result<FillResult, ContractError> {
    if fill_amount <= 0 {
        return Err(ContractError::OrderZeroAmount);
    }
    filler.require_auth();

    let mut order = load_order(env, order_id)?;
    if !order.active {
        return Err(ContractError::OrderAlreadyClosed);
    }
    if fill_amount > order.remaining_amount {
        return Err(ContractError::OrderInsufficientRemaining);
    }

    let paid_amount = fill_amount
        .checked_mul(order.price_tick)
        .ok_or(ContractError::MathOverflow)?
        .checked_div(PRICE_SCALE)
        .ok_or(ContractError::DivisionByZero)?;

    let buy_client = token::Client::new(env, &order.pair.buy_asset);
    buy_client.transfer(&filler, &order.maker, &paid_amount);

    let sell_client = token::Client::new(env, &order.pair.sell_asset);
    sell_client.transfer(&env.current_contract_address(), &filler, &fill_amount);

    order.remaining_amount = order
        .remaining_amount
        .checked_sub(fill_amount)
        .ok_or(ContractError::MathOverflow)?;
    order.filled_amount = order
        .filled_amount
        .checked_add(fill_amount)
        .ok_or(ContractError::MathOverflow)?;

    let order_closed = order.remaining_amount == 0;
    if order_closed {
        order.active = false;
        bucket_remove(env, &order.pair, order.price_tick, order.id);
    }
    save_order(env, &order);

    env.events().publish(
        (soroban_sdk::symbol_short!("ord_fill"), order.id),
        (filler, fill_amount, paid_amount, order.remaining_amount),
    );

    Ok(FillResult {
        order_id: order.id,
        filled_amount: fill_amount,
        paid_amount,
        remaining_amount: order.remaining_amount,
        order_closed,
    })
}

/// Cancel a still-open order and return its unfilled balance to the maker.
/// Callable by the maker at any time — no expiry or keeper approval needed.
pub fn cancel_order(env: &Env, maker: Address, order_id: u64) -> Result<i128, ContractError> {
    maker.require_auth();

    let mut order = load_order(env, order_id)?;
    if order.maker != maker {
        return Err(ContractError::OrderNotMaker);
    }
    if !order.active {
        return Err(ContractError::OrderAlreadyClosed);
    }

    let recovered = order.remaining_amount;
    order.remaining_amount = 0;
    order.active = false;
    bucket_remove(env, &order.pair, order.price_tick, order.id);
    save_order(env, &order);

    if recovered > 0 {
        let sell_client = token::Client::new(env, &order.pair.sell_asset);
        sell_client.transfer(&env.current_contract_address(), &maker, &recovered);
    }

    Ok(recovered)
}

pub fn get_order(env: &Env, order_id: u64) -> Option<LimitOrder> {
    load_order(env, order_id).ok()
}

/// List the ids of every order currently resting at `(pair, price_tick)`.
pub fn get_orders_at_tick(env: &Env, pair: AssetPair, price_tick: i128) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&OrderStorageKey::Bucket(pair, price_tick))
        .unwrap_or_else(|| Vec::new(env))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn setup() -> (Env, crate::TimeLockedUpgradeContractClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, crate::TimeLockedUpgradeContract);
        let client = crate::TimeLockedUpgradeContractClient::new(&env, &contract_id);
        let sell_issuer = Address::generate(&env);
        let buy_issuer = Address::generate(&env);
        let sell_asset = env.register_stellar_asset_contract(sell_issuer);
        let buy_asset = env.register_stellar_asset_contract(buy_issuer);
        (env, client, sell_asset, buy_asset)
    }

    fn mint(env: &Env, asset: &Address, to: &Address, amount: i128) {
        soroban_sdk::token::StellarAssetClient::new(env, asset).mint(to, &amount);
    }

    #[test]
    fn place_order_locks_sell_asset() {
        let (env, client, sell_asset, buy_asset) = setup();
        let maker = Address::generate(&env);
        mint(&env, &sell_asset, &maker, 1_000);
        let pair = AssetPair { sell_asset: sell_asset.clone(), buy_asset };
        let order = client.place_limit_order(&maker, &pair, &(2 * PRICE_SCALE), &1_000);
        assert_eq!(order.remaining_amount, 1_000);
        let token_client = soroban_sdk::token::Client::new(&env, &sell_asset);
        assert_eq!(token_client.balance(&maker), 0);
    }

    #[test]
    fn partial_fill_keeps_order_open_with_reduced_remainder() {
        let (env, client, sell_asset, buy_asset) = setup();
        let maker = Address::generate(&env);
        let filler = Address::generate(&env);
        mint(&env, &sell_asset, &maker, 1_000);
        mint(&env, &buy_asset, &filler, 10_000);
        let pair = AssetPair { sell_asset: sell_asset.clone(), buy_asset: buy_asset.clone() };
        let order = client.place_limit_order(&maker, &pair, &(2 * PRICE_SCALE), &1_000);

        let result = client.fill_limit_order(&filler, &order.id, &400);
        assert_eq!(result.remaining_amount, 600);
        assert!(!result.order_closed);
        assert_eq!(result.paid_amount, 800); // 400 * 2

        let sell_client = soroban_sdk::token::Client::new(&env, &sell_asset);
        assert_eq!(sell_client.balance(&filler), 400);
        let buy_client = soroban_sdk::token::Client::new(&env, &buy_asset);
        assert_eq!(buy_client.balance(&maker), 800);
    }

    #[test]
    fn full_fill_closes_order_and_clears_book_bucket() {
        let (env, client, sell_asset, buy_asset) = setup();
        let maker = Address::generate(&env);
        let filler = Address::generate(&env);
        mint(&env, &sell_asset, &maker, 500);
        mint(&env, &buy_asset, &filler, 5_000);
        let pair = AssetPair { sell_asset, buy_asset };
        let order = client.place_limit_order(&maker, &pair, &PRICE_SCALE, &500);

        let result = client.fill_limit_order(&filler, &order.id, &500);
        assert!(result.order_closed);
        assert_eq!(client.get_orders_at_tick(&pair, &PRICE_SCALE).len(), 0);
    }

    #[test]
    fn maker_can_cancel_and_recover_locked_assets() {
        let (env, client, sell_asset, buy_asset) = setup();
        let maker = Address::generate(&env);
        mint(&env, &sell_asset, &maker, 1_000);
        let pair = AssetPair { sell_asset: sell_asset.clone(), buy_asset };
        let order = client.place_limit_order(&maker, &pair, &PRICE_SCALE, &1_000);

        let recovered = client.cancel_limit_order(&maker, &order.id);
        assert_eq!(recovered, 1_000);
        let sell_client = soroban_sdk::token::Client::new(&env, &sell_asset);
        assert_eq!(sell_client.balance(&maker), 1_000);
    }

    #[test]
    fn non_maker_cannot_cancel_order() {
        let (env, client, sell_asset, buy_asset) = setup();
        let maker = Address::generate(&env);
        let attacker = Address::generate(&env);
        mint(&env, &sell_asset, &maker, 1_000);
        let pair = AssetPair { sell_asset, buy_asset };
        let order = client.place_limit_order(&maker, &pair, &PRICE_SCALE, &1_000);

        let result = client.try_cancel_limit_order(&attacker, &order.id);
        assert_eq!(result, Err(Ok(ContractError::OrderNotMaker)));
    }

    #[test]
    fn fill_exceeding_remaining_amount_fails() {
        let (env, client, sell_asset, buy_asset) = setup();
        let maker = Address::generate(&env);
        let filler = Address::generate(&env);
        mint(&env, &sell_asset, &maker, 100);
        mint(&env, &buy_asset, &filler, 10_000);
        let pair = AssetPair { sell_asset, buy_asset };
        let order = client.place_limit_order(&maker, &pair, &PRICE_SCALE, &100);

        let result = client.try_fill_limit_order(&filler, &order.id, &200);
        assert_eq!(result, Err(Ok(ContractError::OrderInsufficientRemaining)));
    }

    #[test]
    fn cancelled_order_cannot_be_filled() {
        let (env, client, sell_asset, buy_asset) = setup();
        let maker = Address::generate(&env);
        let filler = Address::generate(&env);
        mint(&env, &sell_asset, &maker, 100);
        mint(&env, &buy_asset, &filler, 1_000);
        let pair = AssetPair { sell_asset, buy_asset };
        let order = client.place_limit_order(&maker, &pair, &PRICE_SCALE, &100);
        client.cancel_limit_order(&maker, &order.id);

        let result = client.try_fill_limit_order(&filler, &order.id, &10);
        assert_eq!(result, Err(Ok(ContractError::OrderAlreadyClosed)));
    }
}
