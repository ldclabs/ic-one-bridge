use super::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct State {
    pub key_name: String,
    pub icp_address: Principal,
    pub evm_address: Address,
    #[serde(default)]
    pub svm_address: Pubkey,
    pub token_name: String,
    pub token_symbol: String,
    pub token_decimals: u8,
    pub token_logo: String,
    pub token_ledger: Principal,
    #[serde(default)]
    pub ledger_verified: bool,
    #[serde(default)]
    pub ledger_minting_account: Option<Account>,

    #[serde(default)]
    pub token_bridge_fee: u128, // with the same decimals as token
    pub min_threshold_to_bridge: u128,
    /// Gas limit of this token's ERC-20 `transfer`, on every EVM chain.
    #[serde(default = "default_erc20_gas_limit")]
    pub erc20_gas_limit: u64,
    // chain_name => (contract_address, decimals, chain_id)
    pub evm_token_contracts: HashMap<String, (Address, u8, u64)>,
    // chain_name => (gas_updated_at, gas_price, max_priority_fee_per_gas)
    pub evm_latest_gas: HashMap<String, (u64, u128, u128)>,
    // chain_name => (max_confirmations, [provider_url])
    pub evm_providers: HashMap<String, (u64, Vec<String>)>,
    // (token_address, decimals, token_program)
    #[serde(default)]
    pub svm_token_address: (Pubkey, u8, Pubkey),
    #[serde(default)]
    pub svm_providers: Vec<String>,
    #[serde(default)]
    pub public_evm_providers: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub public_svm_providers: Option<Vec<String>>,

    #[serde(default)]
    pub svm_mint_verified: bool,
    #[serde(default)]
    pub svm_token_account_size: u64,
    pub ecdsa_public_key: PublicKeyOutput,
    #[serde(default)]
    pub ed25519_public_key: PublicKeyOutput,
    pub governance_canister: Option<Principal>,
    #[serde(default, rename = "pending")]
    pub legacy_pending: VecDeque<BridgeLog>,
    // (round, running)
    pub finalize_bridging_round: (u64, bool),
    // when the running round took the lock, in ms; 0 when no round is running
    #[serde(default)]
    pub finalize_bridging_started_at: u64,
    // consecutive finalization rounds in which no pending task advanced
    #[serde(default)]
    pub idle_rounds: u64,
    #[serde(default)]
    pub total_bridged_tokens: u128,
    #[serde(default)]
    pub total_collected_fees: u128,
    /// Fees from ICP-origin deposits, retained as the existing conservative
    /// governance withdrawal ceiling. This statistic does not gate payouts
    /// or represent a separate balance for paying ledger transfer fees.
    #[serde(default)]
    pub icp_collected_fees: u128,
    /// Whether `icp_collected_fees` was recovered from the archive already.
    /// Kept apart from the counter because a share of zero is a valid result.
    #[serde(default)]
    pub icp_collected_fees_migrated: bool,
    #[serde(default)]
    pub total_withdrawn_fees: u128,
    #[serde(default)]
    pub sub_bridges: BTreeSet<Principal>,
    #[serde(default)]
    pub error_rounds: u64,
    #[serde(default)]
    pub resource_limits: ResourceLimits,
    #[serde(default)]
    pub evm_fee_limits: HashMap<String, EvmFeeLimits>,
}

#[derive(CandidType, Serialize, Deserialize)]
pub struct StateInfo {
    pub runtime: Option<StateRuntimeInfo>,

    pub key_name: String,
    pub icp_address: Principal,
    pub evm_address: String,
    pub svm_address: String,
    pub token_name: String,
    pub token_symbol: String,
    pub token_decimals: u8,
    pub token_logo: String,
    pub token_ledger: Principal,
    pub token_bridge_fee: u128,
    pub min_threshold_to_bridge: u128,
    pub erc20_gas_limit: u64,
    pub evm_token_contracts: HashMap<String, (String, u8, u64)>,
    pub evm_latest_gas: HashMap<String, (u64, u128, u128)>,
    pub evm_providers: HashMap<String, (u64, Vec<String>)>,
    pub svm_token_address: (String, u8, String),
    pub svm_providers: Vec<String>,
    pub finalize_bridging_round: (u64, bool),
    pub total_bridged_tokens: u128,
    pub total_collected_fees: u128,
    pub icp_collected_fees: u128,
    pub total_withdrawn_fees: u128,
    pub total_bridge_count: u64,
    pub sub_bridges: BTreeSet<Principal>,
    pub error_rounds: u64,
    pub governance_canister: Option<Principal>,
}

