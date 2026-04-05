//! SDP Maze Relay Server
//!
//! Main entry point for the maze-based privacy relay

use axum::{
    Router,
    routing::{get, post},
    extract::{State, Json, Path},
    http::{StatusCode, Method},
    response::IntoResponse,
};
use tower_http::cors::{CorsLayer, Any};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use bincode;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    signature::{Keypair, Signer},
    pubkey::Pubkey,
    system_instruction,
    transaction::Transaction,
    commitment_config::CommitmentConfig,
};
use tracing::{info, error};
use std::str::FromStr;
use sha2::{Sha256, Digest};

use sdp_maze::{
    Config,
    MetaAddress, create_stealth_address,
    MazeError,
};
use sdp_maze::config::{
    FEE_PERCENT, TX_FEE_LAMPORTS, MIN_AMOUNT_SOL, EXPIRY_SECONDS, FEE_WALLET,
    MazeParameters, MIN_HOPS, MAX_HOPS,
    MergeStrategy, DelayPattern, DelayScope,
};
use sdp_maze::relay::{
    RelayDatabase, MazeRequest, RequestStatus,
    MazeGraph, MazeGenerator, MazePreferencesRow,
};
use sdp_maze::core::utils::{lamports_to_sol, sol_to_lamports};

// ============ APP STATE ============

struct AppState {
    db: RelayDatabase,
    rpc: RpcClient,
    config: Config,
    api_key: Option<String>,
}

type SharedState = Arc<AppState>;

// ============ ERROR WRAPPER ============

struct AppError(MazeError);

impl From<MazeError> for AppError {
    fn from(e: MazeError) -> Self {
        AppError(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, code) = match &self.0 {
            MazeError::InvalidMetaAddress(_) => (StatusCode::BAD_REQUEST, "INVALID_ADDRESS"),
            MazeError::InvalidParameters(_) => (StatusCode::BAD_REQUEST, "INVALID_PARAMS"),
            MazeError::InsufficientFunds { .. } => (StatusCode::BAD_REQUEST, "INSUFFICIENT_FUNDS"),
            MazeError::RequestNotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            MazeError::RequestExpired => (StatusCode::GONE, "EXPIRED"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
        };

        let body = Json(ErrorResponse {
            error: self.0.to_string(),
            code: code.to_string(),
        });

        (status, body).into_response()
    }
}

// ============ API TYPES ============

// Custom maze configuration (for KAUSA holders)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CustomMazeConfig {
    #[serde(default)]
    hop_count: Option<u8>,
    #[serde(default)]
    split_ratio: Option<f64>,
    #[serde(default)]
    merge_strategy: Option<String>,
    #[serde(default)]
    delay_pattern: Option<String>,
    #[serde(default)]
    delay_ms: Option<u64>,
    #[serde(default)]
    delay_scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateTransferRequest {
    sender_meta_hash: String,
    receiver_meta: String,
    amount_sol: f64,
    #[serde(default)]
    hop_count: Option<u8>,
    #[serde(default)]
    maze_config: Option<CustomMazeConfig>,
}

#[derive(Debug, Serialize)]
struct CreateTransferResponse {
    request_id: String,
    deposit_address: String,
    amount_lamports: u64,
    fee_lamports: u64,
    total_lamports: u64,
    expires_at: i64,
    maze_preview: MazePreview,
}

#[derive(Debug, Serialize)]
struct MazePreview {
    total_nodes: usize,
    total_levels: u8,
    total_transactions: u16,
    estimated_time_seconds: u16,
}

#[derive(Debug, Serialize)]
struct TransferStatusResponse {
    request_id: String,
    status: String,
    deposit_address: String,
    amount_lamports: u64,
    progress: ProgressInfo,
    final_tx_signature: Option<String>,
    error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route_signatures: Option<Vec<RouteSignature>>,
}

#[derive(Debug, Serialize)]
struct RouteSignature {
    route_index: usize,
    destination: String,
    tx_signature: Option<String>,
    status: String,
}

#[derive(Debug, Serialize)]
struct ProgressInfo {
    completed_nodes: usize,
    total_nodes: usize,
    current_level: u8,
    total_levels: u8,
    percentage: f64,
}

#[derive(Debug, Deserialize)]
struct ScanRequest {
    receiver_meta: String,
}

#[derive(Debug, Serialize)]
struct ScanResponse {
    transfers: Vec<TransferInfo>,
}

#[derive(Debug, Serialize)]
struct TransferInfo {
    stealth_pubkey: String,
    ephemeral_pubkey: String,
    amount_sol: f64,
    tx_signature: String,
    completed_at: i64,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: String,
    version: String,
    protocol: String,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    code: String,
}
// ============ ALIAS & WALLET TYPES ============

#[derive(Debug, Deserialize)]
struct ResolveAliasQuery {
    alias: String,
}

#[derive(Debug, Serialize)]
struct ResolveAliasResponse {
    alias: String,
    meta_address: Option<String>,
    found: bool,
}

#[derive(Debug, Deserialize)]
struct CheckAliasQuery {
    alias: String,
}

#[derive(Debug, Serialize)]
struct CheckAliasResponse {
    alias: String,
    available: bool,
}

#[derive(Debug, Deserialize)]
struct RegisterAliasRequest {
    alias: String,
    meta_address: String,
    owner_meta_hash: String,
}

#[derive(Debug, Deserialize)]
struct ListAliasesRequest {
    owner_meta_hash: String,
}

#[derive(Debug, Serialize)]
struct ListAliasesResponse {
    aliases: Vec<AliasInfo>,
}

#[derive(Debug, Serialize)]
struct AliasInfo {
    alias: String,
    meta_address: String,
}

#[derive(Debug, Deserialize)]
struct AddWalletRequest {
    owner_meta_hash: String,
    slot: i32,
    wallet_address: String,
}

#[derive(Debug, Deserialize)]
struct DeleteWalletRequest {
    owner_meta_hash: String,
    slot: i32,
}

#[derive(Debug, Deserialize)]
struct ListWalletsRequest {
    owner_meta_hash: String,
}

#[derive(Debug, Serialize)]
struct ListWalletsResponse {
    wallets: Vec<WalletInfo>,
}

#[derive(Debug, Serialize)]
struct WalletInfo {
    slot: i32,
    address: String,
}

#[derive(Debug, Deserialize)]
struct ClaimRequest {
    stealth_pubkey: String,
    destination: String,
    signed_tx: String,
}

#[derive(Debug, Serialize)]
struct ClaimResponse {
    success: bool,
    tx_signature: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RecoverRequest {
    request_id: String,
    destination: String,
}

#[derive(Debug, Serialize)]
struct RecoverResponse {
    success: bool,
    recovered_amount: Option<u64>,
    tx_signatures: Vec<String>,
    error: Option<String>,
}
// ============ SWAP TYPES ============

#[derive(Debug, Deserialize)]
struct SwapRequest {
    sender_meta_hash: String,
    amount_sol: f64,
    token_mint: String,
    destination: String,
    #[serde(default)]
    maze_config: Option<CustomMazeConfig>,
}

#[derive(Debug, Serialize)]
struct SwapResponse {
    success: bool,
    request_id: Option<String>,
    deposit_address: Option<String>,
    deposit_amount: Option<f64>,
    estimated_output: Option<String>,
    fee: Option<f64>,
    expires_in: Option<i64>,
    maze_preview: Option<MazePreview>,
    error: Option<String>,
}

// ============ DIVERSIFY TYPES ============

#[derive(Debug, Deserialize)]
struct DiversifyRouteInput {
    slot: i32,
    value: f64,
}

#[derive(Debug, Deserialize)]
struct DiversifyRequest {
    meta_address: String,
    total_amount: f64,
    distribution_mode: String,
    routes: Vec<DiversifyRouteInput>,
    #[serde(default)]
    maze_config: Option<CustomMazeConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiversifyRouteOutput {
    slot: i32,
    wallet: String,
    amount: f64,
    percentage: Option<f64>,
}

#[derive(Debug, Serialize)]
struct DiversifyResponse {
    success: bool,
    request_id: Option<String>,
    deposit_address: Option<String>,
    deposit_amount: Option<f64>,
    total_amount: Option<f64>,
    fee: Option<f64>,
    routes: Option<Vec<DiversifyRouteOutput>>,
    expires_in: Option<i64>,
    maze_preview: Option<MazePreview>,
    error: Option<String>,
}

// ============ MAZE PREFERENCES TYPES ============

#[derive(Debug, Deserialize)]
struct GetPreferencesRequest {
    meta_address: String,
}

#[derive(Debug, Serialize)]
struct GetPreferencesResponse {
    success: bool,
    preferences: Option<MazePreferencesData>,
    error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MazePreferencesData {
    hop_count: u8,
    split_ratio: f64,
    merge_strategy: String,
    delay_pattern: String,
    delay_ms: u64,
    delay_scope: String,
    updated_at: i64,
}

#[derive(Debug, Deserialize)]
struct SavePreferencesRequest {
    meta_address: String,
    hop_count: Option<u8>,
    split_ratio: Option<f64>,
    merge_strategy: Option<String>,
    delay_pattern: Option<String>,
    delay_ms: Option<u64>,
    delay_scope: Option<String>,
}

#[derive(Debug, Serialize)]
struct SavePreferencesResponse {
    success: bool,
    error: Option<String>,
}


#[derive(Debug, Deserialize)]
struct DiversifyStatusQuery {
    request_id: String,
}





#[derive(Debug, Serialize)]
struct MazeGraphResponse {
    request_id: String,
    total_levels: u8,
    total_nodes: usize,
    total_transactions: u16,
    nodes: Vec<NodeInfo>,
}

#[derive(Debug, Serialize)]
struct NodeInfo {
    index: u16,
    level: u8,
    address: String,
    status: String,
    amount_in: u64,
    amount_out: u64,
    tx_signature: Option<String>,
}

// ============ HANDLERS ============

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol: "SDP Maze v1".to_string(),
    })
}

#[derive(Debug, Serialize)]
struct BlockhashResponse {
    blockhash: String,
}

async fn blockhash_handler(
    State(state): State<SharedState>,
) -> Result<Json<BlockhashResponse>, AppError> {
    let blockhash = state.rpc.get_latest_blockhash()
        .map_err(|e| MazeError::RpcError(e.to_string()))?;
    Ok(Json(BlockhashResponse {
        blockhash: blockhash.to_string(),
    }))
}

#[derive(Debug, Deserialize)]
struct SubmitRequest {
    signed_tx: String,
}

#[derive(Debug, Serialize)]
struct SubmitResponse {
    status: String,
    signature: Option<String>,
    message: Option<String>,
}

async fn submit_handler(
    State(state): State<SharedState>,
    Json(req): Json<SubmitRequest>,
) -> Result<Json<SubmitResponse>, AppError> {
    use solana_sdk::transaction::Transaction;
    use solana_transaction_status::UiTransactionEncoding;
    
    // Decode base64 transaction
    let tx_bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &req.signed_tx)
        .map_err(|e| MazeError::CryptoError(format!("Invalid base64: {}", e)))?;
    
