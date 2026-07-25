use soroban_sdk::{contracttype, symbol_short, Address, BytesN, Env, Map, Symbol, Vec};
use crate::{ContractData, ContractError, DATA_KEY, SIGNERS_KEY};

const BALLOT_TTL_LEDGERS: u32 = 17_280;
const BALLOT_TTL_THRESHOLD: u32 = 5_000;

pub(crate) const GOVERNANCE_UPGRADE_KEY: Symbol = symbol_short!("GOVUPG");
pub(crate) const GOVERNANCE_CONFIG_KEY: Symbol = symbol_short!("GVNCFG");

#[contracttype]
#[derive(Clone)]
pub struct GovernanceConfig {
    pub quorum_threshold: u32,
}

impl Default for GovernanceConfig {
    fn default() -> Self {
        Self { quorum_threshold: 2 }
    }
}

pub fn get_governance_config(env: &Env) -> GovernanceConfig {
    env.storage()
        .instance()
        .get(&GOVERNANCE_CONFIG_KEY)
        .unwrap_or_default()
}

pub fn set_governance_config(env: &Env, config: &GovernanceConfig) {
    env.storage().instance().set(&GOVERNANCE_CONFIG_KEY, config);
}

pub fn verify_upgrade_quorum(env: &Env, signers: &Vec<Address>) -> Result<(), ContractError> {
    let config = get_governance_config(env);
    let data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;
    let authorized_signers: Map<Address, ()> = env
        .storage()
        .instance()
        .get(&SIGNERS_KEY)
        .unwrap_or_else(|| Map::new(env));

    let mut valid_count: u32 = 0;
    for signer in signers.iter() {
        if signer == data.admin || authorized_signers.contains_key(signer.clone()) {
            valid_count += 1;
        }
    }

    if valid_count < config.quorum_threshold {
        return Err(ContractError::ThresholdNotReached);
    }
    Ok(())
}

#[contracttype]
#[derive(Clone)]
pub struct StagedUpgrade {
    pub new_wasm_hash: BytesN<32>,
    pub proposer: Address,
    pub staged_at: u64,
}

#[contracttype]
#[derive(Clone)]
pub struct GovernanceUpgradeProposal {
    pub new_wasm_hash: BytesN<32>,
    pub proposer: Address,
    pub staged_at: u64,
    pub signers: Vec<Address>,
}

pub fn verify_staged_delay(staged_at: u64, current_time: u64, delay_seconds: u64) -> bool {
    current_time.saturating_sub(staged_at) >= delay_seconds
}

#[contracttype]
pub enum BallotKey {
    Proposal(Symbol),
}

#[contracttype]
#[derive(Clone)]
pub struct VotingBallot {
    pub target: Address,
    pub replacement: Address,
    pub proposer: Address,
    pub proposed_at: u64,
    pub votes: Map<Address, ()>,
}

pub fn open_ballot(
    env: &Env,
    proposal_id: Symbol,
    target: Address,
    replacement: Address,
    proposer: Address,
) -> Result<(), ContractError> {
    let key = BallotKey::Proposal(proposal_id);
    if env.storage().temporary().has(&key) {
        return Err(ContractError::ProposalAlreadyActive);
    }
    let ballot = VotingBallot {
        target,
        replacement,
        proposer,
        proposed_at: env.ledger().timestamp(),
        votes: Map::new(env),
    };
    env.storage().temporary().set(&key, &ballot);
    env.storage().temporary().extend_ttl(&key, BALLOT_TTL_THRESHOLD, BALLOT_TTL_LEDGERS);
    Ok(())
}

pub fn cast_vote(
    env: &Env,
    proposal_id: Symbol,
    voter: Address,
) -> Result<VotingBallot, ContractError> {
    let key = BallotKey::Proposal(proposal_id);
    let mut ballot: VotingBallot = env
        .storage()
        .temporary()
        .get(&key)
        .ok_or(ContractError::NoActiveProposal)?;
    if ballot.votes.contains_key(voter.clone()) {
        return Err(ContractError::AlreadyVoted);
    }
    ballot.votes.set(voter, ());
    env.storage().temporary().set(&key, &ballot);
    env.storage().temporary().extend_ttl(&key, BALLOT_TTL_THRESHOLD, BALLOT_TTL_LEDGERS);
    Ok(ballot)
}

pub fn get_ballot(env: &Env, proposal_id: Symbol) -> Option<VotingBallot> {
    env.storage().temporary().get(&BallotKey::Proposal(proposal_id))
}

pub fn close_ballot(env: &Env, proposal_id: Symbol) {
    env.storage().temporary().remove(&BallotKey::Proposal(proposal_id));
}

pub fn verify_block_height(target_height: u32, active_index: u32) -> bool {
    target_height > active_index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_block_height() {
        assert!(verify_block_height(101, 100));
        assert!(!verify_block_height(100, 100));
        assert!(!verify_block_height(99, 100));
    }
}