#[derive(Clone, CandidType, Serialize, Deserialize)]
pub struct StateRuntimeInfo {
    pub evm_provider_hosts: HashMap<String, Vec<String>>,
    pub svm_provider_hosts: Vec<String>,
    pub ledger_verified: bool,
    pub resource_limits: ResourceLimits,
    pub evm_fee_limits: HashMap<String, EvmFeeLimits>,
    pub available_icp_fees: u128,
    pub pending_count: u64,
    pub unresolved_operations: u64,
    pub migration_remaining: u64,
    pub keys_ready: (bool, bool),
    pub svm_mint_verified: bool,
}

impl StateInfo {
    pub(super) fn new(s: &State, total_bridge_count: u64) -> Self {
        Self {
            runtime: Some(StateRuntimeInfo {
                evm_provider_hosts: s
                    .evm_providers
                    .iter()
                    .map(|(chain, (_, providers))| {
                        (
                            chain.clone(),
                            providers
                                .iter()
                                .map(|p| crate::outcall::provider_host(p))
                                .collect(),
                        )
                    })
                    .collect(),
                svm_provider_hosts: s
                    .svm_providers
                    .iter()
                    .map(|p| crate::outcall::provider_host(p))
                    .collect(),
                ledger_verified: s.ledger_verified,
                resource_limits: s.resource_limits.clone(),
                evm_fee_limits: s
                    .evm_token_contracts
                    .keys()
                    .map(|chain| {
                        (
                            chain.clone(),
                            s.evm_fee_limits.get(chain).cloned().unwrap_or_default(),
                        )
                    })
                    .collect(),
                available_icp_fees: journal::available_withdrawal(s),
                pending_count: pending::len(),
                unresolved_operations: journal::open_count(),
                migration_remaining: migration::remaining(),
                keys_ready: (
                    !s.ecdsa_public_key.public_key.is_empty(),
                    !s.ed25519_public_key.public_key.is_empty(),
                ),
                svm_mint_verified: s.svm_mint_verified,
            }),
            key_name: s.key_name.clone(),
            icp_address: s.icp_address,
            evm_address: s.evm_address.to_string(),
            svm_address: s.svm_address.to_string(),
            token_name: s.token_name.clone(),
            token_symbol: s.token_symbol.clone(),
            token_decimals: s.token_decimals,
            token_logo: s.token_logo.clone(),
            token_ledger: s.token_ledger,
            token_bridge_fee: s.token_bridge_fee,
            min_threshold_to_bridge: s.min_threshold_to_bridge,
            erc20_gas_limit: s.erc20_gas_limit,
            evm_token_contracts: s
                .evm_token_contracts
                .iter()
                .map(|(k, v)| (k.clone(), (v.0.to_string(), v.1, v.2)))
                .collect(),
            evm_latest_gas: s
                .evm_latest_gas
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            evm_providers: s
                .evm_providers
                .iter()
                .map(|(chain, (confirmations, providers))| {
                    (
                        chain.clone(),
                        (
                            *confirmations,
                            crate::outcall::browser_providers(
                                providers,
                                s.public_evm_providers.get(chain),
                            ),
                        ),
                    )
                })
                .collect(),
            svm_token_address: (
                s.svm_token_address.0.to_string(),
                s.svm_token_address.1,
                s.svm_token_address.2.to_string(),
            ),
            svm_providers: crate::outcall::browser_providers(
                &s.svm_providers,
                s.public_svm_providers.as_ref(),
            ),
            finalize_bridging_round: s.finalize_bridging_round,
            total_bridged_tokens: s.total_bridged_tokens,
            total_collected_fees: s.total_collected_fees,
            icp_collected_fees: s.icp_collected_fees,
            total_withdrawn_fees: s.total_withdrawn_fees,
            total_bridge_count,
            sub_bridges: s.sub_bridges.clone(),
            error_rounds: s.error_rounds,
            governance_canister: s.governance_canister,
        }
    }
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            key_name: "dfx_test_key".to_string(),
            icp_address: crate::helper::canister_id(),
            evm_address: [0u8; 20].into(),
            svm_address: Pubkey::default(), // 11111111111111111111111111111111
            token_name: "ICPanda".to_string(),
            token_symbol: "PANDA".to_string(),
            token_decimals: 8,
            token_logo: "https://532er-faaaa-aaaaj-qncpa-cai.icp0.io/f/374?inline&filename=1734188626561.webp".to_string(),
            token_ledger: Principal::from_text("druyg-tyaaa-aaaaq-aactq-cai").unwrap(), // mainnet ledger
            ledger_verified: false,
            ledger_minting_account: None,
            token_bridge_fee: 0,
            min_threshold_to_bridge: 100_000_000, // 1 Token (8 decimals)
            erc20_gas_limit: DEFAULT_ERC20_GAS_LIMIT,
            evm_token_contracts: HashMap::new(),
            evm_providers: HashMap::new(),
            evm_latest_gas: HashMap::new(),
            svm_token_address: (Pubkey::default(), 0, Pubkey::default()),
            svm_providers: Vec::new(),
            public_evm_providers: HashMap::new(),
            public_svm_providers: None,
            svm_mint_verified: false,
            svm_token_account_size: 0,
            ecdsa_public_key: PublicKeyOutput::default(),
            ed25519_public_key: PublicKeyOutput::default(),
            governance_canister: None,
            legacy_pending: VecDeque::new(),
            finalize_bridging_round: (0, false),
            finalize_bridging_started_at: 0,
            idle_rounds: 0,
            total_bridged_tokens: 0,
            total_collected_fees: 0,
            icp_collected_fees: 0,
            icp_collected_fees_migrated: false,
            total_withdrawn_fees: 0,
            sub_bridges: BTreeSet::new(),
            error_rounds: 0,
            resource_limits: ResourceLimits::default(),
            evm_fee_limits: HashMap::new(),
        }
    }
}