    let tx: Transaction = bincode::deserialize(&tx_bytes)
        .map_err(|e| MazeError::CryptoError(format!("Invalid transaction: {}", e)))?;
    
    // Send transaction
    match state.rpc.send_and_confirm_transaction(&tx) {
        Ok(sig) => {
            info!("Transaction submitted: {}", sig);
            Ok(Json(SubmitResponse {
                status: "success".to_string(),
                signature: Some(sig.to_string()),
                message: None,
            }))
        }
        Err(e) => {
            error!("Transaction failed: {}", e);
            Ok(Json(SubmitResponse {
                status: "error".to_string(),
                signature: None,
                message: Some(e.to_string()),
            }))
        }
    }
}

async fn create_transfer_handler(
    State(state): State<SharedState>,
    Json(req): Json<CreateTransferRequest>,
) -> Result<Json<CreateTransferResponse>, AppError> {
    info!("Creating maze transfer to: {}", req.receiver_meta);

    // Validate meta address
    let meta = MetaAddress::decode(&req.receiver_meta)?;

    // Validate amount
    if req.amount_sol < MIN_AMOUNT_SOL {
        return Err(MazeError::InvalidParameters(
            format!("Minimum amount is {} SOL", MIN_AMOUNT_SOL)
        ).into());
    }

    let amount_lamports = sol_to_lamports(req.amount_sol);
    
    // Hash sender meta address for subscription check
    let mut hasher = Sha256::new();
    hasher.update(req.sender_meta_hash.as_bytes());
    let sender_hash = format!("{:x}", hasher.finalize());

    // Check if sender is Pro subscriber (fee = 0)
    let fee_lamports = if state.db.is_pro_subscriber(&sender_hash) {
        info!("Pro subscriber detected, fee = 0");
        0
    } else {
        (amount_lamports as f64 * FEE_PERCENT / 100.0) as u64
    };

    // Generate maze parameters (custom config or random)
    let params = if let Some(config) = req.maze_config {
        let mut p = MazeParameters::default();
        if let Some(hops) = config.hop_count {
            p.hop_count = hops.max(MIN_HOPS).min(MAX_HOPS);
        } else if let Some(hops) = req.hop_count {
            p.hop_count = hops.max(MIN_HOPS).min(MAX_HOPS);
        }
        if let Some(ratio) = config.split_ratio {
            p.split_ratio = ratio.max(1.1).min(3.0);
        }
        if let Some(ref strategy) = config.merge_strategy {
            p.merge_strategy = match strategy.as_str() {
                "early" => MergeStrategy::Early,
                "late" => MergeStrategy::Late,
                "middle" => MergeStrategy::Middle,
                "fibonacci" => MergeStrategy::Fibonacci,
                _ => MergeStrategy::Random,
            };
        }
        if let Some(ref pattern) = config.delay_pattern {
            p.delay_pattern = match pattern.as_str() {
                "linear" => DelayPattern::Linear,
                "exponential" => DelayPattern::Exponential,
                "random" => DelayPattern::Random,
                "fibonacci" => DelayPattern::Fibonacci,
                _ => DelayPattern::None,
            };
        }
        if let Some(ms) = config.delay_ms {
            p.delay_ms = ms.min(5000);
        }
        if let Some(ref scope) = config.delay_scope {
            p.delay_scope = match scope.as_str() {
                "level" => DelayScope::Level,
                _ => DelayScope::Node,
            };
        }
        info!("Using CUSTOM maze config: hop_count={}, split_ratio={:.2}, merge={:?}, delay={:?} {}ms scope={:?}", p.hop_count, p.split_ratio, p.merge_strategy, p.delay_pattern, p.delay_ms, p.delay_scope);
        p
    } else {
        let mut p = MazeParameters::random();
        if let Some(hops) = req.hop_count {
            p.hop_count = hops.max(MIN_HOPS).min(MAX_HOPS);
        }
        info!("Using RANDOM maze config: hop_count={}, split_ratio={:.2}, merge={:?}, delay={:?} {}ms", p.hop_count, p.split_ratio, p.merge_strategy, p.delay_pattern, p.delay_ms);
        p
    };


    // Create stealth address for receiver
    let stealth = create_stealth_address(&meta)?;

    // Generate maze graph
    let generator = MazeGenerator::new(params.clone());
    let encrypt_fn = |data: &[u8]| state.db.encrypt(data);
    
    let total_with_fees = amount_lamports + fee_lamports + (TX_FEE_LAMPORTS * 50);
    let maze = generator.generate(total_with_fees, encrypt_fn)?;

    // Get deposit address from maze
    let deposit_node = maze.get_deposit_node()
        .ok_or_else(|| MazeError::MazeGenerationError("No deposit node".into()))?;

    // Create request ID
    let request_id = format!("maze_{}", hex::encode(&rand::random::<[u8; 8]>()));
    
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + EXPIRY_SECONDS;

    // Serialize maze for storage
    let maze_json = serde_json::to_string(&maze)
        .map_err(|e| MazeError::DatabaseError(e.to_string()))?;

    // Create request record
    let request = MazeRequest {
        id: request_id.clone(),
        receiver_meta: req.receiver_meta.clone(),
        stealth_pubkey: bs58::encode(&stealth.pubkey).into_string(),
        ephemeral_pubkey: bs58::encode(&stealth.ephemeral_pubkey).into_string(),
        deposit_address: deposit_node.address.clone(),
        amount_lamports,
        fee_lamports,
        status: RequestStatus::Pending,
        maze_graph_json: maze_json,
        created_at: now,
        expires_at,
        completed_at: None,
        final_tx_signature: None,
        error_message: None,
        sender_meta_hash: Some(sender_hash.clone()),
    };

    // Store in database
    state.db.create_maze_request(&request, &maze)?;

    info!("Created maze request {} with {} nodes", request_id, maze.nodes.len());

    // Calculate estimated time
    let estimated_time = (maze.total_levels as u16) * 4;

    Ok(Json(CreateTransferResponse {
        request_id,
        deposit_address: deposit_node.address.clone(),
        amount_lamports,
        fee_lamports,
        total_lamports: amount_lamports + fee_lamports,
        expires_at,
        maze_preview: MazePreview {
            total_nodes: maze.nodes.len(),
            total_levels: maze.total_levels,
            total_transactions: maze.total_transactions,
            estimated_time_seconds: estimated_time,
        },
    }))
}

async fn get_status_handler(
    State(state): State<SharedState>,
    Path(request_id): Path<String>,
) -> Result<Json<TransferStatusResponse>, AppError> {
    // Check if this is a diversify request
    if request_id.starts_with("div_") && !request_id.contains("_route_") {
        // Get diversify request status
        if let Ok(Some((deposit_address, status, _, total_amount, _, route_count, _, _, _))) = 
            state.db.get_diversify_request(&request_id) 
        {
            // Get routes for progress
            let routes = state.db.get_diversify_routes(&request_id).unwrap_or_default();
            let completed_routes = routes.iter().filter(|(_, _, _, _, _, _, _, status)| status == "completed").count();
            let total_routes = routes.len();
            let percentage = if total_routes > 0 {
                (completed_routes as f64 / total_routes as f64) * 100.0
            } else {
                0.0
            };
            
            // Map diversify status to transfer status
            let mapped_status = match status.as_str() {
                "pending" => "pending",
                "funded" => "deposit_received",
                "processing" => "processing",
                "completed" => "completed",
                "partial" => "partial",
                "failed" => "failed",
                "recovered" => "recovered",
                _ => &status,
            };
            
            // Get tx signatures from child maze requests
            let mut route_sigs: Vec<RouteSignature> = Vec::new();
            for (_, route_idx, _, dest_wallet, _, _, child_id, route_status) in &routes {
                let tx_sig = if let Some(child_request_id) = child_id {
                    state.db.get_maze_request(child_request_id)
                        .ok()
                        .flatten()
                        .and_then(|r| r.final_tx_signature)
                } else {
                    None
                };
                route_sigs.push(RouteSignature {
                    route_index: *route_idx,
                    destination: dest_wallet.clone(),
                    tx_signature: tx_sig,
                    status: route_status.clone(),
                });
            }

            return Ok(Json(TransferStatusResponse {
                request_id: request_id.clone(),
                status: mapped_status.to_string(),
                deposit_address,
                amount_lamports: total_amount as u64,
                progress: ProgressInfo {
                    completed_nodes: completed_routes,
                    total_nodes: total_routes,
                    current_level: completed_routes as u8,
                    total_levels: total_routes as u8,
                    percentage,
                },
                final_tx_signature: None,
                error_message: None,
                route_signatures: Some(route_sigs),
            }));
        }
        return Err(MazeError::RequestNotFound(request_id.clone()).into());
    }
    
    // Standard maze request
    let request = state.db.get_maze_request(&request_id)?
        .ok_or_else(|| MazeError::RequestNotFound(request_id.clone()))?;

    // Get nodes for progress
    let nodes = state.db.get_request_nodes(&request_id)?;
    let completed_nodes = nodes.iter().filter(|n| n.status == "completed").count();
    let current_level = nodes.iter()
        .filter(|n| n.status == "completed")
        .map(|n| n.level)
        .max()
        .unwrap_or(0);

    // Parse maze for total levels
    let maze: MazeGraph = serde_json::from_str(&request.maze_graph_json)
        .map_err(|e| MazeError::DatabaseError(e.to_string()))?;

    let total_nodes = nodes.len();
    let percentage = if total_nodes > 0 {
        (completed_nodes as f64 / total_nodes as f64) * 100.0
    } else {
        0.0
    };

    Ok(Json(TransferStatusResponse {
        request_id: request.id,
        status: request.status.as_str().to_string(),
        deposit_address: request.deposit_address,
        amount_lamports: request.amount_lamports,
        progress: ProgressInfo {
            completed_nodes,
            total_nodes,
            current_level,
            total_levels: maze.total_levels,
            percentage,
        },
        final_tx_signature: request.final_tx_signature,
        error_message: request.error_message,
        route_signatures: None,
    }))
}

