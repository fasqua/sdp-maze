//! Configuration for SDP Maze

use serde::{Deserialize, Serialize};

/// Fee percentage (0.5%)
pub const FEE_PERCENT: f64 = 0.5;

/// Transaction fee per TX in lamports
pub const TX_FEE_LAMPORTS: u64 = 5_000;

/// Minimum transfer amount in SOL
pub const MIN_AMOUNT_SOL: f64 = 0.01;

/// Request expiry in seconds (30 minutes)
pub const EXPIRY_SECONDS: i64 = 1800;

/// Fee wallet address
pub const FEE_WALLET: &str = "Nd5yLUNpZwqQ9GzMt1TmbwBNfR5EYpjrNWuHbQh9SDP";

/// Database path
pub const DB_PATH: &str = "maze_relay.db";
pub const SHARED_DB_PATH: &str = "shared_relay.db";

/// Autopurge interval (24 hours in seconds)
pub const AUTOPURGE_SECONDS: i64 = 86400;

// ============ MAZE PARAMETERS ============

/// Minimum hops in maze
pub const MIN_HOPS: u8 = 5;

/// Maximum hops in maze
pub const MAX_HOPS: u8 = 10;

/// Default hop count
pub const DEFAULT_HOPS: u8 = 10;

/// Minimum split branches per node
pub const MIN_SPLIT: u8 = 2;

/// Maximum split branches per node
pub const MAX_SPLIT: u8 = 4;

/// Amount noise percentage (for obfuscation)
pub const AMOUNT_NOISE_PERCENT: f64 = 0.5;

// ============ SUBSCRIPTION CONSTANTS ============

pub const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
pub const KAUSA_MINT: &str = "BWXSNRBKMviG68MqavyssnzDq4qSArcN7eNYjqEfpump";
pub const USDC_DECIMALS: u8 = 6;
pub const KAUSA_DECIMALS: u8 = 6;
pub const SUBSCRIPTION_USDC_AMOUNT: u64 = 20_000_000;  // $20 USDC
pub const SUBSCRIPTION_KAUSA_USD: f64 = 15.0;          // $15 worth of KAUSA
pub const PRICE_CACHE_SECONDS: u64 = 300;              // 5 minutes

/// Maze generation parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MazeParameters {
    /// Random seed for deterministic generation (256-bit)
    pub seed: [u8; 32],
    /// Fibonacci offset for path variation (0-100)
    pub fib_offset: u8,
    /// Split ratio based on golden ratio variant (1.1-3.0)
    pub split_ratio: f64,
    /// Total number of hops/nodes in maze
    pub hop_count: u8,
    /// Merge strategy: "early", "late", "middle", "random", "fibonacci"
    pub merge_strategy: MergeStrategy,
    /// Delay pattern between transactions
    pub delay_pattern: DelayPattern,
    /// Amount variation percentage (0.01% - 1%)
    pub amount_noise: f64,
    /// Base delay in milliseconds (0-5000)
    pub delay_ms: u64,
    /// Delay scope: per node or per level
    pub delay_scope: DelayScope,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum MergeStrategy {
    Early,
    Late,
    Middle,
    Random,
    Fibonacci,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum DelayPattern {
    Linear,
    Exponential,
    Random,
    Fibonacci,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub enum DelayScope {
    #[default]
    Node,
    Level,
}

impl Default for MazeParameters {
    fn default() -> Self {
        Self {
            seed: rand::random(),
            fib_offset: rand::random::<u8>() % 100,
            split_ratio: 1.618, // Golden ratio
            hop_count: DEFAULT_HOPS,
            merge_strategy: MergeStrategy::Random,
            delay_pattern: DelayPattern::None, // No delay for speed
            amount_noise: 0.1, // 0.1% noise
            delay_ms: 0, // No delay by default
            delay_scope: DelayScope::Node, // Per node by default
        }
    }
}

impl MazeParameters {
    /// Generate random parameters for a new maze
    pub fn random() -> Self {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        Self {
            seed: rand::random(),
            fib_offset: rng.gen_range(0..100),
            split_ratio: rng.gen_range(1.1..3.0),
            hop_count: rng.gen_range(MIN_HOPS..=MAX_HOPS),
            merge_strategy: match rng.gen_range(0..5) {
                0 => MergeStrategy::Early,
                1 => MergeStrategy::Late,
                2 => MergeStrategy::Middle,
                3 => MergeStrategy::Fibonacci,
                _ => MergeStrategy::Random,
            },
            delay_pattern: DelayPattern::None,
            amount_noise: rng.gen_range(0.01..1.0),
            delay_ms: 0, // No delay for random (speed priority)
            delay_scope: DelayScope::Node,
        }
    }

    /// Serialize parameters for encryption
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).unwrap_or_default()
    }

    /// Deserialize parameters
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bincode::deserialize(bytes).ok()
    }
}

/// Global configuration
#[derive(Debug, Clone)]
pub struct Config {
    pub rpc_url: String,
    pub fee_wallet: String,
    pub fee_percent: f64,
    pub min_amount_sol: f64,
    pub expiry_seconds: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rpc_url: std::env::var("SOLANA_RPC_URL")
                .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string()),
            fee_wallet: FEE_WALLET.to_string(),
            fee_percent: FEE_PERCENT,
            min_amount_sol: MIN_AMOUNT_SOL,
            expiry_seconds: EXPIRY_SECONDS,
        }
    }
}