#[derive(Clone, CandidType, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum BridgeTarget {
    Icp,
    Sol,
    Evm(String), // chain_name
}

#[derive(Clone, CandidType, Debug, Serialize, Deserialize)]
pub enum BridgeTx {
    Icp(bool, u64),           // (finalized, block_height)
    Evm(bool, ByteArray<32>), // (finalized, tx_hash)
    Sol(bool, ByteArray<64>), // (finalized, tx_signature)
}

/// Two records of one transaction are the same transaction whether or not
/// either of them has seen it finalize, so equality ignores the flag.
impl cmp::PartialEq for BridgeTx {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (BridgeTx::Icp(_, tx1), BridgeTx::Icp(_, tx2)) => tx1 == tx2,
            (BridgeTx::Evm(_, tx1), BridgeTx::Evm(_, tx2)) => tx1 == tx2,
            (BridgeTx::Sol(_, tx1), BridgeTx::Sol(_, tx2)) => tx1 == tx2,
            _ => false,
        }
    }
}

impl BridgeTarget {
    /// The name errors raised against this chain are prefixed with.
    pub fn name(&self) -> &str {
        match self {
            BridgeTarget::Icp => "ICP",
            BridgeTarget::Sol => "SOL",
            BridgeTarget::Evm(chain) => chain,
        }
    }
}

impl BridgeTx {
    pub fn is_finalized(&self) -> bool {
        match self {
            BridgeTx::Icp(finalized, _) => *finalized,
            BridgeTx::Evm(finalized, _) => *finalized,
            BridgeTx::Sol(finalized, _) => *finalized,
        }
    }
}

/// The point past which a transaction can never be included any more.
#[derive(Clone, CandidType, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TxDeadline {
    /// EVM: the nonce it spends. Once the sender's nonce has moved past it
    /// without it being mined, another transaction took its place.
    Nonce(u64),
    /// Solana: the last block height its blockhash is valid at.
    BlockHeight(u64),
}