async fn scan_handler(
    State(state): State<SharedState>,
    Json(req): Json<ScanRequest>,
) -> Result<Json<ScanResponse>, AppError> {
    // Validate meta address
    let _meta = MetaAddress::decode(&req.receiver_meta)?;

    let transfers = state.db.scan_transfers(&req.receiver_meta)?;
    let mut transfer_infos: Vec<TransferInfo> = Vec::new();
    
    for (stealth, ephemeral, _amount, tx_sig, completed_at) in transfers {
        // Check on-chain balance - only include if still has funds
        if let Ok(stealth_pubkey) = stealth.parse::<Pubkey>() {
            let balance = state.rpc.get_balance(&stealth_pubkey).unwrap_or(0);
            if balance > 5000 {  // More than dust (network fee)
                transfer_infos.push(TransferInfo {
                    stealth_pubkey: stealth,
                    ephemeral_pubkey: ephemeral,
                    amount_sol: lamports_to_sol(balance),
                    tx_signature: tx_sig,
                    completed_at,
                });
            }
        }
    }

    Ok(Json(ScanResponse {
        transfers: transfer_infos,
    }))
}

async fn get_maze_graph_handler(
    State(state): State<SharedState>,
    Path(request_id): Path<String>,
) -> Result<Json<MazeGraphResponse>, AppError> {
    let request = state.db.get_maze_request(&request_id)?
        .ok_or_else(|| MazeError::RequestNotFound(request_id.clone()))?;

    let maze: MazeGraph = serde_json::from_str(&request.maze_graph_json)
        .map_err(|e| MazeError::DatabaseError(e.to_string()))?;

    // Get current node statuses from DB
    let nodes = state.db.get_request_nodes(&request_id)?;

    // Build node info (without encrypted keypairs)
    let node_infos: Vec<NodeInfo> = nodes.iter().map(|n| {
        NodeInfo {
            index: n.index,
            level: n.level,
            address: n.address.clone(),
            status: n.status.clone(),
            amount_in: n.amount_in,
            amount_out: n.amount_out,
            tx_signature: n.tx_in_signature.clone(),
        }
    }).collect();

    Ok(Json(MazeGraphResponse {
        request_id: request.id,
        total_levels: maze.total_levels,
        total_nodes: maze.nodes.len(),
        total_transactions: maze.total_transactions,
        nodes: node_infos,
    }))
}

// ============ BACKGROUND TASKS ============

