#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short,
    Address, Bytes, BytesN, Env, Map, Symbol, Vec,
};

/// Numeric asset identifier for gas-optimized storage.
pub type AssetId = u32;

/// Convert a currency Symbol to a numeric AssetId using FNV-1a hash.
pub fn symbol_to_asset_id(symbol: &Symbol) -> AssetId {
    let payload = symbol.to_val().get_payload();
    let mut hash: u32 = 2166136261u32;
    for i in 0..8u64 {
        let byte = ((payload >> (i * 8)) & 0xff) as u8;
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

const ID_NGN: u32 = symbol_to_asset_id_const(b"NGN");
const ID_GHS: u32 = symbol_to_asset_id_const(b"GHS");
const ID_CFA: u32 = symbol_to_asset_id_const(b"CFA");
const ID_KES: u32 = symbol_to_asset_id_const(b"KES");
const ID_ZAR: u32 = symbol_to_asset_id_const(b"ZAR");
const ID_UGX: u32 = symbol_to_asset_id_const(b"UGX");

const fn symbol_to_asset_id_const(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 2166136261u32;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(16777619);
        i += 1;
    }
    hash
}

/// Reverse lookup from AssetId to Symbol.
pub fn asset_id_to_symbol(asset_id: u32) -> Symbol {
    match asset_id {
        _ if asset_id == ID_NGN => symbol_short!("NGN"),
        _ if asset_id == ID_GHS => symbol_short!("GHS"),
        _ if asset_id == ID_CFA => symbol_short!("CFA"),
        _ if asset_id == ID_KES => symbol_short!("KES"),
        _ if asset_id == ID_ZAR => symbol_short!("ZAR"),
        _ if asset_id == ID_UGX => symbol_short!("UGX"),
        _ => panic!("Unknown asset ID mapping context"),
    }
}

pub(crate) mod nonce;
use crate::nonce::{consume_nonce, get_nonce};

pub mod admin;
pub mod auth;
pub mod config;
pub use config::{get_price_variance_config, set_price_variance_config, PriceVarianceConfig};
pub mod consensus;
pub mod fees;
pub mod governance;
pub mod math;
pub mod slashing;
pub mod staking_tiers;
pub mod storage;
pub mod temp_governance;
pub mod validation;

use crate::governance::{
    cast_vote, close_ballot, open_ballot, verify_staged_delay, StagedUpgrade, VotingBallot,
};
use crate::validation::{check_bond_capacity, check_liquidity_depth, validate_telemetry_submission};
pub use staking_tiers::{AssetFeedMetrics, StakingTier, StakingTierConfig};
use staking_tiers::{assign_tier, effective_volume_score, required_stake_for_tier, validate_tier_config};
use slashing::{
    apply_escrow_penalty, get_fault_count_in_window, get_penalty_multiplier,
    record_tracking_fault, IngestionPenaltyResult,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NotAdmin = 3,
    NoPendingUpgrade = 4,
    UpgradeTimelockNotSatisfied = 5,
    InvalidHeartbeatInterval = 6,
    InvalidNonce = 7,
    AlreadyRegistered = 8,
    NotRegistered = 9,
    InvalidStakeAmount = 10,
    Overflow = 11,
    Unauthorized = 12,
    TargetNotAdmin = 13,
    ProposalAlreadyActive = 14,
    NoActiveProposal = 15,
    AlreadyVoted = 16,
    ThresholdNotReached = 17,
    SignatureExpired = 18,
    InvalidSaltSignature = 19,
    InsufficientStakeForTier = 20,
    InvalidTierConfig = 21,
    FeedAlreadyRegistered = 22,
    PremiumPoolAccessDenied = 23,
    TransferAlreadyPending = 24,
    NoPendingOwner = 25,
    FeeCeilingExceeded = 26,
    DivisionByZero = 27,
    InvalidVarianceConfig = 28,
    ContractPaused = 29,
    RevokedAddress = 30,
    EmergencyRevocationAlreadyActive = 31,
    NoActiveEmergencyRevocation = 32,
    StaleTelemetryPayload = 33,
    InsufficientReserveBalance = 34,
    InsufficientVolume = 35,
    StaleSequence = 36,
    InsufficientLiquidityDepth = 37,
}

// Contract state keys
pub(crate) const DATA_KEY: Symbol = symbol_short!("DATA");
pub(crate) const SIGNERS_KEY: Symbol = symbol_short!("SIGNERS");
const PENDING_UPGRADE_KEY: Symbol = symbol_short!("PENDING");
pub(crate) const UPGRADE_DELAY_SECONDS: u64 = 48 * 60 * 60;
pub(crate) const STAKE_REGISTRY_KEY: Symbol = symbol_short!("STAKES");
pub(crate) const TOTAL_STAKED_KEY: Symbol = symbol_short!("TOTAL");
const HEARTBEAT_KEY: Symbol = symbol_short!("HBEAT");
const HB_INTERVAL_KEY: Symbol = symbol_short!("HBINTV");
pub(crate) const DEFAULT_HEARTBEAT_INTERVAL: u64 = 5 * 60;
pub(crate) const VALIDATOR_STATE_KEY: Symbol = symbol_short!("VLSTATE");
pub(crate) const REVOKED_SIGNER_KEY: Symbol = symbol_short!("REVOKED");
const NODE_PROFILES_KEY: Symbol = symbol_short!("NODES");
const PLATFORM_CAPITAL_KEY: Symbol = symbol_short!("CAPITAL");
const CONSENSUS_CACHE_KEY: Symbol = symbol_short!("CACHE");
const RELAYER_TTL_THRESHOLD: u32 = 5_000;
const INSTANCE_TTL_EXTEND: u32 = 100_000;
const TREASURY_KEY: Symbol = symbol_short!("TREASURY");
const SEQUENCE_COUNTER_KEY: Symbol = symbol_short!("SEQCTR");
const REVOCATION_KEY: Symbol = symbol_short!("REVOKE");

#[contracttype]
#[derive(Clone)]
pub struct RevocationProposal {
    pub target: Address,
    pub replacement: Address,
    pub proposer: Address,
    pub proposed_at: u64,
    pub votes: Vec<Address>,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ContractData {
    pub admin: Address,
    pub value: u64,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct StakeRecord {
    pub node: Address,
    pub amount: u64,
    pub registered_at: u64,
}

#[contracttype]
#[derive(Clone)]
pub struct NodeProfile {
    pub node: Address,
    pub rate: u64,
    pub confidence: u32,
    pub updated_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct FeedStakeRecord {
    pub node: Address,
    pub asset: AssetId,
    pub amount: u64,
    pub tier: StakingTier,
    pub registered_at: u64,
}

#[contracttype]
pub enum StakingStorageKey {
    TierConfig,
    AssetMetrics(AssetId),
    FeedStake(Address, AssetId),
}

// Storage key newtype wrappers
#[contracttype] pub struct StakeKey(pub Address);
#[contracttype] pub struct SignerKey(pub Address);
#[contracttype] pub struct NodeProfileKey(pub Address);
#[contracttype] pub struct HeartbeatKey(pub AssetId);
#[contracttype] pub struct CorridorFeeKey(pub Symbol);

// CorridorFeePool (used by add_corridor_fees before fees module delegation)
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CorridorFeePool {
    pub asset: AssetId,
    pub collected: u64,
    pub variable_pool: u64,
}

// AssetMetrics key wrapper
#[contracttype] pub struct AssetMetricsKey(pub AssetId);

#[contract]
pub struct TimeLockedUpgradeContract;

#[contractimpl]
impl TimeLockedUpgradeContract {
    pub fn initialize(env: Env, admin: Address, treasury: Address) -> Result<(), ContractError> {
        if env.storage().instance().has(&DATA_KEY) {
            return Err(ContractError::AlreadyInitialized);
        }
        admin.require_auth();
        let data = ContractData { admin: admin.clone(), value: 0 };
        env.storage().instance().set(&DATA_KEY, &data);
        env.storage().instance().set(&TREASURY_KEY, &treasury);
        Ok(())
    }

    pub fn stake_and_register(env: Env, node: Address, amount: u64) -> Result<StakeRecord, ContractError> {
        if amount == 0 { return Err(ContractError::InvalidStakeAmount); }
        admin::assert_not_revoked(&env, &node)?;
        node.require_auth();
        let stake_key = StakeKey(node.clone());
        if env.storage().instance().has(&stake_key) { return Err(ContractError::AlreadyRegistered); }
        let total: u64 = env.storage().instance().get(&TOTAL_STAKED_KEY).unwrap_or(0u64);
        let new_total = total.checked_add(amount).ok_or(ContractError::Overflow)?;
        env.storage().instance().set(&stake_key, &amount);
        env.storage().instance().set(&TOTAL_STAKED_KEY, &new_total);
        Self::_record_heartbeat(&env, 0u32);
        Ok(StakeRecord { node, amount, registered_at: env.ledger().timestamp() })
    }

    pub fn unstake(env: Env, node: Address) -> Result<u64, ContractError> {
        node.require_auth();
        let stake_key = StakeKey(node.clone());
        let amount: u64 = env.storage().instance().get(&stake_key).ok_or(ContractError::NotRegistered)?;
        let total: u64 = env.storage().instance().get(&TOTAL_STAKED_KEY).unwrap_or(0u64);
        let new_total = total.saturating_sub(amount);
        env.storage().instance().remove(&stake_key);
        env.storage().instance().set(&TOTAL_STAKED_KEY, &new_total);
        Ok(amount)
    }

    pub fn remove_signer(env: Env, signer: Address, caller: Address) -> Result<(), ContractError> {
        Self::assert_contract_is_active(&env)?;
        let data = Self::_load_data(&env)?;
        if data.admin != caller { return Err(ContractError::NotAdmin); }
        caller.require_auth();
        let signer_key = SignerKey(signer.clone());
        if env.storage().instance().has(&signer_key) {
            env.storage().instance().remove(&signer_key);
            let count: u32 = env.storage().instance().get(&SIGNERS_KEY).unwrap_or(0u32);
            if count > 0 { env.storage().instance().set(&SIGNERS_KEY, &(count - 1)); }
        }
        Self::_extend_instance_ttl(&env);
        Ok(())
    }

    pub fn propose_revocation(
        env: Env, proposer: Address, target: Address, replacement: Address, sig_expires_at: u64,
    ) -> Result<(), ContractError> {
        if env.ledger().timestamp() > sig_expires_at { return Err(ContractError::SignatureExpired); }
        admin::assert_not_revoked(&env, &proposer)?;
        proposer.require_auth();
        let data = Self::get_data(env.clone())?;
        if !Self::_is_signer(&env, &proposer) && data.admin != proposer {
            return Err(ContractError::Unauthorized);
        }
        open_ballot(&env, REVOCATION_KEY, target, replacement, proposer)
    }

    pub fn vote_revocation(env: Env, voter: Address, sig_expires_at: u64) -> Result<(), ContractError> {
        if env.ledger().timestamp() > sig_expires_at { return Err(ContractError::SignatureExpired); }
        voter.require_auth();
        let data = Self::_load_data(&env)?;
        if !Self::_is_signer(&env, &voter) && data.admin != voter {
            return Err(ContractError::Unauthorized);
        }
        let ballot = cast_vote(&env, REVOCATION_KEY, voter)?;
        let threshold = Self::_revocation_threshold(&env);
        if ballot.votes.len() >= threshold {
            let mut contract_data = data;
            contract_data.admin = ballot.replacement.clone();
            env.storage().instance().set(&DATA_KEY, &contract_data);
            close_ballot(&env, REVOCATION_KEY);
        }
        Ok(())
    }

    pub fn get_revocation_ballot(env: Env) -> Option<VotingBallot> {
        governance::get_ballot(&env, REVOCATION_KEY)
    }

    fn _load_data(env: &Env) -> Result<ContractData, ContractError> {
        env.storage().instance().get(&DATA_KEY).ok_or(ContractError::NotInitialized)
    }

    pub fn get_data(env: Env) -> Result<ContractData, ContractError> {
        Self::_load_data(&env)
    }

    pub fn propose_upgrade(
        env: Env, new_wasm_hash: BytesN<32>, proposer: Address,
        nonce: u64, salt: Bytes, salt_signature: BytesN<32>, sig_expires_at: u64,
    ) -> Result<(), ContractError> {
        if env.ledger().timestamp() > sig_expires_at { return Err(ContractError::SignatureExpired); }
        let data = Self::_load_data(&env)?;
        if data.admin != proposer { return Err(ContractError::NotAdmin); }
        proposer.require_auth();
        consume_nonce(&env, &proposer, nonce, salt, salt_signature)?;
        let staged = StagedUpgrade { new_wasm_hash, proposer, staged_at: env.ledger().timestamp() };
        env.storage().instance().set(&PENDING_UPGRADE_KEY, &staged);
        Ok(())
    }

    pub fn execute_upgrade(
        env: Env, executor: Address,
        nonce: u64, salt: Bytes, signature: BytesN<32>, sig_expires_at: u64,
    ) -> Result<(), ContractError> {
        if env.ledger().timestamp() > sig_expires_at { return Err(ContractError::SignatureExpired); }
        let data = Self::_load_data(&env)?;
        if data.admin != executor { return Err(ContractError::NotAdmin); }
        executor.require_auth();
        consume_nonce(&env, &executor, nonce, salt, signature)?;
        let pending: StagedUpgrade = env.storage().instance()
            .get(&PENDING_UPGRADE_KEY)
            .ok_or(ContractError::NoPendingUpgrade)?;
        if !verify_staged_delay(pending.staged_at, env.ledger().timestamp(), UPGRADE_DELAY_SECONDS) {
            return Err(ContractError::UpgradeTimelockNotSatisfied);
        }
        env.deployer().update_current_contract_wasm(pending.new_wasm_hash);
        env.storage().instance().remove(&PENDING_UPGRADE_KEY);
        Self::_extend_instance_ttl(&env);
        Ok(())
    }

    pub fn get_pending_upgrade(env: Env) -> Option<StagedUpgrade> {
        env.storage().instance().get(&PENDING_UPGRADE_KEY)
    }

    pub fn get_upgrade_timelock_remaining(env: Env) -> Option<u64> {
        env.storage().instance().get(&PENDING_UPGRADE_KEY).map(|staged: StagedUpgrade| {
            let elapsed = env.ledger().timestamp().saturating_sub(staged.staged_at);
            UPGRADE_DELAY_SECONDS.saturating_sub(elapsed)
        })
    }

    pub fn cancel_upgrade(env: Env, canceller: Address) -> Result<(), ContractError> {
        let data = Self::_load_data(&env)?;
        if data.admin != canceller { return Err(ContractError::NotAdmin); }
        canceller.require_auth();
        env.storage().instance().remove(&PENDING_UPGRADE_KEY);
        Self::_extend_instance_ttl(&env);
        Ok(())
    }

    pub fn set_value(
        env: Env, new_value: u64, caller: Address,
        nonce: u64, salt: Bytes, signature: BytesN<32>, sig_expires_at: u64,
    ) -> Result<(), ContractError> {
        if env.ledger().timestamp() > sig_expires_at { return Err(ContractError::SignatureExpired); }
        let mut data = Self::_load_data(&env)?;
        if data.admin != caller { return Err(ContractError::NotAdmin); }
        caller.require_auth();
        consume_nonce(&env, &caller, nonce, salt, signature)?;
        data.value = new_value;
        env.storage().instance().set(&DATA_KEY, &data);
        Self::_record_heartbeat(&env, 1u32);
        Ok(())
    }

    pub fn get_coordinator_nonce(env: Env, coordinator: Address) -> u64 {
        get_nonce(&env, &coordinator)
    }

    pub fn get_last_update_timestamp(env: Env, asset: Symbol) -> Option<u64> {
        let asset_id = symbol_to_asset_id(&asset);
        let heartbeat_key = HeartbeatKey(asset_id);
        env.storage().temporary().get(&heartbeat_key)
    }

    pub fn get_heartbeat_interval(env: Env) -> u64 {
        Self::_get_interval(&env)
    }

    pub fn set_heartbeat_interval(env: Env, interval: u64, admin: Address) -> Result<(), ContractError> {
        if interval == 0 { return Err(ContractError::InvalidHeartbeatInterval); }
        let data = Self::_load_data(&env)?;
        if data.admin != admin { return Err(ContractError::NotAdmin); }
        admin.require_auth();
        env.storage().instance().set(&HB_INTERVAL_KEY, &interval);
        Self::_extend_instance_ttl(&env);
        Ok(())
    }

    pub fn get_stake(env: Env, node: Address) -> u64 {
        let stake_key = StakeKey(node);
        env.storage().instance().get(&stake_key).unwrap_or(0u64)
    }

    pub fn get_total_staked(env: Env) -> u64 {
        env.storage().instance().get(&TOTAL_STAKED_KEY).unwrap_or(0u64)
    }

    pub fn update_heartbeat(env: Env, asset: AssetId, updater: Address) -> Result<(), ContractError> {
        let data = Self::_load_data(&env)?;
        if data.admin != updater { return Err(ContractError::NotAdmin); }
        updater.require_auth();
        check_liquidity_depth(&env, asset)?;
        Self::_record_heartbeat(&env, asset);
        Self::_extend_instance_ttl(&env);
        Ok(())
    }

    pub fn is_data_fresh(env: Env, asset: AssetId) -> bool {
        let heartbeat_key = HeartbeatKey(asset);
        if let Some(last_update) = env.storage().temporary().get::<_, u64>(&heartbeat_key) {
            env.ledger().timestamp().saturating_sub(last_update) <= Self::_get_interval(&env)
        } else {
            false
        }
    }

    pub fn upsert_node_profile(env: Env, admin: Address, node: Address, rate: u64, confidence: u32) -> Result<(), ContractError> {
        let data = Self::_load_data(&env)?;
        if data.admin != admin { return Err(ContractError::NotAdmin); }
        admin.require_auth();
        let profile_key = NodeProfileKey(node.clone());
        let profile = NodeProfile { node, rate, confidence, updated_at: env.ledger().timestamp() };
        env.storage().persistent().set(&profile_key, &profile);
        Self::_extend_instance_ttl(&env);
        Ok(())
    }

    pub fn get_latest_rate(env: Env, node: Address) -> Result<u64, ContractError> {
        Self::_maintain_relayer_profile_ttl(&env);
        let profile_key = NodeProfileKey(node);
        let profile: NodeProfile = env.storage().persistent().get(&profile_key)
            .ok_or(ContractError::NotRegistered)?;
        Self::_scan_profile_for_rate(profile).ok_or(ContractError::NotRegistered)
    }

    pub fn add_corridor_fees(env: Env, asset: AssetId, collected: u64, variable_fee: u64) -> Result<CorridorFeePool, ContractError> {
        let fee_key = CorridorFeeKey(asset_id_to_symbol(asset));
        let mut pool: CorridorFeePool = env.storage().persistent().get(&fee_key)
            .unwrap_or(CorridorFeePool { asset, collected: 0, variable_pool: 0 });
        pool.collected = pool.collected.checked_add(collected).ok_or(ContractError::Overflow)?;
        pool.variable_pool = pool.variable_pool.checked_add(variable_fee).ok_or(ContractError::Overflow)?;
        env.storage().persistent().set(&fee_key, &pool);
        Ok(pool)
    }

    pub fn get_corridor_fee_pool(env: Env, asset: AssetId) -> fees::CorridorFeePool {
        fees::get_corridor_fee_pool(env, asset)
    }

    pub fn set_corridor_weight(
        env: Env, admin: Address, asset: AssetId, base_weight: u64, dynamic_weight: u64,
    ) -> Result<fees::CorridorWeightProfile, ContractError> {
        let profile = fees::set_corridor_weight(env.clone(), admin, asset, base_weight, dynamic_weight)?;
        Self::_extend_instance_ttl(&env);
        Ok(profile)
    }

    pub fn get_corridor_weight(env: Env, asset: AssetId) -> fees::CorridorWeightProfile {
        fees::get_corridor_weight(env, asset)
    }

    pub fn set_staking_tier_config(
        env: Env, admin: Address, config: StakingTierConfig, signers: Vec<Address>,
    ) -> Result<(), ContractError> {
        let data = Self::_load_data(&env)?;
        if data.admin != admin { return Err(ContractError::NotAdmin); }
        admin.require_auth();
        crate::auth::require_multisig(&env, &signers)?;
        validate_tier_config(&config)?;
        env.storage().instance().set(&StakingStorageKey::TierConfig, &config);
        Self::_extend_instance_ttl(&env);
        Ok(())
    }

    pub fn get_staking_tier_config(env: Env) -> StakingTierConfig {
        env.storage().instance().get(&StakingStorageKey::TierConfig).unwrap_or_default()
    }

    pub fn set_asset_feed_metrics(
        env: Env, admin: Address, asset: AssetId,
        volume_score_floor: u32, volatility_bps: u32, signers: Vec<Address>,
    ) -> Result<AssetFeedMetrics, ContractError> {
        let data = Self::_load_data(&env)?;
        if data.admin != admin { return Err(ContractError::NotAdmin); }
        admin.require_auth();
        crate::auth::require_multisig(&env, &signers)?;
        let metrics = AssetFeedMetrics {
            volume_score: volume_score_floor.min(100),
            volatility_bps,
        };
        env.storage().persistent().set(&StakingStorageKey::AssetMetrics(asset), &metrics);
        Self::_extend_instance_ttl(&env);
        Ok(metrics)
    }

    pub fn get_asset_feed_metrics(env: Env, asset: AssetId) -> AssetFeedMetrics {
        Self::_resolve_feed_metrics(&env, asset)
    }

    pub fn get_staking_tier(env: Env, asset: AssetId) -> StakingTier {
        assign_tier(&Self::_resolve_feed_metrics(&env, asset))
    }

    pub fn get_required_stake(env: Env, asset: AssetId) -> u64 {
        let tier = Self::get_staking_tier(env.clone(), asset);
        let config = Self::get_staking_tier_config(env);
        required_stake_for_tier(tier, &config)
    }

    pub fn stake_and_register_for_feed(
        env: Env, node: Address, asset: AssetId, amount: u64,
    ) -> Result<FeedStakeRecord, ContractError> {
        if amount == 0 { return Err(ContractError::InvalidStakeAmount); }
        admin::assert_not_revoked(&env, &node)?;
        node.require_auth();
        let feed_key = StakingStorageKey::FeedStake(node.clone(), asset);
        if env.storage().persistent().has(&feed_key) { return Err(ContractError::FeedAlreadyRegistered); }
        let tier = Self::get_staking_tier(env.clone(), asset);
        let required = Self::get_required_stake(env.clone(), asset);
        if amount < required { return Err(ContractError::InsufficientStakeForTier); }
        let stake_val = storage::FeedStakeValue { amount, last_active: env.ledger().timestamp() };
        env.storage().persistent().set(&feed_key, &stake_val);
        env.storage().persistent().extend_ttl(&feed_key, storage::RENT_THRESHOLD, storage::RENT_EXTEND_TO);
        let stake_key = StakeKey(node.clone());
        let node_total: u64 = env.storage().instance().get(&stake_key).unwrap_or(0);
        let new_node_total = node_total.checked_add(amount).ok_or(ContractError::Overflow)?;
        env.storage().instance().set(&stake_key, &new_node_total);
        let total: u64 = env.storage().instance().get(&TOTAL_STAKED_KEY).unwrap_or(0u64);
        let new_total = total.checked_add(amount).ok_or(ContractError::Overflow)?;
        env.storage().instance().set(&TOTAL_STAKED_KEY, &new_total);
        Self::_record_heartbeat(&env, asset);
        Ok(FeedStakeRecord { node, asset, amount, tier, registered_at: env.ledger().timestamp() })
    }

    pub fn unstake_from_feed(env: Env, node: Address, asset: AssetId) -> Result<u64, ContractError> {
        node.require_auth();
        let feed_key = StakingStorageKey::FeedStake(node.clone(), asset);
        let stake_val: storage::FeedStakeValue = env.storage().persistent()
            .get(&feed_key).ok_or(ContractError::NotRegistered)?;
        let amount = stake_val.amount;
        env.storage().persistent().remove(&feed_key);
        let stake_key = StakeKey(node.clone());
        let node_total: u64 = env.storage().instance().get(&stake_key).unwrap_or(0);
        let new_node_total = node_total.saturating_sub(amount);
        if new_node_total == 0 {
            env.storage().instance().remove(&stake_key);
        } else {
            env.storage().instance().set(&stake_key, &new_node_total);
        }
        let total: u64 = env.storage().instance().get(&TOTAL_STAKED_KEY).unwrap_or(0u64);
        env.storage().instance().set(&TOTAL_STAKED_KEY, &total.saturating_sub(amount));
        Ok(amount)
    }

    pub fn get_feed_stake(env: Env, node: Address, asset: AssetId) -> u64 {
        storage::check_and_prune_feed_stake(&env, node.clone(), asset);
        let feed_key = StakingStorageKey::FeedStake(node, asset);
        env.storage().persistent().get::<_, storage::FeedStakeValue>(&feed_key)
            .map(|v| v.amount).unwrap_or(0)
    }

    pub fn set_platform_capital(env: Env, capital: u64) {
        env.storage().instance().set(&PLATFORM_CAPITAL_KEY, &capital);
    }

    pub fn finalize_consensus(env: Env) {
        env.storage().temporary().remove(&CONSENSUS_CACHE_KEY);
        env.storage().temporary().remove(&HEARTBEAT_KEY);
        close_ballot(&env, REVOCATION_KEY);
    }

    pub fn register_signer(env: Env, signer: Address, caller: Address) -> Result<(), ContractError> {
        let data = Self::_load_data(&env)?;
        if data.admin != caller { return Err(ContractError::NotAdmin); }
        caller.require_auth();
        let signer_key = SignerKey(signer.clone());
        if !env.storage().instance().has(&signer_key) {
            env.storage().instance().set(&signer_key, &true);
            let count: u32 = env.storage().instance().get(&SIGNERS_KEY).unwrap_or(0u32);
            env.storage().instance().set(&SIGNERS_KEY, &(count + 1));
        }
        Self::_extend_instance_ttl(&env);
        Ok(())
    }

    // --- Admin Ownership Transfer (Issue #429) ---

    pub fn propose_ownership_transfer(env: Env, current_admin: Address, nominee: Address) -> Result<(), ContractError> {
        crate::admin::propose_ownership_transfer(&env, current_admin, nominee)
    }

    pub fn claim_ownership(env: Env, claimer: Address) -> Result<(), ContractError> {
        crate::admin::claim_ownership(&env, claimer)
    }

    // --- Two-Phase Admin Key Change (Issue #493) ---

    pub fn propose_admin_change(env: Env, current_admin: Address, new_admin: Address) -> Result<(), ContractError> {
        crate::admin::propose_admin_change(&env, current_admin, new_admin)
    }

    pub fn countersign_admin_change(env: Env, cosigner: Address) -> Result<(), ContractError> {
        crate::admin::countersign_admin_change(&env, cosigner)
    }

    pub fn execute_admin_change_by_timelock(env: Env, executor: Address) -> Result<(), ContractError> {
        crate::admin::execute_admin_change_by_timelock(&env, executor)
    }

    pub fn cancel_admin_change(env: Env, canceller: Address) -> Result<(), ContractError> {
        crate::admin::cancel_admin_change(&env, canceller)
    }

    pub fn get_pending_admin_change(env: Env) -> Option<admin::AdminChangeProposal> {
        crate::admin::get_pending_admin_change(&env)
    }

    pub fn vote_emergency_revocation(
        env: Env, voter: Address, sig_expires_at: u64, nonce: u64,
    ) -> Result<(), ContractError> {
        admin::vote_emergency_revocation(&env, voter, sig_expires_at)
    }

    pub fn get_emergency_revocation(env: Env) -> Option<admin::EmergencyRevocationProposal> {
        admin::get_emergency_revocation_proposal(&env)
    }

    pub fn purge_expired_revocation_prop(env: Env) -> Result<(), ContractError> {
        admin::purge_emergency_revocation_proposal(&env)
    }

    pub fn has_active_revocation_proposal(env: Env) -> bool {
        admin::has_active_emergency_revocation(&env)
    }

    // ── Multi-Tier Escrow Penalties (Issue #525) ──────────────────────────────

    pub fn report_ingestion_dropout(
        env: Env, admin: Address, validator: Address, asset: Symbol,
    ) -> Result<u32, ContractError> {
        Self::assert_contract_is_active(&env)?;
        let data = Self::get_data(env.clone())?;
        if data.admin != admin { return Err(ContractError::NotAdmin); }
        admin.require_auth();
        record_tracking_fault(&env, &validator, &asset)
    }

    pub fn get_ingestion_fault_count(env: Env, validator: Address, asset: Symbol) -> u32 {
        get_fault_count_in_window(&env, &validator, &asset)
    }

    pub fn get_ingestion_multiplier(env: Env, validator: Address, asset: Symbol) -> u64 {
        let fault_count = get_fault_count_in_window(&env, &validator, &asset);
        get_penalty_multiplier(fault_count)
    }

    pub fn apply_ingestion_penalty(
        env: Env, admin: Address, validator: Address, asset: Symbol, base_bond: u64,
    ) -> Result<IngestionPenaltyResult, ContractError> {
        Self::assert_contract_is_active(&env)?;
        let data = Self::get_data(env.clone())?;
        if data.admin != admin { return Err(ContractError::NotAdmin); }
        admin.require_auth();
        let fault_count = record_tracking_fault(&env, &validator, &asset)?;
        apply_escrow_penalty(
            &env, &validator, &asset, base_bond, fault_count,
            &STAKE_REGISTRY_KEY, &TOTAL_STAKED_KEY,
            &StakingStorageKey::FeedStake(validator.clone(), symbol_to_asset_id(&asset)),
        )
    }

    pub fn update_validator_profile(env: Env, node: Address, pool: Symbol) -> Result<(), ContractError> {
        admin::assert_not_revoked(&env, &node)?;
        node.require_auth();
        check_bond_capacity(&env, &node, &pool)?;
        let asset_id = symbol_to_asset_id(&pool);
        check_liquidity_depth(&env, asset_id)?;
        storage::update_feed_stake_activity(&env, node.clone(), asset_id);
        Self::_record_heartbeat(&env, asset_id);
        Ok(())
    }

    pub fn submit_telemetry_data(
        env: Env, node: Address, pool: Symbol,
        payload_timestamp: u64, reserve_a: i128, reserve_b: i128, volume_24h: i128,
    ) -> Result<(), ContractError> {
        admin::assert_not_revoked(&env, &node)?;
        node.require_auth();
        validate_telemetry_submission(&env, &node, &pool, payload_timestamp, reserve_a, reserve_b, volume_24h)?;
        Self::_record_heartbeat(&env, symbol_to_asset_id(&pool));
        env.events().publish(
            (soroban_sdk::symbol_short!("telem_ok"),),
            (node, pool, payload_timestamp),
        );
        Ok(())
    }

    // --- Private Helpers ---

    fn assert_contract_is_active(env: &Env) -> Result<(), ContractError> {
        if !env.storage().instance().has(&DATA_KEY) {
            return Err(ContractError::NotInitialized);
        }
        if admin::is_paused(env) {
            return Err(ContractError::ContractPaused);
        }
        Ok(())
    }

    fn _record_heartbeat(env: &Env, asset: AssetId) {
        let heartbeat_key = HeartbeatKey(asset);
        env.storage().temporary().set(&heartbeat_key, &env.ledger().timestamp());
    }

    fn _get_interval(env: &Env) -> u64 {
        env.storage().instance().get(&HB_INTERVAL_KEY).unwrap_or(DEFAULT_HEARTBEAT_INTERVAL)
    }

    fn _get_node_profiles(env: &Env) -> Map<Address, NodeProfile> {
        crate::storage::get_node_profiles(env)
    }

    fn _scan_profile_for_rate(profile: NodeProfile) -> Option<u64> {
        if profile.confidence == 0 { None } else { Some(profile.rate) }
    }

    fn _maintain_relayer_profile_ttl(_env: &Env) {
        // TTL managed per-entry via persistent storage; no-op placeholder.
    }

    fn _extend_instance_ttl(env: &Env) {
        env.storage().instance().extend_ttl(
            RELAYER_TTL_THRESHOLD,
            RELAYER_TTL_THRESHOLD + INSTANCE_TTL_EXTEND,
        );
    }

    fn _is_signer(env: &Env, addr: &Address) -> bool {
        let signer_key = SignerKey(addr.clone());
        env.storage().instance().has(&signer_key)
    }

    fn _revocation_threshold(env: &Env) -> u32 {
        let signer_count: u32 = env.storage().instance().get(&SIGNERS_KEY).unwrap_or(0u32);
        if signer_count == 0 { 1 } else { signer_count / 2 + 1 }
    }

    fn _resolve_feed_metrics(env: &Env, asset: AssetId) -> AssetFeedMetrics {
        let stored: AssetFeedMetrics = env.storage().persistent()
            .get(&StakingStorageKey::AssetMetrics(asset))
            .unwrap_or(AssetFeedMetrics { volume_score: 10, volatility_bps: 100 });
        let corridor = fees::get_corridor_fee_pool(env.clone(), asset);
        AssetFeedMetrics {
            volume_score: effective_volume_score(stored.volume_score, corridor.collected),
            volatility_bps: stored.volatility_bps,
        }
    }
}

#[cfg(test)]
mod query_guardrail_tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::{symbol_short, Env};

    fn setup() -> (Env, crate::TimeLockedUpgradeContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register_contract(None, TimeLockedUpgradeContract);
        let client = crate::TimeLockedUpgradeContractClient::new(&env, &id);
        (env, client)
    }

    fn advance(env: &Env, delta: u64) {
        let ts = env.ledger().timestamp();
        env.ledger().set(LedgerInfo {
            timestamp: ts + delta,
            protocol_version: env.ledger().protocol_version(),
            sequence_number: env.ledger().sequence(),
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 0,
            min_persistent_entry_ttl: 0,
            max_entry_ttl: u32::MAX,
        });
    }

    #[test]
    fn test_get_data_before_and_after_init() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        let result = client.try_get_data();
        assert_eq!(result, Err(Ok(ContractError::NotInitialized)));
        let treasury = soroban_sdk::Address::generate(&env);
        client.initialize(&admin, &treasury);
        let data = client.get_data();
        assert_eq!(data.admin, admin);
        assert_eq!(data.value, 0u64);
    }

    #[test]
    fn test_get_data_is_idempotent() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        let treasury = soroban_sdk::Address::generate(&env);
        client.initialize(&admin, &treasury);
        let first_value = client.get_data().value;
        let second_value = client.get_data().value;
        assert_eq!(first_value, second_value);
        assert_eq!(first_value, 0);
    }

    #[test]
    fn test_is_data_fresh_unknown_asset_returns_false() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        let treasury = soroban_sdk::Address::generate(&env);
        client.initialize(&admin, &treasury);
        let asset = symbol_to_asset_id(&symbol_short!("NGN"));
        assert!(!client.is_data_fresh(&asset));
    }

    #[test]
    fn test_is_data_fresh_transitions_on_staleness() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        let treasury = soroban_sdk::Address::generate(&env);
        client.initialize(&admin, &treasury);
        let asset = symbol_to_asset_id(&symbol_short!("KES"));
        client.add_corridor_fees(&asset, &crate::validation::MIN_POOL_VOLUME_DEPTH, &0u64);
        client.update_heartbeat(&asset, &admin);
        assert!(client.is_data_fresh(&asset));
        advance(&env, DEFAULT_HEARTBEAT_INTERVAL + 1);
        assert!(!client.is_data_fresh(&asset));
    }

    #[test]
    fn test_is_data_fresh_does_not_mutate_heartbeat() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        let treasury = soroban_sdk::Address::generate(&env);
        client.initialize(&admin, &treasury);
        let asset = symbol_to_asset_id(&symbol_short!("GHS"));
        client.add_corridor_fees(&asset, &crate::validation::MIN_POOL_VOLUME_DEPTH, &0u64);
        client.update_heartbeat(&asset, &admin);
        for _ in 0..5 { assert!(client.is_data_fresh(&asset)); }
        advance(&env, DEFAULT_HEARTBEAT_INTERVAL + 1);
        assert!(!client.is_data_fresh(&asset));
    }

    #[test]
    fn test_query_methods_do_not_interfere() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        let treasury = soroban_sdk::Address::generate(&env);
        client.initialize(&admin, &treasury);
        let asset = symbol_to_asset_id(&symbol_short!("CFA"));
        let value_before = client.get_data().value;
        let _ = client.is_data_fresh(&asset);
        let value_after = client.get_data().value;
        assert_eq!(value_before, value_after);
    }
}

// #[cfg(test)]
// mod test;