/// What a round needs to know about a transaction besides its hash: how to
/// tell that it is dead, and how to broadcast it again while it is not.
#[derive(Clone, CandidType, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TxMeta {
    pub deadline: TxDeadline,
    /// The signed transaction, kept while it is unconfirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<ByteBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svm_validity: Option<SolValidity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, CandidType, Serialize, Deserialize)]
pub enum PayoutResolution {
    Completed,
    Failed,
    Expired,
    Incomplete,
}

#[derive(Clone, CandidType, Serialize, Deserialize)]
pub struct BridgeLog {
    #[serde(default)]
    pub runtime: Option<LogRuntime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub user: Principal,
    pub from: BridgeTarget,
    pub to: BridgeTarget,
    pub icp_amount: u128,
    #[serde(default)]
    pub fee: u128,
    pub from_tx: BridgeTx,
    pub to_tx: Option<BridgeTx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_addr: Option<String>,
    pub created_at: u64,
    pub finalized_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The error is the task's own and will not clear by itself: an
    /// administrator has to retry or close the task. It blocks nothing else
    /// meanwhile.
    #[serde(default)]
    pub stuck: bool,
    /// When the payout was first attempted, in ms, or 0. The ledger dedup key
    /// of an ICP payout is built from it, so every attempt shares it.
    #[serde(default)]
    pub payout_started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_meta: Option<TxMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_meta: Option<TxMeta>,
}

#[derive(Clone, Default, CandidType, Serialize, Deserialize)]
pub struct LogRuntime {
    pub task_id: u64,
    pub ledger: Option<Principal>,
    pub payout_attempt: Option<u64>,
    pub payout_resolution: Option<PayoutResolution>,
    pub next_poll_at: u64,
    pub poll_attempts: u32,
    pub error_chain: Option<BridgeTarget>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BridgeLogLocal {
    #[serde(default)]
    pub task_id: u64,
    #[serde(default)]
    pub ledger: Option<Principal>,
    #[serde(default)]
    pub payout_attempt: Option<u64>,
    #[serde(default)]
    pub payout_resolution: Option<PayoutResolution>,
    #[serde(default)]
    pub next_poll_at: u64,
    #[serde(default)]
    pub poll_attempts: u32,
    #[serde(default)]
    pub error_chain: Option<BridgeTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(rename = "u", alias = "user")]
    pub user: Principal,
    #[serde(rename = "f", alias = "from")]
    pub from: BridgeTarget,
    #[serde(rename = "t", alias = "to")]
    pub to: BridgeTarget,
    #[serde(rename = "a", alias = "icp_amount")]
    pub icp_amount: u128,
    #[serde(default, rename = "e", alias = "fee")]
    pub fee: u128,
    #[serde(rename = "ft", alias = "from_tx")]
    pub from_tx: BridgeTx,
    #[serde(rename = "tt", alias = "to_tx")]
    pub to_tx: Option<BridgeTx>,
    #[serde(
        rename = "ta",
        alias = "to_addr",
        skip_serializing_if = "Option::is_none"
    )]
    pub to_addr: Option<String>,
    #[serde(rename = "ca", alias = "created_at")]
    pub created_at: u64,
    #[serde(rename = "fa", alias = "finalized_at")]
    pub finalized_at: u64,
    #[serde(
        rename = "er",
        alias = "error",
        skip_serializing_if = "Option::is_none"
    )]
    pub error: Option<String>,
    #[serde(default, rename = "st", alias = "stuck")]
    pub stuck: bool,
    #[serde(default, rename = "ps", alias = "payout_started_at")]
    pub payout_started_at: u64,
    #[serde(
        default,
        rename = "fm",
        alias = "from_meta",
        skip_serializing_if = "Option::is_none"
    )]
    pub from_meta: Option<TxMeta>,
    #[serde(
        default,
        rename = "tm",
        alias = "to_meta",
        skip_serializing_if = "Option::is_none"
    )]
    pub to_meta: Option<TxMeta>,
}