async fn deposit_monitor_task(state: SharedState) {
    info!("Starting deposit monitor task");
    
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        if let Ok(requests) = state.db.get_pending_requests() {
            for request in requests {
                // Skip child routes - they are executed by execute_diversify directly
                if request.id.contains("_route_") {
                    continue;
                }
                
                if request.status == RequestStatus::Pending {
                    if let Ok(pubkey) = Pubkey::from_str(&request.deposit_address) {
                        if let Ok(balance) = state.rpc.get_balance(&pubkey) {
                            if balance >= request.amount_lamports + request.fee_lamports {
                                info!("Deposit received for {}: {} lamports", 
                                    request.id, balance);
                                
                                let _ = state.db.update_request_status(
                                    &request.id, 
                                    RequestStatus::DepositReceived
                                );

                                let state_clone = state.clone();
                                let request_id = request.id.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = execute_maze(state_clone, &request_id).await {
                                        error!("Maze execution failed for {}: {}", request_id, e);
                                    }
                                });
                            }
                        }
                    }
                }
            }
        }

        // Monitor diversify requests
        if let Ok(div_requests) = state.db.get_pending_diversify_requests() {
            for div_id in div_requests {
                if let Ok(Some((deposit_address, status, keypair_encrypted, total_amount, fee_amount, _mode, _expires, _meta, _))) = state.db.get_diversify_request(&div_id) {
                    if status == "pending" {
                        if let Ok(pubkey) = Pubkey::from_str(&deposit_address) {
                            if let Ok(balance) = state.rpc.get_balance(&pubkey) {
                                let required = total_amount + fee_amount + 5_000_000; // buffer
                                if balance >= required {
                                    info!("Diversify deposit received for {}: {} lamports", div_id, balance);
                                    
                                    let _ = state.db.update_diversify_status(&div_id, "funded");

                                    let state_clone = state.clone();
                                    let div_id_clone = div_id.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = execute_diversify(state_clone, &div_id_clone).await {
                                            error!("Diversify execution failed for {}: {}", div_id_clone, e);
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}


/// Calculate delay based on pattern and level/node
fn calculate_delay(pattern: &DelayPattern, base_ms: u64, index: u8) -> u64 {
    match pattern {
        DelayPattern::None => 0,
        DelayPattern::Linear => base_ms * (index as u64 + 1),
        DelayPattern::Exponential => base_ms * 2u64.pow(index as u32),
        DelayPattern::Random => {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            rng.gen_range(0..=base_ms * 2)
        }
        DelayPattern::Fibonacci => {
            let fib = |n: u8| -> u64 {
                let mut a = 0u64;
                let mut b = 1u64;
                for _ in 0..n {
                    let tmp = a;
                    a = b;
                    b = tmp.saturating_add(b);
                }
                a
            };
            base_ms * fib(index + 1)
        }
    }
}

async fn execute_maze(state: SharedState, request_id: &str) -> Result<(), MazeError> {
    info!("Executing maze for request {}", request_id);

    state.db.update_request_status(request_id, RequestStatus::Processing)?;

    let request = state.db.get_maze_request(request_id)?
        .ok_or_else(|| MazeError::RequestNotFound(request_id.into()))?;

    let maze: MazeGraph = serde_json::from_str(&request.maze_graph_json)
        .map_err(|e| MazeError::DatabaseError(e.to_string()))?;

    let nodes = state.db.get_request_nodes(request_id)?;

    // Process level by level - SEQUENTIAL within level for reliability
    for level in 0..maze.total_levels {
        let level_nodes: Vec<_> = nodes.iter()
            .filter(|n| n.level == level)
            .collect();

        info!("Processing level {} with {} nodes", level, level_nodes.len());

        // Execute nodes in this level sequentially
        for node in level_nodes {
            match execute_node(&state, request_id, node).await {
                Ok(_) => {
                    info!("Node {} level {} completed", node.index, level);
                    // Apply delay if configured (Node scope)
                    if maze.parameters.delay_scope == DelayScope::Node && maze.parameters.delay_ms > 0 {
                        let delay = calculate_delay(&maze.parameters.delay_pattern, maze.parameters.delay_ms, node.index as u8);
                        if delay > 0 {
                            info!("Delay {}ms after node {}", delay, node.index);
                            tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                        }
                    }
                }
                Err(e) => {
                    error!("Node {} failed: {}", node.index, e);
                    state.db.update_request_status(request_id, RequestStatus::SwapFailed)?;
                    return Err(e);
                }
            }
        }
        // Apply delay if configured (Level scope)
        if maze.parameters.delay_scope == DelayScope::Level && maze.parameters.delay_ms > 0 {
            let delay = calculate_delay(&maze.parameters.delay_pattern, maze.parameters.delay_ms, level);
            if delay > 0 {
                info!("Delay {}ms after level {}", delay, level);
                tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
            }
        }
    }

    // Final transfer - check if swap, diversify, or normal
    let db_final = nodes.iter()
        .find(|n| n.index == maze.final_index)
        .ok_or_else(|| MazeError::MazeGenerationError("Final node not found".into()))?;

    let final_keypair_bytes = state.db.decrypt(&db_final.keypair_encrypted)?;
    let final_keypair = Keypair::from_bytes(&final_keypair_bytes)
        .map_err(|e| MazeError::CryptoError(e.to_string()))?;

    // Wait for funds to arrive at final node
    let mut attempts = 0;
    loop {
        let balance = state.rpc.get_balance(&final_keypair.pubkey())?;
        if balance > TX_FEE_LAMPORTS {
            break;
        }
        attempts += 1;
        if attempts > 30 {
            error!("Timeout waiting for final node funds");
            return Err(MazeError::TransactionError("Timeout waiting for final node funds".into()));
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    let balance = state.rpc.get_balance(&final_keypair.pubkey())?;
    let transfer_amount = balance.saturating_sub(TX_FEE_LAMPORTS);

    if transfer_amount == 0 {
        return Ok(());
    }

    // Check if this is a swap request
    if request.receiver_meta.starts_with("swap:") {
        let parts: Vec<&str> = request.receiver_meta.split(':').collect();
        if parts.len() >= 3 {
            let token_mint = parts[1];
            let destination = parts[2];
            // Reserve fee + gas for post-swap transfers (like KausaLayer)
            let swap_amount = balance.saturating_sub(request.fee_lamports + 10_000_000);
            
            info!("Executing Jupiter swap: {} lamports -> {} to {}", swap_amount, token_mint, destination);
            
            match execute_jupiter_swap(&final_keypair, swap_amount, token_mint, destination).await {
                Ok(sig) => {
                    info!("Swap complete: {}", sig);
                    
                    // Transfer protocol fee to fee wallet
                    let fee_wallet = Pubkey::from_str(FEE_WALLET).unwrap();
                    let blockhash = state.rpc.get_latest_blockhash()?;
                    let fee_ix = system_instruction::transfer(&final_keypair.pubkey(), &fee_wallet, request.fee_lamports);
                    let fee_tx = Transaction::new_signed_with_payer(
                        &[fee_ix],
                        Some(&final_keypair.pubkey()),
                        &[&final_keypair],
                        blockhash,
                    );
                    if let Err(e) = state.rpc.send_and_confirm_transaction(&fee_tx) {
                        error!("Fee transfer failed: {}", e);
                    } else {
                        info!("Fee transferred to fee wallet: {} lamports", request.fee_lamports);
                    }
                    
                    // Transfer remaining SOL to destination
                    let dest_pubkey = Pubkey::from_str(destination).unwrap();
                    let remaining = state.rpc.get_balance(&final_keypair.pubkey()).unwrap_or(0);
                    if remaining > 900_000 {
                        let blockhash = state.rpc.get_latest_blockhash()?;
                        let sol_ix = system_instruction::transfer(&final_keypair.pubkey(), &dest_pubkey, remaining - 900_000);
                        let sol_tx = Transaction::new_signed_with_payer(
                            &[sol_ix],
                            Some(&final_keypair.pubkey()),
                            &[&final_keypair],
                            blockhash,
                        );
                        if let Ok(_) = state.rpc.send_and_confirm_transaction(&sol_tx) {
                            info!("Remaining {} SOL sent to destination", (remaining - 900_000) as f64 / 1_000_000_000.0);
                        }
                    }
                    
                    state.db.complete_request(request_id, &sig)?;
                }
                Err(e) => {
                    error!("Swap failed: {}", e);
                    state.db.update_request_status(request_id, RequestStatus::SwapFailed)?;
                    return Err(MazeError::TransactionError(format!("Swap failed: {}", e)));
                }
            }
        }
        return Ok(());
    }

    // Check if this is a diversify request
    if request.receiver_meta.starts_with("diversify:") {
        let routes_json = &request.receiver_meta[10..];
        let routes: Vec<DiversifyRouteOutput> = serde_json::from_str(routes_json)
            .map_err(|e| MazeError::DatabaseError(e.to_string()))?;
        
        info!("Executing diversify to {} destinations", routes.len());
        
        let total_routes = routes.len();
        let mut completed_sigs = Vec::new();
        
        for (idx, route) in routes.iter().enumerate() {
            let is_last = idx == total_routes - 1;
            let route_amount = if is_last {
                // Last route gets remaining balance
                let current_balance = state.rpc.get_balance(&final_keypair.pubkey())?;
                current_balance.saturating_sub(TX_FEE_LAMPORTS)
            } else {
                sol_to_lamports(route.amount)
            };
            
            if route_amount == 0 {
                continue;
            }
            
            let dest_pubkey = Pubkey::from_str(&route.wallet)
                .map_err(|e| MazeError::InvalidParameters(e.to_string()))?;
            
            let recent_blockhash = state.rpc.get_latest_blockhash()
                .map_err(|e| MazeError::RpcError(e.to_string()))?;
            
            let ix = system_instruction::transfer(
                &final_keypair.pubkey(),
                &dest_pubkey,
                route_amount,
            );
            
            let tx = Transaction::new_signed_with_payer(
                &[ix],
                Some(&final_keypair.pubkey()),
                &[&final_keypair],
                recent_blockhash,
            );
            
            match state.rpc.send_and_confirm_transaction_with_spinner(&tx) {
                Ok(sig) => {
                    info!("Diversify route {} complete: {} -> {}", idx, lamports_to_sol(route_amount), route.wallet);
                    completed_sigs.push(sig.to_string());
                }
                Err(e) => {
                    error!("Diversify route {} failed: {}", idx, e);
                }
            }
        }
        
        if !completed_sigs.is_empty() {
            state.db.complete_request(request_id, &completed_sigs.join(","))?;
        }
        return Ok(());
    }

    // Transfer protocol fee to fee wallet first (if not Pro subscriber)
    if request.fee_lamports > 0 {
        let fee_wallet = Pubkey::from_str(FEE_WALLET).unwrap();
        let fee_blockhash = state.rpc.get_latest_blockhash()?;
        let fee_ix = system_instruction::transfer(&final_keypair.pubkey(), &fee_wallet, request.fee_lamports);
        let fee_tx = Transaction::new_signed_with_payer(
            &[fee_ix],
            Some(&final_keypair.pubkey()),
            &[&final_keypair],
            fee_blockhash,
        );
        if let Err(e) = state.rpc.send_and_confirm_transaction(&fee_tx) {
            error!("Fee transfer failed: {}", e);
        } else {
            info!("Fee transferred to fee wallet: {} lamports", request.fee_lamports);
        }
    }

    // Normal transfer to stealth address
    let stealth_pubkey = Pubkey::from_str(&request.stealth_pubkey)
        .map_err(|e| MazeError::InvalidParameters(e.to_string()))?;

    // Recalculate transfer amount after fee
    let current_balance = state.rpc.get_balance(&final_keypair.pubkey())?;
    let transfer_amount = current_balance.saturating_sub(TX_FEE_LAMPORTS);

    let recent_blockhash = state.rpc.get_latest_blockhash().map_err(|e| MazeError::RpcError(e.to_string()))?;

    let ix = system_instruction::transfer(
        &final_keypair.pubkey(),
        &stealth_pubkey,
        transfer_amount,
    );

    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&final_keypair.pubkey()),
        &[&final_keypair],
        recent_blockhash,
    );

    let sig = state.rpc.send_and_confirm_transaction_with_spinner(&tx)?;
    let sig_str = sig.to_string();

    info!("Final transfer complete: {}", sig_str);

    state.db.complete_request(request_id, &sig_str)?;

    state.db.record_completed_transfer(
        &request.receiver_meta,
        &request.stealth_pubkey,
        &request.ephemeral_pubkey,
        transfer_amount,
        &sig_str,
    )?;

    Ok(())
}

// Execute diversify - processes each route through its own maze
async fn execute_diversify(state: SharedState, parent_id: &str) -> Result<(), MazeError> {
    info!("Executing diversify for {}", parent_id);

    state.db.update_diversify_status(parent_id, "processing")?;

    // Get parent request info
    let (deposit_address, _status, keypair_encrypted, total_amount, fee_amount, _mode, _expires, meta_address, maze_config_json) = 
        state.db.get_diversify_request(parent_id)?
            .ok_or_else(|| MazeError::RequestNotFound(parent_id.into()))?;

    // Decrypt deposit keypair
    let keypair_bytes = state.db.decrypt(&keypair_encrypted)?;
    let deposit_keypair = Keypair::from_bytes(&keypair_bytes)
        .map_err(|e| MazeError::EncryptionError(e.to_string()))?;

    // Get routes
    let routes = state.db.get_diversify_routes(parent_id)?;
    let total_routes = routes.len();

    info!("Processing {} routes for diversify {}", total_routes, parent_id);

    // Transfer fee to fee wallet first
    if fee_amount > 0 {
        let fee_wallet = Pubkey::from_str(FEE_WALLET).unwrap();
        let blockhash = state.rpc.get_latest_blockhash()?;
        let fee_ix = system_instruction::transfer(&deposit_keypair.pubkey(), &fee_wallet, fee_amount);
        let fee_tx = Transaction::new_signed_with_payer(
            &[fee_ix],
            Some(&deposit_keypair.pubkey()),
            &[&deposit_keypair],
            blockhash,
        );
        match state.rpc.send_and_confirm_transaction(&fee_tx) {
            Ok(_) => info!("Fee transferred: {} lamports to fee wallet", fee_amount),
            Err(e) => error!("Fee transfer failed: {}", e),
        }
    }

    // Process each route
    let mut completed_count = 0;
    let mut failed_count = 0;

    for (route_idx, route) in routes.iter().enumerate() {
        let (route_id, _idx, _slot, dest_wallet, route_amount, _pct, _child_id, route_status) = route;
        
        if route_status == "completed" {
            completed_count += 1;
            continue;
        }

        let is_last = route_idx == total_routes - 1;
        
        // Calculate amount for this route
        let actual_amount = if is_last {
            // Last route gets remaining balance
            // Reserve: TX fee for this transfer + rent exempt minimum + buffer
            let balance = state.rpc.get_balance(&deposit_keypair.pubkey()).unwrap_or(0);
            let reserve = TX_FEE_LAMPORTS + 1_000_000; // TX fee + rent buffer
            balance.saturating_sub(reserve)
        } else {
            *route_amount
        };

        if actual_amount < 10_000 {
            state.db.update_diversify_route_status(*route_id, "skipped", Some("Amount too low"))?;
            continue;
        }

        info!("Route {}: {} lamports -> {} [last={}]", route_idx, actual_amount, dest_wallet, is_last);
        state.db.update_diversify_route_status(*route_id, "processing", None)?;
        // Generate maze for this route
        let params = if let Some(ref config_json) = maze_config_json {
            if let Ok(config) = serde_json::from_str::<CustomMazeConfig>(config_json) {
                let mut p = MazeParameters::default();
                if let Some(hops) = config.hop_count {
                    p.hop_count = hops.max(5).min(10);
                }
                if let Some(ratio) = config.split_ratio {
                    p.split_ratio = ratio.max(1.1).min(3.0);
                }
                if let Some(ref strategy) = config.merge_strategy {
                    p.merge_strategy = match strategy.as_str() {
                        "early" => MergeStrategy::Early,
                        "late" => MergeStrategy::Late,
                        "middle" => MergeStrategy::Middle,
                        "fibonacci" => MergeStrategy::Fibonacci,
                        _ => MergeStrategy::Random,
                    };
                }
                if let Some(ref pattern) = config.delay_pattern {
                    p.delay_pattern = match pattern.as_str() {
                        "linear" => DelayPattern::Linear,
                        "exponential" => DelayPattern::Exponential,
                        "random" => DelayPattern::Random,
                        "fibonacci" => DelayPattern::Fibonacci,
                        _ => DelayPattern::None,
                    };
                }
                if let Some(ms) = config.delay_ms {
                    p.delay_ms = ms.min(5000);
                }
                if let Some(ref scope) = config.delay_scope {
                    p.delay_scope = match scope.as_str() {
                        "level" => DelayScope::Level,
                        _ => DelayScope::Node,
                    };
                }
                p
            } else {
                let mut p = MazeParameters::random();
                p.hop_count = 7;
                p
            }
        } else {
            let mut p = MazeParameters::random();
            p.hop_count = 7;
            p
        };
        let generator = MazeGenerator::new(params);
        let encrypt_fn = |data: &[u8]| state.db.encrypt(data);

        let maze_amount = actual_amount + TX_FEE_LAMPORTS * 30;
        let maze = match generator.generate(maze_amount, encrypt_fn) {
            Ok(m) => m,
            Err(e) => {
                state.db.update_diversify_route_status(*route_id, "failed", Some(&e.to_string()))?;
                failed_count += 1;
                continue;
            }
        };

        let maze_deposit = maze.get_deposit_node()
            .ok_or_else(|| MazeError::MazeGenerationError("No deposit node".into()))?;

        // Create child maze request
        let child_id = format!("{}_route_{}", parent_id, route_idx);
        let now = chrono::Utc::now().timestamp();
        let maze_json = serde_json::to_string(&maze)
            .map_err(|e| MazeError::DatabaseError(e.to_string()))?;

        let child_request = MazeRequest {
            id: child_id.clone(),
            receiver_meta: dest_wallet.clone(),
            stealth_pubkey: dest_wallet.clone(),
            ephemeral_pubkey: "".into(),
            deposit_address: maze_deposit.address.clone(),
            amount_lamports: actual_amount,
            fee_lamports: 0, // Fee already paid from parent
            status: RequestStatus::Pending,
            maze_graph_json: maze_json,
            created_at: now,
            expires_at: now + EXPIRY_SECONDS,
            completed_at: None,
            final_tx_signature: None,
            error_message: None,
        sender_meta_hash: Some(meta_address.clone()),
        };

        state.db.create_maze_request(&child_request, &maze)?;
        state.db.link_route_to_maze(parent_id, route_idx, &child_id)?;

        // Transfer from parent deposit to child maze deposit
        let maze_deposit_pubkey = Pubkey::from_str(&maze_deposit.address)
            .map_err(|e| MazeError::InvalidParameters(e.to_string()))?;
        
        let blockhash = state.rpc.get_latest_blockhash()?;
        // For last route, transfer all remaining (already calculated in actual_amount)
        // For other routes, add buffer for maze execution
        let transfer_amount = if is_last {
            let balance = state.rpc.get_balance(&deposit_keypair.pubkey()).unwrap_or(0);
            balance.saturating_sub(TX_FEE_LAMPORTS) // Leave only TX fee for this transfer
        } else {
            actual_amount + TX_FEE_LAMPORTS * 25
        };
        let ix = system_instruction::transfer(&deposit_keypair.pubkey(), &maze_deposit_pubkey, transfer_amount);
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&deposit_keypair.pubkey()),
            &[&deposit_keypair],
            blockhash,
        );

        match state.rpc.send_and_confirm_transaction(&tx) {
            Ok(sig) => {
                info!("Route {} funded: {} -> child maze {}", route_idx, sig, child_id);
                
                // Execute the child maze
                match execute_maze(state.clone(), &child_id).await {
                    Ok(_) => {
                        state.db.update_diversify_route_status(*route_id, "completed", None)?;
                        completed_count += 1;
                        info!("Route {} completed successfully", route_idx);
                    }
                    Err(e) => {
                        state.db.update_diversify_route_status(*route_id, "failed", Some(&e.to_string()))?;
                        failed_count += 1;
                        error!("Route {} maze execution failed: {}", route_idx, e);
                    }
                }
            }
            Err(e) => {
                state.db.update_diversify_route_status(*route_id, "failed", Some(&e.to_string()))?;
                failed_count += 1;
                error!("Route {} transfer failed: {}", route_idx, e);
            }
        }
    }

    // Update parent status
    if completed_count == total_routes {
        state.db.complete_diversify_request(parent_id)?;
        info!("Diversify {} completed: {}/{} routes", parent_id, completed_count, total_routes);
    } else if completed_count > 0 {
        state.db.update_diversify_status(parent_id, "partial")?;
        info!("Diversify {} partial: {}/{} completed, {} failed", parent_id, completed_count, total_routes, failed_count);
    } else {
        state.db.update_diversify_status(parent_id, "failed")?;
        error!("Diversify {} failed: all {} routes failed", parent_id, total_routes);
    }

    Ok(())
}

// Execute Jupiter swap via Node.js script
async fn execute_jupiter_swap(
    signer_keypair: &Keypair,
    amount_lamports: u64,
    output_mint: &str,
    destination: &str,
) -> Result<String, String> {
    use std::process::Command;

    let privkey_bs58 = bs58::encode(signer_keypair.to_bytes()).into_string();

    info!("Calling Jupiter swap script: {} lamports -> {} to {}",
        amount_lamports, output_mint, destination);

    let rpc_url = std::env::var("SOLANA_RPC_URL").unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());
    let output = Command::new("node")
        .env("SOLANA_RPC_URL", &rpc_url)
        .arg("./scripts/swap.js")
        .arg(&privkey_bs58)
        .arg(amount_lamports.to_string())
        .arg(output_mint)
        .arg(destination)
        .output()
        .map_err(|e| format!("Failed to run swap script: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Swap script failed: {}", stderr));
    }

    let signature = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(signature)
}

async fn execute_node(state: &SharedState, request_id: &str, node: &sdp_maze::relay::MazeNode) -> Result<(), MazeError> {
    if node.status == "completed" {
        return Ok(());
    }

    // Load maze graph FIRST to get outputs
    let request = state.db.get_maze_request(request_id)?
        .ok_or_else(|| MazeError::RequestNotFound(request_id.into()))?;

    let maze: MazeGraph = serde_json::from_str(&request.maze_graph_json)
        .map_err(|e| MazeError::DatabaseError(e.to_string()))?;

    // Get outputs from maze graph, not from node (which has empty outputs)
    let maze_node = maze.nodes.get(node.index as usize)
        .ok_or_else(|| MazeError::MazeGenerationError(format!("Node {} not found in maze", node.index)))?;
    let outputs = &maze_node.outputs;

    let keypair_bytes = state.db.decrypt(&node.keypair_encrypted)?;
    let keypair = Keypair::from_bytes(&keypair_bytes)
        .map_err(|e| MazeError::CryptoError(e.to_string()))?;

    // Wait for incoming funds (level 0 already has deposit)
    let mut attempts = 0;
    loop {
        let balance = state.rpc.get_balance(&keypair.pubkey()).map_err(|e| MazeError::RpcError(e.to_string()))?;
        if balance > TX_FEE_LAMPORTS {
            info!("Node {} has balance: {} lamports", node.index, balance);
            break;
        }
        attempts += 1;
        if attempts > 120 {
            return Err(MazeError::TransactionError(
                format!("Timeout waiting for funds at node {}", node.index)
            ));
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    // If no outputs, this is the final node - just mark complete
    if outputs.is_empty() {
        state.db.update_node_status(request_id, node.index, "completed", None)?;
        info!("Node {} (final) marked complete", node.index);
        return Ok(());
    }

    let balance = state.rpc.get_balance(&keypair.pubkey()).map_err(|e| MazeError::RpcError(e.to_string()))?;
    let recent_blockhash = state.rpc.get_latest_blockhash().map_err(|e| MazeError::RpcError(e.to_string()))?;

    let num_outputs = outputs.len();
    let total_fees = TX_FEE_LAMPORTS * num_outputs as u64;
    let distributable = balance.saturating_sub(total_fees);
    let per_output = distributable / num_outputs as u64;

    if per_output == 0 {
        return Err(MazeError::InsufficientFunds {
            required: total_fees + num_outputs as u64,
            available: balance,
        });
    }

    // Sequential transfers - one by one, last one drains all
    let mut last_sig = String::new();
    for (i, &output_idx) in outputs.iter().enumerate() {
        if let Some(output_node) = maze.nodes.get(output_idx as usize) {
            let output_pubkey = Pubkey::from_str(&output_node.address)
                .map_err(|e| MazeError::InvalidParameters(e.to_string()))?;

            // Get fresh balance and blockhash for each transfer
            let current_balance = state.rpc.get_balance(&keypair.pubkey())?;
            let blockhash = state.rpc.get_latest_blockhash()?;

            // Last output gets ALL remaining balance (drain account)
            let is_last = i == outputs.len() - 1;
            let transfer_amount = if is_last {
                current_balance.saturating_sub(TX_FEE_LAMPORTS)
            } else {
                per_output
            };

            if transfer_amount == 0 {
                continue;
            }

            let ix = system_instruction::transfer(
                &keypair.pubkey(),
                &output_pubkey,
                transfer_amount,
            );

            let tx = Transaction::new_signed_with_payer(
                &[ix],
                Some(&keypair.pubkey()),
                &[&keypair],
                blockhash,
            );

            let sig = state.rpc.send_and_confirm_transaction_with_spinner(&tx)?;
            last_sig = sig.to_string();

            info!("Node {} transfer {}/{}: {} lamports to {} ({})", 
                node.index, i + 1, outputs.len(), transfer_amount, output_idx, last_sig);
        }
    }

    state.db.update_node_status(
        request_id,
        node.index,
        "completed",
        Some(&last_sig)
    )?;

    info!("Node {} completed all {} transfers", node.index, outputs.len());

    Ok(())
}


// ============ ALIAS HANDLERS ============

async fn resolve_alias_handler(
    State(state): State<SharedState>,
    axum::extract::Query(query): axum::extract::Query<ResolveAliasQuery>,
) -> Result<Json<ResolveAliasResponse>, AppError> {
    let meta = state.db.resolve_alias(&query.alias)?;
    
    Ok(Json(ResolveAliasResponse {
        alias: query.alias,
        meta_address: meta.clone(),
        found: meta.is_some(),
    }))
}

async fn check_alias_handler(
    State(state): State<SharedState>,
    axum::extract::Query(query): axum::extract::Query<CheckAliasQuery>,
) -> Result<Json<CheckAliasResponse>, AppError> {
    let available = state.db.check_alias_available(&query.alias)?;
    
    Ok(Json(CheckAliasResponse {
        alias: query.alias,
        available,
    }))
}

async fn register_alias_handler(
    State(state): State<SharedState>,
    Json(req): Json<RegisterAliasRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Validate alias format
    if !req.alias.starts_with("kl_") || req.alias.len() < 5 || req.alias.len() > 20 {
        return Err(MazeError::InvalidParameters(
            "Alias must start with kl_ and be 5-20 characters".into()
        ).into());
    }
    
    // Check availability
    if !state.db.check_alias_available(&req.alias)? {
        return Err(MazeError::InvalidParameters("Alias already taken".into()).into());
    }
    
    state.db.register_alias(&req.alias, &req.meta_address, &req.owner_meta_hash)?;
    
    Ok(Json(serde_json::json!({
        "success": true,
        "alias": req.alias
    })))
}

async fn list_aliases_handler(
    State(state): State<SharedState>,
    Json(req): Json<ListAliasesRequest>,
) -> Result<Json<ListAliasesResponse>, AppError> {
    let aliases = state.db.list_aliases(&req.owner_meta_hash)?;
    
    Ok(Json(ListAliasesResponse {
        aliases: aliases.into_iter().map(|(alias, meta)| AliasInfo {
            alias,
            meta_address: meta,
        }).collect(),
    }))
}

// ============ WALLET HANDLERS ============

async fn add_wallet_handler(
    State(state): State<SharedState>,
    Json(req): Json<AddWalletRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Validate slot (1-5)
    if req.slot < 1 || req.slot > 5 {
        return Err(MazeError::InvalidParameters("Slot must be 1-5".into()).into());
    }
    
    // Validate wallet address
    if Pubkey::from_str(&req.wallet_address).is_err() {
        return Err(MazeError::InvalidParameters("Invalid wallet address".into()).into());
    }
    
    // Hash the meta address
    let mut hasher = Sha256::new();
    hasher.update(req.owner_meta_hash.as_bytes());
    let meta_hash = format!("{:x}", hasher.finalize());
    
    state.db.add_destination_wallet(&meta_hash, req.slot, &req.wallet_address)?;
    
    Ok(Json(serde_json::json!({
        "success": true,
        "slot": req.slot,
        "wallet": req.wallet_address
    })))
}

async fn delete_wallet_handler(
    State(state): State<SharedState>,
    Json(req): Json<DeleteWalletRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Hash the meta address
    let mut hasher = Sha256::new();
    hasher.update(req.owner_meta_hash.as_bytes());
    let meta_hash = format!("{:x}", hasher.finalize());
    
    state.db.delete_destination_wallet(&meta_hash, req.slot)?;
    
    Ok(Json(serde_json::json!({
        "success": true,
        "slot": req.slot
    })))
}

async fn list_wallets_handler(
    State(state): State<SharedState>,
    Json(req): Json<ListWalletsRequest>,
) -> Result<Json<ListWalletsResponse>, AppError> {
    // Hash the meta address
    let mut hasher = Sha256::new();
    hasher.update(req.owner_meta_hash.as_bytes());
    let meta_hash = format!("{:x}", hasher.finalize());
    
    let wallets = state.db.list_destination_wallets(&meta_hash)?;
    
    Ok(Json(ListWalletsResponse {
        wallets: wallets.into_iter().map(|(slot, addr)| WalletInfo {
            slot,
            address: addr,
        }).collect(),
    }))
}

// ============ CLAIM HANDLER ============

async fn claim_handler(
    State(state): State<SharedState>,
    Json(req): Json<ClaimRequest>,
) -> Result<Json<ClaimResponse>, AppError> {
    // Decode and submit the signed transaction
    let tx_bytes = match bs58::decode(&req.signed_tx).into_vec() {
        Ok(b) => b,
        Err(_) => return Ok(Json(ClaimResponse {
            success: false,
            tx_signature: None,
            error: Some("Invalid transaction encoding".into()),
        })),
    };
    
    let tx: Transaction = match bincode::deserialize(&tx_bytes) {
        Ok(t) => t,
        Err(_) => return Ok(Json(ClaimResponse {
            success: false,
            tx_signature: None,
            error: Some("Invalid transaction format".into()),
        })),
    };
    
    match state.rpc.send_and_confirm_transaction(&tx) {
        Ok(sig) => Ok(Json(ClaimResponse {
            success: true,
            tx_signature: Some(sig.to_string()),
            error: None,
        })),
        Err(e) => Ok(Json(ClaimResponse {
            success: false,
            tx_signature: None,
            error: Some(e.to_string()),
        })),
    }
}

// ============ RECOVER HANDLER ============

async fn recover_handler(
    State(state): State<SharedState>,
    Json(req): Json<RecoverRequest>,
) -> Result<Json<RecoverResponse>, AppError> {
    let destination = Pubkey::from_str(&req.destination)
        .map_err(|e| MazeError::InvalidParameters(e.to_string()))?;
    
    let mut total_recovered: u64 = 0;
    let mut signatures = vec![];

    // Check if this is a diversify parent request (div_xxx without _route_)
    if req.request_id.starts_with("div_") && !req.request_id.contains("_route_") {
        info!("Recovering diversify request: {}", req.request_id);
        
        // Get diversify parent request
        if let Ok(Some((deposit_address, status, keypair_encrypted, _, _, _, _, owner_meta, _))) = 
            state.db.get_diversify_request(&req.request_id) 
        {
            // Validate ownership - destination must be a wallet registered to the owner
            let owner_wallets = state.db.list_destination_wallets(&owner_meta).unwrap_or_default();
            let is_owner_wallet = owner_wallets.iter().any(|(_, addr)| addr == &req.destination);
            if !is_owner_wallet {
                return Ok(Json(RecoverResponse {
                    success: false,
                    recovered_amount: None,
                    tx_signatures: vec![],
                    error: Some("Unauthorized: destination is not a registered wallet of the request owner".into()),
                }));
            }

            // Recover from parent deposit address
            let keypair_bytes = state.db.decrypt(&keypair_encrypted)?;
            let keypair = Keypair::from_bytes(&keypair_bytes)
                .map_err(|e| MazeError::CryptoError(e.to_string()))?;
            
            let balance = state.rpc.get_balance(&keypair.pubkey())
                .map_err(|e| MazeError::RpcError(e.to_string()))?;
            
            if balance > TX_FEE_LAMPORTS {
                let transfer_amount = balance - TX_FEE_LAMPORTS;
                let blockhash = state.rpc.get_latest_blockhash()
                    .map_err(|e| MazeError::RpcError(e.to_string()))?;
                
                let ix = system_instruction::transfer(&keypair.pubkey(), &destination, transfer_amount);
                let tx = Transaction::new_signed_with_payer(
                    &[ix],
                    Some(&keypair.pubkey()),
                    &[&keypair],
                    blockhash,
                );
                
                match state.rpc.send_and_confirm_transaction_with_spinner(&tx) {
                    Ok(sig) => {
                        total_recovered += transfer_amount;
                        signatures.push(sig.to_string());
                        info!("Recovered {} lamports from diversify parent deposit: {}", transfer_amount, sig);
                    }
                    Err(e) => {
                        error!("Failed to recover from diversify parent: {}", e);
                    }
                }
            }
            
            // Also recover from all child routes
            if let Ok(routes) = state.db.get_diversify_routes(&req.request_id) {
                for (_, route_idx, _, _, _, _, child_id_opt, _) in routes {
                    if let Some(child_id) = child_id_opt {
                        // Recover from child maze nodes
                        if let Ok(nodes) = state.db.get_request_nodes(&child_id) {
                            for node in &nodes {
                                let keypair_bytes = match state.db.decrypt(&node.keypair_encrypted) {
                                    Ok(b) => b,
                                    Err(_) => continue,
                                };
                                let keypair = match Keypair::from_bytes(&keypair_bytes) {
                                    Ok(k) => k,
                                    Err(_) => continue,
                                };
                                
                                let balance = state.rpc.get_balance(&keypair.pubkey()).unwrap_or(0);
                                
                                if balance > TX_FEE_LAMPORTS {
                                    let transfer_amount = balance - TX_FEE_LAMPORTS;
                                    if let Ok(blockhash) = state.rpc.get_latest_blockhash() {
                                        let ix = system_instruction::transfer(&keypair.pubkey(), &destination, transfer_amount);
                                        let tx = Transaction::new_signed_with_payer(
                                            &[ix],
                                            Some(&keypair.pubkey()),
                                            &[&keypair],
                                            blockhash,
                                        );
                                        
                                        if let Ok(sig) = state.rpc.send_and_confirm_transaction_with_spinner(&tx) {
                                            total_recovered += transfer_amount;
                                            signatures.push(sig.to_string());
                                            info!("Recovered {} lamports from route {} node {}: {}", 
                                                  transfer_amount, route_idx, node.index, sig);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            
            // Update diversify status
            if total_recovered > 0 {
                state.db.update_diversify_status(&req.request_id, "recovered").ok();
            }
        }
        
        return Ok(Json(RecoverResponse {
            success: total_recovered > 0,
            recovered_amount: Some(total_recovered),
            tx_signatures: signatures,
            error: if total_recovered == 0 { Some("No funds to recover".into()) } else { None },
        }));
    }
    
    // Standard maze request recovery
    let request = state.db.get_maze_request(&req.request_id)?
        .ok_or_else(|| MazeError::RequestNotFound(req.request_id.clone()))?;
    
    // Validate ownership - destination must be a wallet registered to the sender
    if let Some(ref sender_hash) = request.sender_meta_hash {
        let sender_wallets = state.db.list_destination_wallets(sender_hash).unwrap_or_default();
        let is_sender_wallet = sender_wallets.iter().any(|(_, addr)| addr == &req.destination);
        if !is_sender_wallet {
            return Ok(Json(RecoverResponse {
                success: false,
                recovered_amount: None,
                tx_signatures: vec![],
                error: Some("Unauthorized: destination is not a registered wallet of the request owner".into()),
            }));
        }
    }

    // Only allow recovery for failed/processing/swap_failed requests
    if request.status != RequestStatus::Failed 
        && request.status != RequestStatus::Processing 
        && request.status != RequestStatus::SwapFailed {
        return Ok(Json(RecoverResponse {
            success: false,
            recovered_amount: None,
            tx_signatures: vec![],
            error: Some("Request not in recoverable state".into()),
        }));
    }
    
    let nodes = state.db.get_request_nodes(&req.request_id)?;
    
    // Check each node for remaining balance
    for node in &nodes {
        let keypair_bytes = state.db.decrypt(&node.keypair_encrypted)?;
        let keypair = Keypair::from_bytes(&keypair_bytes)
            .map_err(|e| MazeError::CryptoError(e.to_string()))?;
        
        let balance = state.rpc.get_balance(&keypair.pubkey()).map_err(|e| MazeError::RpcError(e.to_string()))?;
        
        if balance > TX_FEE_LAMPORTS {
            let transfer_amount = balance - TX_FEE_LAMPORTS;
            let recent_blockhash = state.rpc.get_latest_blockhash().map_err(|e| MazeError::RpcError(e.to_string()))?;
            
            let ix = system_instruction::transfer(
                &keypair.pubkey(),
                &destination,
                transfer_amount,
            );
            
            let tx = Transaction::new_signed_with_payer(
                &[ix],
                Some(&keypair.pubkey()),
                &[&keypair],
                recent_blockhash,
            );
            
            match state.rpc.send_and_confirm_transaction_with_spinner(&tx) {
                Ok(sig) => {
                    total_recovered += transfer_amount;
                    signatures.push(sig.to_string());
                    info!("Recovered {} lamports from node {}: {}", transfer_amount, node.index, sig);
                }
                Err(e) => {
                    error!("Failed to recover from node {}: {}", node.index, e);
                }
            }
        }
    }
    
    // Update request status
    if total_recovered > 0 {
        state.db.update_request_status(&req.request_id, RequestStatus::Recovered)?;
    }
    
    Ok(Json(RecoverResponse {
        success: total_recovered > 0,
        recovered_amount: Some(total_recovered),
        tx_signatures: signatures,
        error: None,
    }))
}


// ============ SWAP HANDLER ============

async fn swap_request_handler(
    State(state): State<SharedState>,
    Json(req): Json<SwapRequest>,
) -> Result<Json<SwapResponse>, AppError> {
    info!("Swap request: {} SOL -> {} to {}", req.amount_sol, req.token_mint, req.destination);

    if req.amount_sol < MIN_AMOUNT_SOL {
        return Ok(Json(SwapResponse {
            success: false,
            error: Some(format!("Minimum amount is {} SOL", MIN_AMOUNT_SOL)),
            ..Default::default()
        }));
    }

    // Validate destination address
    if Pubkey::from_str(&req.destination).is_err() {
        return Ok(Json(SwapResponse {
            success: false,
            error: Some("Invalid destination address".into()),
            ..Default::default()
        }));
    }

    let amount_lamports = sol_to_lamports(req.amount_sol);
    
    // Hash sender meta address for subscription check
    let mut hasher = Sha256::new();
    hasher.update(req.sender_meta_hash.as_bytes());
    let sender_hash = format!("{:x}", hasher.finalize());

    // Check if sender is Pro subscriber (fee = 0)
    let fee_lamports = if state.db.is_pro_subscriber(&sender_hash) {
        info!("Pro subscriber detected, fee = 0");
        0
    } else {
        (amount_lamports as f64 * FEE_PERCENT / 100.0) as u64
    };


    // Generate maze parameters (custom config or random)
    let params = if let Some(config) = req.maze_config {
        let mut p = MazeParameters::default();
        p.hop_count = config.hop_count.unwrap_or(7).max(MIN_HOPS).min(MAX_HOPS);
        if let Some(ratio) = config.split_ratio {
            p.split_ratio = ratio.max(1.1).min(3.0);
        }
        if let Some(ref strategy) = config.merge_strategy {
            p.merge_strategy = match strategy.as_str() {
                "early" => MergeStrategy::Early,
                "late" => MergeStrategy::Late,
                "middle" => MergeStrategy::Middle,
                "fibonacci" => MergeStrategy::Fibonacci,
                _ => MergeStrategy::Random,
            };
        }
        if let Some(ref pattern) = config.delay_pattern {
            p.delay_pattern = match pattern.as_str() {
                "linear" => DelayPattern::Linear,
                "exponential" => DelayPattern::Exponential,
                "random" => DelayPattern::Random,
                "fibonacci" => DelayPattern::Fibonacci,
                _ => DelayPattern::None,
            };
        }
        if let Some(ms) = config.delay_ms {
            p.delay_ms = ms.min(5000);
        }
        if let Some(ref scope) = config.delay_scope {
            p.delay_scope = match scope.as_str() {
                "level" => DelayScope::Level,
                _ => DelayScope::Node,
            };
        }
        p
    } else {
        let mut p = MazeParameters::random();
        p.hop_count = 7; // Default for swap
        p
    };

    // Create special receiver_meta for swap: "swap:{token_mint}:{destination}"
    let swap_meta = format!("swap:{}:{}", req.token_mint, req.destination);

    // Generate maze graph
    let generator = MazeGenerator::new(params.clone());
    let encrypt_fn = |data: &[u8]| state.db.encrypt(data);

    let total_with_fees = amount_lamports + fee_lamports + (TX_FEE_LAMPORTS * 50);
    let maze = generator.generate(total_with_fees, encrypt_fn)?;

    let deposit_node = maze.get_deposit_node()
        .ok_or_else(|| MazeError::MazeGenerationError("No deposit node".into()))?;

    let request_id = format!("swap_{}", hex::encode(&rand::random::<[u8; 8]>()));
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + EXPIRY_SECONDS;

    let maze_json = serde_json::to_string(&maze)
        .map_err(|e| MazeError::DatabaseError(e.to_string()))?;

    // Create request with swap meta
    let request = MazeRequest {
        id: request_id.clone(),
        receiver_meta: swap_meta,
        stealth_pubkey: req.destination.clone(), // For swap, this is the destination
        ephemeral_pubkey: req.token_mint.clone(), // Store token mint here
        deposit_address: deposit_node.address.clone(),
        amount_lamports,
        fee_lamports,
        status: RequestStatus::Pending,
        maze_graph_json: maze_json,
        created_at: now,
        expires_at,
        completed_at: None,
        final_tx_signature: None,
        error_message: None,
        sender_meta_hash: Some(sender_hash.clone()),
    };

    state.db.create_maze_request(&request, &maze)?;

    info!("Created swap request {} with {} nodes", request_id, maze.nodes.len());

    let total_deposit = amount_lamports + fee_lamports + (TX_FEE_LAMPORTS * maze.nodes.len() as u64);

    Ok(Json(SwapResponse {
        success: true,
        request_id: Some(request_id),
        deposit_address: Some(deposit_node.address.clone()),
        deposit_amount: Some(lamports_to_sol(total_deposit)),
        estimated_output: Some("TBD".into()), // Would need Jupiter quote
        fee: Some(lamports_to_sol(fee_lamports)),
        expires_in: Some(EXPIRY_SECONDS),
        maze_preview: Some(MazePreview {
            total_nodes: maze.nodes.len(),
            total_levels: maze.total_levels,
            total_transactions: maze.total_transactions,
            estimated_time_seconds: (maze.total_levels as u16) * 4,
        }),
        error: None,
    }))
}

impl Default for SwapResponse {
    fn default() -> Self {
        Self {
            success: false,
            request_id: None,
            deposit_address: None,
            deposit_amount: None,
            estimated_output: None,
            fee: None,
            expires_in: None,
            maze_preview: None,
            error: None,
        }
    }
}

// ============ MAZE PREFERENCES HANDLERS ============

async fn get_preferences_handler(
    State(state): State<SharedState>,
    Json(req): Json<GetPreferencesRequest>,
) -> Result<Json<GetPreferencesResponse>, AppError> {
    // Hash meta address
    let mut hasher = Sha256::new();
    hasher.update(req.meta_address.as_bytes());
    let meta_hash = format!("{:x}", hasher.finalize());

    match state.db.get_maze_preferences(&meta_hash) {
        Ok(Some(prefs)) => {
            Ok(Json(GetPreferencesResponse {
                success: true,
                preferences: Some(MazePreferencesData {
                    hop_count: prefs.hop_count as u8,
                    split_ratio: prefs.split_ratio,
                    merge_strategy: prefs.merge_strategy,
                    delay_pattern: prefs.delay_pattern,
                    delay_ms: prefs.delay_ms as u64,
                    delay_scope: prefs.delay_scope,
                    updated_at: prefs.updated_at,
                }),
                error: None,
            }))
        }
        Ok(None) => {
            Ok(Json(GetPreferencesResponse {
                success: true,
                preferences: None,
                error: None,
            }))
        }
        Err(e) => {
            Ok(Json(GetPreferencesResponse {
                success: false,
                preferences: None,
                error: Some(e.to_string()),
            }))
        }
    }
}

async fn save_preferences_handler(
    State(state): State<SharedState>,
    Json(req): Json<SavePreferencesRequest>,
) -> Result<Json<SavePreferencesResponse>, AppError> {
    // Hash meta address
    let mut hasher = Sha256::new();
    hasher.update(req.meta_address.as_bytes());
    let meta_hash = format!("{:x}", hasher.finalize());

    let now = chrono::Utc::now().timestamp();

    // Get existing or create default
    let existing = state.db.get_maze_preferences(&meta_hash).ok().flatten();

    let prefs = MazePreferencesRow {
        owner_meta_hash: meta_hash,
        hop_count: req.hop_count.map(|h| h as i32).unwrap_or_else(|| existing.as_ref().map(|e| e.hop_count).unwrap_or(10)),
        split_ratio: req.split_ratio.unwrap_or_else(|| existing.as_ref().map(|e| e.split_ratio).unwrap_or(1.618)),
        merge_strategy: req.merge_strategy.unwrap_or_else(|| existing.as_ref().map(|e| e.merge_strategy.clone()).unwrap_or_else(|| "random".to_string())),
        delay_pattern: req.delay_pattern.unwrap_or_else(|| existing.as_ref().map(|e| e.delay_pattern.clone()).unwrap_or_else(|| "none".to_string())),
        delay_ms: req.delay_ms.map(|d| d as i64).unwrap_or_else(|| existing.as_ref().map(|e| e.delay_ms).unwrap_or(0)),
        delay_scope: req.delay_scope.unwrap_or_else(|| existing.as_ref().map(|e| e.delay_scope.clone()).unwrap_or_else(|| "node".to_string())),
        updated_at: now,
    };

    match state.db.save_maze_preferences(&prefs) {
        Ok(_) => {
            info!("Saved maze preferences for user");
            Ok(Json(SavePreferencesResponse {
                success: true,
                error: None,
            }))
        }
        Err(e) => {
            Ok(Json(SavePreferencesResponse {
                success: false,
                error: Some(e.to_string()),
            }))
        }
    }
}

// ============ DIVERSIFY HANDLER ============

async fn diversify_request_handler(
    State(state): State<SharedState>,
    Json(req): Json<DiversifyRequest>,
) -> Result<Json<DiversifyResponse>, AppError> {
    info!("Diversify request: {} SOL, mode: {}, routes: {}", 
          req.total_amount, req.distribution_mode, req.routes.len());

    // Validation
    if req.total_amount < 1.0 {
        return Ok(Json(DiversifyResponse {
            success: false,
            error: Some("Minimum 1 SOL for diversification".into()),
            ..Default::default()
        }));
    }

    if req.routes.len() < 2 {
        return Ok(Json(DiversifyResponse {
            success: false,
            error: Some("Minimum 2 destinations required".into()),
            ..Default::default()
        }));
    }

    if req.routes.len() > 5 {
        return Ok(Json(DiversifyResponse {
            success: false,
            error: Some("Maximum 5 destinations allowed".into()),
            ..Default::default()
        }));
    }

    // Hash meta address (same as KausaLayer)
    let mut hasher = Sha256::new();
    hasher.update(req.meta_address.as_bytes());
    let meta_hash = format!("{:x}", hasher.finalize());

    // Get destination wallets
    let wallets = state.db.list_destination_wallets(&meta_hash)?;
    let wallet_map: std::collections::HashMap<i32, String> = wallets.into_iter().collect();

    // Validate all slots exist
    for route in &req.routes {
        if !wallet_map.contains_key(&route.slot) {
            return Ok(Json(DiversifyResponse {
                success: false,
                error: Some(format!("Slot {} is empty", route.slot)),
                ..Default::default()
            }));
        }
    }

    let total_lamports = sol_to_lamports(req.total_amount);
    
    // Check if subscriber (fee waiver)
    let is_subscriber = state.db.check_shared_subscription(&meta_hash).unwrap_or(false);
    let fee_lamports = if is_subscriber {
        info!("Subscriber detected - fee waived for diversify");
        0
    } else {
        (total_lamports as f64 * FEE_PERCENT / 100.0) as u64
    };

    // Calculate distribution
    let mut route_outputs: Vec<DiversifyRouteOutput> = Vec::new();
    
    match req.distribution_mode.as_str() {
        "equal" => {
            let amount_each = req.total_amount / req.routes.len() as f64;
            let pct_each = 100.0 / req.routes.len() as f64;
            for route in &req.routes {
                let wallet = wallet_map.get(&route.slot).unwrap().clone();
                route_outputs.push(DiversifyRouteOutput {
                    slot: route.slot,
                    wallet,
                    amount: amount_each,
                    percentage: Some(pct_each),
                });
            }
        },
        "percentage" => {
            let total_pct: f64 = req.routes.iter().map(|r| r.value).sum();
            if (total_pct - 100.0).abs() > 0.01 {
                return Ok(Json(DiversifyResponse {
                    success: false,
                    error: Some(format!("Percentages must total 100% (got {}%)", total_pct)),
                    ..Default::default()
                }));
            }
            for route in &req.routes {
                let wallet = wallet_map.get(&route.slot).unwrap().clone();
                let amount = req.total_amount * (route.value / 100.0);
                route_outputs.push(DiversifyRouteOutput {
                    slot: route.slot,
                    wallet,
                    amount,
                    percentage: Some(route.value),
                });
            }
        },
        "fixed" => {
            let total_fixed: f64 = req.routes.iter().map(|r| r.value).sum();
            if (total_fixed - req.total_amount).abs() > 0.001 {
                return Ok(Json(DiversifyResponse {
                    success: false,
                    error: Some(format!("Fixed amounts must total {} SOL", req.total_amount)),
                    ..Default::default()
                }));
            }
            for route in &req.routes {
                let wallet = wallet_map.get(&route.slot).unwrap().clone();
                route_outputs.push(DiversifyRouteOutput {
                    slot: route.slot,
                    wallet,
                    amount: route.value,
                    percentage: None,
                });
            }
        },
        _ => {
            return Ok(Json(DiversifyResponse {
                success: false,
                error: Some("Invalid mode. Use: equal, percentage, or fixed".into()),
                ..Default::default()
            }));
        }
    }

    // Generate deposit keypair for parent request
    let deposit_keypair = Keypair::new();
    let deposit_address = deposit_keypair.pubkey().to_string();
    let encrypted_keypair = state.db.encrypt(&deposit_keypair.to_bytes())?;

    // Calculate network fees: each route needs its own maze (estimate ~20 nodes per maze)
    // Each node needs ~2 TX fees (in + out), plus buffer for safety
    let estimated_nodes_per_route: u64 = 25; // Slightly higher estimate
    let route_count = route_outputs.len() as u64;
    // Fee per route: nodes * 2 TX fees + transfer to child deposit + buffer
    let fee_per_route = TX_FEE_LAMPORTS * estimated_nodes_per_route * 3;
    // Total: (fee per route * routes) + parent fee transfer + safety buffer (0.01 SOL)
    let network_fee = (fee_per_route * route_count) + TX_FEE_LAMPORTS * 10 + 10_000_000;
    let total_deposit = total_lamports + fee_lamports + network_fee;

    // Create parent diversify request
    let request_id = format!("div_{}", hex::encode(&rand::random::<[u8; 8]>()));
    
    state.db.create_diversify_request(
        &request_id,
        &meta_hash,
        &deposit_address,
        &encrypted_keypair,
        total_lamports,
        fee_lamports,
        route_outputs.len(),
        &req.distribution_mode,
        EXPIRY_SECONDS,
        req.maze_config.as_ref().map(|c| serde_json::to_string(c).ok()).flatten().as_deref(),
    )?;

    // Add routes to database
    for (idx, route) in route_outputs.iter().enumerate() {
        state.db.add_diversify_route(
            &request_id,
            idx,
            route.slot,
            &route.wallet,
            sol_to_lamports(route.amount),
            route.percentage,
        )?;
    }

    let route_count = route_outputs.len();
    
    info!("Created diversify request {} with {} routes, deposit: {}", 
          request_id, route_count, deposit_address);

    Ok(Json(DiversifyResponse {
        success: true,
        request_id: Some(request_id),
        deposit_address: Some(deposit_address),
        deposit_amount: Some(lamports_to_sol(total_deposit)),
        total_amount: Some(req.total_amount),
        fee: Some(lamports_to_sol(fee_lamports)),
        routes: Some(route_outputs),
        expires_in: Some(EXPIRY_SECONDS),
        maze_preview: Some(MazePreview {
            total_nodes: (estimated_nodes_per_route as usize) * route_count,
            total_levels: 7,
            total_transactions: ((estimated_nodes_per_route as usize) * route_count) as u16,
            estimated_time_seconds: (7 * route_count * 4) as u16,
        }),
        error: None,
    }))
}

impl Default for DiversifyResponse {
    fn default() -> Self {
        Self {
            success: false,
            request_id: None,
            deposit_address: None,
            deposit_amount: None,
            total_amount: None,
            fee: None,
            routes: None,
            expires_in: None,
            maze_preview: None,
            error: None,
        }
    }
}

async fn autopurge_task(state: SharedState) {
    info!("Starting autopurge task");
    
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;

        match state.db.autopurge() {
            Ok(count) => {
                if count > 0 {
                    info!("Autopurged {} old requests", count);
                }
            }
            Err(e) => error!("Autopurge error: {}", e),
        }
    }
}

// ============ MAIN ============

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("sdp_maze=info".parse().unwrap())
        )
        .init();

    info!("Starting SDP Maze Relay Server");

    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());
    let port: u16 = std::env::var("RELAY_PORT")
        .unwrap_or_else(|_| "3031".to_string())
        .parse()
        .unwrap_or(3031);
    let api_key = std::env::var("API_KEY").ok();

    let db = RelayDatabase::new(None)?;
    info!("Database initialized");

    let rpc = RpcClient::new_with_commitment(
        rpc_url.clone(),
        CommitmentConfig::confirmed(),
    );
    info!("RPC client connected to {}", rpc_url);

    let state = Arc::new(AppState {
        db,
        rpc,
        config: Config::default(),
        api_key,
    });

    let state_clone = state.clone();
    tokio::spawn(async move {
        deposit_monitor_task(state_clone).await;
    });

    let state_clone = state.clone();
    tokio::spawn(async move {
        autopurge_task(state_clone).await;
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/blockhash", get(blockhash_handler))
        .route("/submit", post(submit_handler))
        .route("/api/v1/transfer", post(create_transfer_handler))
        .route("/api/v1/transfer/:request_id", get(get_status_handler))
        .route("/api/v1/transfer/:request_id/maze", get(get_maze_graph_handler))
        .route("/api/v1/scan", post(scan_handler))
        // Alias routes
        .route("/api/v1/alias/resolve", get(resolve_alias_handler))
        .route("/api/v1/alias/check", get(check_alias_handler))
        .route("/api/v1/alias/register", post(register_alias_handler))
        .route("/api/v1/alias/list", post(list_aliases_handler))
        // Wallet routes
        .route("/api/v1/wallet/add", post(add_wallet_handler))
        .route("/api/v1/wallet/delete", post(delete_wallet_handler))
        .route("/api/v1/wallet/list", post(list_wallets_handler))
        // Claim route
        .route("/api/v1/claim", post(claim_handler))
        // Recovery route
        .route("/api/v1/recover", post(recover_handler))
        // Swap routes
        .route("/api/v1/swap/request", post(swap_request_handler))
        .route("/api/v1/swap/status/:request_id", get(get_status_handler))
        // Diversify routes
        .route("/api/v1/diversify/request", post(diversify_request_handler))
        .route("/api/v1/diversify/status/:request_id", get(get_status_handler))
        // Maze preferences
        .route("/api/v1/preferences/maze", post(get_preferences_handler))
        .route("/api/v1/preferences/maze/save", post(save_preferences_handler))
        .layer(cors)
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("Listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