impl From<BridgeLogLocal> for BridgeLog {
    fn from(log: BridgeLogLocal) -> Self {
        Self {
            runtime: Some(LogRuntime {
                task_id: log.task_id,
                ledger: log.ledger,
                payout_attempt: log.payout_attempt,
                payout_resolution: log.payout_resolution,
                next_poll_at: log.next_poll_at,
                poll_attempts: log.poll_attempts,
                error_chain: log.error_chain.clone(),
            }),
            id: log.id,
            user: log.user,
            from: log.from,
            to: log.to,
            icp_amount: log.icp_amount,
            fee: log.fee,
            from_tx: log.from_tx,
            to_tx: log.to_tx,
            to_addr: log.to_addr,
            created_at: log.created_at,
            finalized_at: log.finalized_at,
            error: log.error,
            stuck: log.stuck,
            payout_started_at: log.payout_started_at,
            from_meta: log.from_meta,
            to_meta: log.to_meta,
        }
    }
}

impl From<BridgeLog> for BridgeLogLocal {
    fn from(log: BridgeLog) -> Self {
        Self {
            task_id: log.task_id,
            ledger: log.ledger,
            payout_attempt: log.payout_attempt,
            payout_resolution: log.payout_resolution,
            next_poll_at: log.next_poll_at,
            poll_attempts: log.poll_attempts,
            error_chain: log.error_chain.clone(),
            id: log.id,
            user: log.user,
            from: log.from,
            to: log.to,
            icp_amount: log.icp_amount,
            fee: log.fee,
            from_tx: log.from_tx,
            to_tx: log.to_tx,
            to_addr: log.to_addr,
            created_at: log.created_at,
            finalized_at: log.finalized_at,
            error: log.error,
            stuck: log.stuck,
            payout_started_at: log.payout_started_at,
            from_meta: log.from_meta,
            to_meta: log.to_meta,
        }
    }
}

impl BridgeLog {
    pub fn public_view(mut self) -> Self {
        if let Some(meta) = &mut self.from_meta {
            meta.raw = None;
        }
        if let Some(meta) = &mut self.to_meta {
            meta.raw = None;
        }
        self
    }

    pub fn payout_may_execute(&self) -> bool {
        self.payout_resolution.is_none()
            && !self.to_tx.as_ref().is_some_and(BridgeTx::is_finalized)
            && (self.payout_attempt.is_some()
                || self.payout_started_at > 0
                || self.payout_in_flight())
    }

    pub fn is_finalized(&self) -> bool {
        self.from_tx.is_finalized() && self.to_tx.as_ref().is_some_and(|tx| tx.is_finalized())
    }

    /// Whether the payout has been handed to a chain and not confirmed yet.
    pub fn payout_in_flight(&self) -> bool {
        self.to_tx.as_ref().is_some_and(|tx| !tx.is_finalized())
    }

    /// A transient error: a provider or ledger problem the next round retries.
    /// Only these gate a chain and count towards the circuit breaker.
    pub fn has_transient_error(&self) -> bool {
        self.error.is_some() && !self.stuck
    }
}

impl Storable for BridgeLogLocal {
    const BOUND: Bound = Bound::Unbounded;

    fn into_bytes(self) -> Vec<u8> {
        cbor_into_vec(&self).expect("failed to encode BridgeLogLocal data")
    }

    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(cbor_into_vec(self).expect("failed to encode BridgeLogLocal data"))
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        cbor_from_slice(&bytes).expect("failed to decode BridgeLogLocal data")
    }
}

// Pending records always populate runtime. Older archive/API records have no
// extension; defaults are used until migration assigns the stable task identity.
impl std::ops::Deref for BridgeLog {
    type Target = LogRuntime;
    fn deref(&self) -> &Self::Target {
        static LEGACY: LazyLock<LogRuntime> = LazyLock::new(LogRuntime::default);
        self.runtime.as_ref().unwrap_or(&LEGACY)
    }
}
impl std::ops::DerefMut for BridgeLog {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.runtime.get_or_insert_with(LogRuntime::default)
    }
}
