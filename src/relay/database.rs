//! Database layer for SDP Maze
//!
//! Uses SQLite for persistent storage of maze requests

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use aes_gcm::aead::{Aead, KeyInit};
use sha2::{Sha256, Digest};

use crate::config::{DB_PATH, SHARED_DB_PATH, AUTOPURGE_SECONDS};
use crate::relay::maze::{MazeGraph, MazeNode};
use crate::error::{MazeError, Result};

/// Request status enum
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RequestStatus {
    Pending,
    DepositReceived,
    Processing,
    Completed,
    Failed,
    Expired,
    Recovered,
    SwapFailed,
}

impl RequestStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::DepositReceived => "deposit_received",
            Self::Processing => "processing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::Recovered => "recovered",
            Self::SwapFailed => "swap_failed",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "deposit_received" => Self::DepositReceived,
            "processing" => Self::Processing,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "expired" => Self::Expired,
            "recovered" => Self::Recovered,
            "swap_failed" => Self::SwapFailed,
            _ => Self::Pending,
        }
    }
}

/// A maze transfer request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MazeRequest {
    pub id: String,
    pub receiver_meta: String,
    pub stealth_pubkey: String,
    pub ephemeral_pubkey: String,
    pub deposit_address: String,
    pub amount_lamports: u64,
    pub fee_lamports: u64,
    pub status: RequestStatus,
    pub maze_graph_json: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub completed_at: Option<i64>,
    pub final_tx_signature: Option<String>,
    pub error_message: Option<String>,
    pub sender_meta_hash: Option<String>,
}

/// User maze preferences (for KAUSA holders)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MazePreferencesRow {
    pub owner_meta_hash: String,
    pub hop_count: i32,
    pub split_ratio: f64,
    pub merge_strategy: String,
    pub delay_pattern: String,
    pub delay_ms: i64,
    pub delay_scope: String,
    pub updated_at: i64,
}

/// Database wrapper with encryption
pub struct RelayDatabase {
    conn: Arc<Mutex<Connection>>,
    shared_conn: Arc<Mutex<Connection>>,
    encryption_key: [u8; 32],
}

// Re-export MazeNode for mod.rs
impl RelayDatabase {
    /// Create new database connection
    pub fn new(db_path: Option<&str>) -> Result<Self> {
        let path = db_path.unwrap_or(DB_PATH);
        let conn = Connection::open(path)?;
        
        // Get encryption key from env
        let key_str = std::env::var("DB_ENCRYPTION_KEY")
            .unwrap_or_else(|_| "default_key_change_in_production_32b".to_string());
        let mut hasher = Sha256::new();
        hasher.update(key_str.as_bytes());
        let key_bytes: [u8; 32] = hasher.finalize().into();

        // Open shared database for aliases, wallets
        let shared_conn = Connection::open(SHARED_DB_PATH)?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            shared_conn: Arc::new(Mutex::new(shared_conn)),
            encryption_key: key_bytes,
        };

        db.init_tables()?;
        Ok(db)
    }

    /// Initialize database tables
    fn init_tables(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        conn.execute_batch(r#"
            -- Main maze requests table
            CREATE TABLE IF NOT EXISTS maze_requests (
                id TEXT PRIMARY KEY,
                receiver_meta TEXT NOT NULL,
                stealth_pubkey TEXT NOT NULL,
                ephemeral_pubkey TEXT NOT NULL,
                deposit_address TEXT NOT NULL,
                amount_lamports INTEGER NOT NULL,
                fee_lamports INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                maze_graph_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                completed_at INTEGER,
                final_tx_signature TEXT,
                error_message TEXT
            );

            -- Maze nodes table (for individual node tracking)
            CREATE TABLE IF NOT EXISTS maze_nodes (
                request_id TEXT NOT NULL,
                node_index INTEGER NOT NULL,
                level INTEGER NOT NULL,
                address TEXT NOT NULL,
                keypair_encrypted BLOB NOT NULL,
                amount_in INTEGER NOT NULL,
                amount_out INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                tx_in_signature TEXT,
                tx_out_signatures TEXT,
                PRIMARY KEY (request_id, node_index),
                FOREIGN KEY (request_id) REFERENCES maze_requests(id)
            );

            -- Subscriptions table
            CREATE TABLE IF NOT EXISTS subscriptions (
                wallet_address TEXT PRIMARY KEY,
                subscription_type TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                tx_signature TEXT NOT NULL,
                amount_paid TEXT NOT NULL
            );

            -- API keys table
            CREATE TABLE IF NOT EXISTS api_keys (
                key_hash TEXT PRIMARY KEY,
                wallet_address TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_used INTEGER,
                request_count INTEGER DEFAULT 0
            );

            -- Completed transfers (for receiver scanning)
            CREATE TABLE IF NOT EXISTS completed_transfers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                receiver_meta TEXT NOT NULL,
                stealth_pubkey TEXT NOT NULL,
                ephemeral_pubkey TEXT NOT NULL,
                amount_lamports INTEGER NOT NULL,
                tx_signature TEXT NOT NULL,
                completed_at INTEGER NOT NULL
            );

            -- Diversify parent requests table
            CREATE TABLE IF NOT EXISTS diversify_requests (
                id TEXT PRIMARY KEY,
                meta_address TEXT NOT NULL,
                deposit_address TEXT NOT NULL,
                deposit_keypair_encrypted BLOB NOT NULL,
                total_amount INTEGER NOT NULL,
                fee_amount INTEGER NOT NULL,
                route_count INTEGER NOT NULL,
                distribution_mode TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                completed_at INTEGER,
                maze_config_json TEXT
            );

            -- Diversify routes table (links to child maze requests)
            CREATE TABLE IF NOT EXISTS diversify_routes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                parent_id TEXT NOT NULL,
                route_index INTEGER NOT NULL,
                child_request_id TEXT,
                destination_slot INTEGER NOT NULL,
                destination_wallet TEXT NOT NULL,
                amount INTEGER NOT NULL,
                percentage REAL,
                status TEXT NOT NULL DEFAULT 'pending',
                error_message TEXT,
                completed_at INTEGER,
                FOREIGN KEY (parent_id) REFERENCES diversify_requests(id),
                FOREIGN KEY (child_request_id) REFERENCES maze_requests(id)
            );

            -- Indexes
            CREATE INDEX IF NOT EXISTS idx_maze_requests_status ON maze_requests(status);
            CREATE INDEX IF NOT EXISTS idx_maze_requests_deposit ON maze_requests(deposit_address);
            CREATE INDEX IF NOT EXISTS idx_maze_requests_expires ON maze_requests(expires_at);
            CREATE INDEX IF NOT EXISTS idx_maze_nodes_address ON maze_nodes(address);
            CREATE INDEX IF NOT EXISTS idx_completed_receiver ON completed_transfers(receiver_meta);
            CREATE INDEX IF NOT EXISTS idx_diversify_requests_status ON diversify_requests(status);
            CREATE INDEX IF NOT EXISTS idx_diversify_routes_parent ON diversify_routes(parent_id);

            -- User maze preferences (for KAUSA holders)
            CREATE TABLE IF NOT EXISTS maze_preferences (
                owner_meta_hash TEXT PRIMARY KEY,
                hop_count INTEGER DEFAULT 10,
                split_ratio REAL DEFAULT 1.618,
                merge_strategy TEXT DEFAULT 'random',
                delay_pattern TEXT DEFAULT 'none',
                delay_ms INTEGER DEFAULT 0,
                delay_scope TEXT DEFAULT 'node',
                updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_maze_preferences_updated ON maze_preferences(updated_at);
        "#)?;

        Ok(())
    }

    /// Encrypt data using AES-256-GCM
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let key = Key::<Aes256Gcm>::from_slice(&self.encryption_key);
        let cipher = Aes256Gcm::new(key);
        
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = Nonce::from_slice(&nonce_bytes);
        
        let ciphertext = cipher.encrypt(nonce, plaintext)
            .map_err(|e| MazeError::EncryptionError(e.to_string()))?;
        
        // Prepend nonce to ciphertext
        let mut result = Vec::with_capacity(12 + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);
        
        Ok(result)
    }

    /// Decrypt data using AES-256-GCM
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        if ciphertext.len() < 12 {
            return Err(MazeError::DecryptionError("Ciphertext too short".into()));
        }

        let key = Key::<Aes256Gcm>::from_slice(&self.encryption_key);
        let cipher = Aes256Gcm::new(key);
        
        let nonce = Nonce::from_slice(&ciphertext[..12]);
        let encrypted = &ciphertext[12..];
        
        cipher.decrypt(nonce, encrypted)
            .map_err(|e| MazeError::DecryptionError(e.to_string()))
    }

    /// Create a new maze request
    pub fn create_maze_request(&self, request: &MazeRequest, maze: &MazeGraph) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Insert main request
        conn.execute(
            r#"INSERT INTO maze_requests 
               (id, receiver_meta, stealth_pubkey, ephemeral_pubkey, deposit_address,
                amount_lamports, fee_lamports, status, maze_graph_json, created_at, expires_at, sender_meta_hash)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
            params![
                request.id,
                request.receiver_meta,
                request.stealth_pubkey,
                request.ephemeral_pubkey,
                request.deposit_address,
                request.amount_lamports,
                request.fee_lamports,
                request.status.as_str(),
                request.maze_graph_json,
                request.created_at,
                request.expires_at,
                request.sender_meta_hash,
            ],
        )?;

        // Insert maze nodes
        for node in &maze.nodes {
            let tx_out_json = serde_json::to_string(&node.tx_out_signatures)
                .unwrap_or_else(|_| "[]".to_string());

            conn.execute(
                r#"INSERT INTO maze_nodes
                   (request_id, node_index, level, address, keypair_encrypted,
                    amount_in, amount_out, status, tx_out_signatures)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
                params![
                    request.id,
                    node.index,
                    node.level,
                    node.address,
                    node.keypair_encrypted,
                    node.amount_in,
                    node.amount_out,
                    node.status,
                    tx_out_json,
                ],
            )?;
        }

        Ok(())
    }

    /// Get maze request by ID
    pub fn get_maze_request(&self, id: &str) -> Result<Option<MazeRequest>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            r#"SELECT id, receiver_meta, stealth_pubkey, ephemeral_pubkey, deposit_address,
                      amount_lamports, fee_lamports, status, maze_graph_json, created_at,
                      expires_at, completed_at, final_tx_signature, error_message, sender_meta_hash
               FROM maze_requests WHERE id = ?1"#
        )?;

        let result = stmt.query_row(params![id], |row| {
            Ok(MazeRequest {
                id: row.get(0)?,
                receiver_meta: row.get(1)?,
                stealth_pubkey: row.get(2)?,
                ephemeral_pubkey: row.get(3)?,
                deposit_address: row.get(4)?,
                amount_lamports: row.get(5)?,
                fee_lamports: row.get(6)?,
                status: RequestStatus::from_str(&row.get::<_, String>(7)?),
                maze_graph_json: row.get(8)?,
                created_at: row.get(9)?,
                expires_at: row.get(10)?,
                completed_at: row.get(11)?,
                final_tx_signature: row.get(12)?,
                error_message: row.get(13)?,
                sender_meta_hash: row.get(14)?,
            })
        });

        match result {
            Ok(req) => Ok(Some(req)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get maze request by deposit address
    pub fn get_request_by_deposit(&self, deposit_address: &str) -> Result<Option<MazeRequest>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            r#"SELECT id, receiver_meta, stealth_pubkey, ephemeral_pubkey, deposit_address,
                      amount_lamports, fee_lamports, status, maze_graph_json, created_at,
                      expires_at, completed_at, final_tx_signature, error_message, sender_meta_hash
               FROM maze_requests WHERE deposit_address = ?1"#
        )?;

        let result = stmt.query_row(params![deposit_address], |row| {
            Ok(MazeRequest {
                id: row.get(0)?,
                receiver_meta: row.get(1)?,
                stealth_pubkey: row.get(2)?,
                ephemeral_pubkey: row.get(3)?,
                deposit_address: row.get(4)?,
                amount_lamports: row.get(5)?,
                fee_lamports: row.get(6)?,
                status: RequestStatus::from_str(&row.get::<_, String>(7)?),
                maze_graph_json: row.get(8)?,
                created_at: row.get(9)?,
                expires_at: row.get(10)?,
                completed_at: row.get(11)?,
                final_tx_signature: row.get(12)?,
                error_message: row.get(13)?,
                sender_meta_hash: row.get(14)?,
            })
        });

        match result {
            Ok(req) => Ok(Some(req)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Update request status
    pub fn update_request_status(&self, id: &str, status: RequestStatus) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        
        conn.execute(
            "UPDATE maze_requests SET status = ?1 WHERE id = ?2",
            params![status.as_str(), id],
        )?;

        Ok(())
    }

    /// Update request with completion info
    pub fn complete_request(&self, id: &str, tx_signature: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            r#"UPDATE maze_requests 
               SET status = 'completed', completed_at = ?1, final_tx_signature = ?2
               WHERE id = ?3"#,
            params![now, tx_signature, id],
        )?;

        Ok(())
    }

    /// Update request with error
    pub fn fail_request(&self, id: &str, error: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            r#"UPDATE maze_requests 
               SET status = 'failed', error_message = ?1
               WHERE id = ?2"#,
            params![error, id],
        )?;

        Ok(())
    }

    /// Get maze node by address
    pub fn get_node_by_address(&self, address: &str) -> Result<Option<(String, MazeNode)>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            r#"SELECT request_id, node_index, level, address, keypair_encrypted,
                      amount_in, amount_out, status, tx_in_signature, tx_out_signatures
               FROM maze_nodes WHERE address = ?1"#
        )?;

        let result = stmt.query_row(params![address], |row| {
            let tx_out_json: String = row.get(9)?;
            let tx_out_signatures: Vec<Option<String>> = 
                serde_json::from_str(&tx_out_json).unwrap_or_default();

            Ok((
                row.get::<_, String>(0)?,
                MazeNode {
                    index: row.get::<_, i64>(1)? as u16,
                    level: row.get::<_, i64>(2)? as u8,
                    address: row.get(3)?,
                    keypair_encrypted: row.get(4)?,
                    inputs: vec![], // Not stored individually
                    outputs: vec![], // Not stored individually
                    amount_in: row.get::<_, i64>(5)? as u64,
                    amount_out: row.get::<_, i64>(6)? as u64,
                    tx_in_signature: row.get(8)?,
                    tx_out_signatures,
                    status: row.get(7)?,
                },
            ))
        });

        match result {
            Ok(data) => Ok(Some(data)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Update node status and TX signature
    pub fn update_node_status(
        &self,
        request_id: &str,
        node_index: u16,
        status: &str,
        tx_in_sig: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            r#"UPDATE maze_nodes 
               SET status = ?1, tx_in_signature = ?2
               WHERE request_id = ?3 AND node_index = ?4"#,
            params![status, tx_in_sig, request_id, node_index],
        )?;

        Ok(())
    }

    /// Get all nodes for a request
    pub fn get_request_nodes(&self, request_id: &str) -> Result<Vec<MazeNode>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            r#"SELECT node_index, level, address, keypair_encrypted,
                      amount_in, amount_out, status, tx_in_signature, tx_out_signatures
               FROM maze_nodes WHERE request_id = ?1 ORDER BY level, node_index"#
        )?;

        let nodes = stmt.query_map(params![request_id], |row| {
            let tx_out_json: String = row.get(8)?;
            let tx_out_signatures: Vec<Option<String>> = 
                serde_json::from_str(&tx_out_json).unwrap_or_default();

            Ok(MazeNode {
                index: row.get::<_, i64>(0)? as u16,
                level: row.get::<_, i64>(1)? as u8,
                address: row.get(2)?,
                keypair_encrypted: row.get(3)?,
                inputs: vec![],
                outputs: vec![],
                amount_in: row.get::<_, i64>(4)? as u64,
                amount_out: row.get::<_, i64>(5)? as u64,
                tx_in_signature: row.get(7)?,
                tx_out_signatures,
                status: row.get(6)?,
            })
        })?;

        nodes.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| MazeError::DatabaseError(e.to_string()))
    }

    /// Get pending requests (for processing)
    pub fn get_pending_requests(&self) -> Result<Vec<MazeRequest>> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();

        let mut stmt = conn.prepare(
            r#"SELECT id, receiver_meta, stealth_pubkey, ephemeral_pubkey, deposit_address,
                      amount_lamports, fee_lamports, status, maze_graph_json, created_at,
                      expires_at, completed_at, final_tx_signature, error_message, sender_meta_hash
               FROM maze_requests 
               WHERE status IN ('pending', 'deposit_received', 'processing')
               AND expires_at > ?1"#
        )?;

        let requests = stmt.query_map(params![now], |row| {
            Ok(MazeRequest {
                id: row.get(0)?,
                receiver_meta: row.get(1)?,
                stealth_pubkey: row.get(2)?,
                ephemeral_pubkey: row.get(3)?,
                deposit_address: row.get(4)?,
                amount_lamports: row.get(5)?,
                fee_lamports: row.get(6)?,
                status: RequestStatus::from_str(&row.get::<_, String>(7)?),
                maze_graph_json: row.get(8)?,
                created_at: row.get(9)?,
                expires_at: row.get(10)?,
                completed_at: row.get(11)?,
                final_tx_signature: row.get(12)?,
                error_message: row.get(13)?,
                sender_meta_hash: row.get(14)?,
            })
        })?;

        requests.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| MazeError::DatabaseError(e.to_string()))
    }

    /// Record completed transfer for receiver scanning
    pub fn record_completed_transfer(
        &self,
        receiver_meta: &str,
        stealth_pubkey: &str,
        ephemeral_pubkey: &str,
        amount_lamports: u64,
        tx_signature: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            r#"INSERT INTO completed_transfers
               (receiver_meta, stealth_pubkey, ephemeral_pubkey, amount_lamports, tx_signature, completed_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            params![receiver_meta, stealth_pubkey, ephemeral_pubkey, amount_lamports, tx_signature, now],
        )?;

        Ok(())
    }

    /// Scan for transfers to a receiver
    pub fn scan_transfers(&self, receiver_meta: &str) -> Result<Vec<(String, String, u64, String, i64)>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            r#"SELECT stealth_pubkey, ephemeral_pubkey, amount_lamports, tx_signature, completed_at
               FROM completed_transfers WHERE receiver_meta = ?1 ORDER BY completed_at DESC"#
        )?;

        let transfers = stmt.query_map(params![receiver_meta], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;

        transfers.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| MazeError::DatabaseError(e.to_string()))
    }

    /// Autopurge expired requests
    pub fn autopurge(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let cutoff = chrono::Utc::now().timestamp() - AUTOPURGE_SECONDS;

        // Delete old nodes first (foreign key)
        conn.execute(
            r#"DELETE FROM maze_nodes WHERE request_id IN 
               (SELECT id FROM maze_requests WHERE created_at < ?1 AND status IN ('completed', 'expired', 'recovered'))"#,
            params![cutoff],
        )?;

        // Delete old requests
        let deleted = conn.execute(
            r#"DELETE FROM maze_requests 
               WHERE created_at < ?1 AND status IN ('completed', 'expired', 'recovered')"#,
            params![cutoff],
        )?;

        // Delete old completed transfers
        conn.execute(
            "DELETE FROM completed_transfers WHERE completed_at < ?1",
            params![cutoff],
        )?;

        Ok(deleted)
    }

    /// Check subscription status
    pub fn check_subscription(&self, wallet: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();

        let mut stmt = conn.prepare(
            "SELECT 1 FROM subscriptions WHERE wallet_address = ?1 AND expires_at > ?2"
        )?;

        Ok(stmt.exists(params![wallet, now])?)
    }

    /// Add subscription
    pub fn add_subscription(
        &self,
        wallet: &str,
        sub_type: &str,
        duration_days: i64,
        tx_sig: &str,
        amount: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        let expires = now + (duration_days * 86400);

        conn.execute(
            r#"INSERT OR REPLACE INTO subscriptions
               (wallet_address, subscription_type, started_at, expires_at, tx_signature, amount_paid)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            params![wallet, sub_type, now, expires, tx_sig, amount],
        )?;

        Ok(())
    }

    // ============ SHARED DATABASE METHODS (aliases, wallets) ============

    /// Resolve alias to meta address
    pub fn resolve_alias(&self, alias: &str) -> Result<Option<String>> {
        let conn = self.shared_conn.lock().unwrap();
        
        let mut stmt = conn.prepare(
            "SELECT meta_address FROM aliases WHERE alias = ?1 AND is_active = 1"
        )?;
        
        let result = stmt.query_row(params![alias], |row| row.get::<_, String>(0));
        
        match result {
            Ok(meta) => Ok(Some(meta)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Check if alias is available
    pub fn check_alias_available(&self, alias: &str) -> Result<bool> {
        let conn = self.shared_conn.lock().unwrap();
        
        let mut stmt = conn.prepare(
            "SELECT 1 FROM aliases WHERE alias = ?1"
        )?;
        
        Ok(!stmt.exists(params![alias])?)
    }

    /// Register new alias
    pub fn register_alias(&self, alias: &str, meta_address: &str, owner_meta_hash: &str) -> Result<()> {
        let conn = self.shared_conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        
        conn.execute(
            r#"INSERT INTO aliases (alias, meta_address, owner_meta_hash, created_at, is_active)
               VALUES (?1, ?2, ?3, ?4, 1)"#,
            params![alias, meta_address, owner_meta_hash, now],
        )?;
        
        Ok(())
    }

    /// List aliases for owner
    pub fn list_aliases(&self, owner_meta_hash: &str) -> Result<Vec<(String, String)>> {
        let conn = self.shared_conn.lock().unwrap();
        
        let mut stmt = conn.prepare(
            "SELECT alias, meta_address FROM aliases WHERE owner_meta_hash = ?1 AND is_active = 1"
        )?;
        
        let aliases = stmt.query_map(params![owner_meta_hash], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        
        aliases.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| MazeError::DatabaseError(e.to_string()))
    }

    /// Add destination wallet
    pub fn add_destination_wallet(&self, owner_meta_hash: &str, slot: i32, wallet_address: &str) -> Result<()> {
        let conn = self.shared_conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        
        conn.execute(
            r#"INSERT OR REPLACE INTO destination_wallets (owner_meta_hash, slot, wallet_address, created_at)
               VALUES (?1, ?2, ?3, ?4)"#,
            params![owner_meta_hash, slot, wallet_address, now],
        )?;
        
        Ok(())
    }

    /// Delete destination wallet
    pub fn delete_destination_wallet(&self, owner_meta_hash: &str, slot: i32) -> Result<()> {
        let conn = self.shared_conn.lock().unwrap();
        
        conn.execute(
            "DELETE FROM destination_wallets WHERE owner_meta_hash = ?1 AND slot = ?2",
            params![owner_meta_hash, slot],
        )?;
        
        Ok(())
    }

    /// List destination wallets
    pub fn list_destination_wallets(&self, owner_meta_hash: &str) -> Result<Vec<(i32, String)>> {
        let conn = self.shared_conn.lock().unwrap();
        
        let mut stmt = conn.prepare(
            "SELECT slot, wallet_address FROM destination_wallets WHERE owner_meta_hash = ?1 ORDER BY slot"
        )?;
        
        let wallets = stmt.query_map(params![owner_meta_hash], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))

        })?;
        
        wallets.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| MazeError::DatabaseError(e.to_string()))
    }

    /// Get destination wallet by slot
    pub fn get_destination_wallet(&self, owner_meta_hash: &str, slot: i32) -> Result<Option<String>> {
        let conn = self.shared_conn.lock().unwrap();
        
        let mut stmt = conn.prepare(
            "SELECT wallet_address FROM destination_wallets WHERE owner_meta_hash = ?1 AND slot = ?2"
        )?;
        
        let result = stmt.query_row(params![owner_meta_hash, slot], |row| row.get::<_, String>(0));
        
        match result {
            Ok(addr) => Ok(Some(addr)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Check subscription from shared db
    pub fn check_shared_subscription(&self, meta_hash: &str) -> Result<bool> {
        let conn = self.shared_conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        
        let mut stmt = conn.prepare(
            "SELECT 1 FROM subscriptions WHERE meta_address_hash = ?1 AND expires_at > ?2 AND is_active = 1"
        )?;
        
        Ok(stmt.exists(params![meta_hash, now])?)
    }

    // ============ DIVERSIFY FUNCTIONS ============

    /// Create diversify parent request
    pub fn create_diversify_request(
        &self,
        id: &str,
        meta_address: &str,
        deposit_address: &str,
        deposit_keypair_encrypted: &[u8],
        total_amount: u64,
        fee_amount: u64,
        route_count: usize,
        distribution_mode: &str,
        expires_in_secs: i64,
        maze_config_json: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        let expires_at = now + expires_in_secs;

        conn.execute(
            r#"INSERT INTO diversify_requests (
                id, meta_address, deposit_address, deposit_keypair_encrypted,
                total_amount, fee_amount, route_count, distribution_mode,
                status, created_at, expires_at, maze_config_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10, ?11)"#,
            params![
                id,
                meta_address,
                deposit_address,
                deposit_keypair_encrypted,
                total_amount as i64,
                fee_amount as i64,
                route_count as i64,
                distribution_mode,
                now,
                expires_at,
                maze_config_json,
            ],
        )?;

        Ok(())
    }

    /// Add route to diversify request
    pub fn add_diversify_route(
        &self,
        parent_id: &str,
        route_index: usize,
        destination_slot: i32,
        destination_wallet: &str,
        amount: u64,
        percentage: Option<f64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            r#"INSERT INTO diversify_routes (
                parent_id, route_index, destination_slot, destination_wallet,
                amount, percentage, status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending')"#,
            params![
                parent_id,
                route_index as i64,
                destination_slot,
                destination_wallet,
                amount as i64,
                percentage,
            ],
        )?;

        Ok(())
    }

    /// Update route with child maze request ID
    pub fn link_route_to_maze(&self, parent_id: &str, route_index: usize, child_request_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            "UPDATE diversify_routes SET child_request_id = ?1 WHERE parent_id = ?2 AND route_index = ?3",
            params![child_request_id, parent_id, route_index as i64],
        )?;

        Ok(())
    }

    /// Get diversify request by ID
    pub fn get_diversify_request(&self, id: &str) -> Result<Option<(String, String, Vec<u8>, u64, u64, String, i64, String, Option<String>)>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT deposit_address, status, deposit_keypair_encrypted, total_amount, fee_amount, distribution_mode, expires_at, meta_address, maze_config_json 
             FROM diversify_requests WHERE id = ?1"
        )?;

        let result = stmt.query_row(params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)? as u64,
                row.get::<_, i64>(4)? as u64,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        });

        match result {
            Ok(data) => Ok(Some(data)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get diversify routes
    pub fn get_diversify_routes(&self, parent_id: &str) -> Result<Vec<(i64, usize, i32, String, u64, Option<f64>, Option<String>, String)>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id, route_index, destination_slot, destination_wallet, amount, percentage, child_request_id, status 
             FROM diversify_routes WHERE parent_id = ?1 ORDER BY route_index"
        )?;

        let routes = stmt.query_map(params![parent_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)? as usize,
                row.get::<_, i32>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)? as u64,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?;

        routes.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| MazeError::DatabaseError(e.to_string()))
    }

    /// Update diversify request status
    pub fn update_diversify_status(&self, id: &str, status: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            "UPDATE diversify_requests SET status = ?1 WHERE id = ?2",
            params![status, id],
        )?;

        Ok(())
    }

    /// Update diversify route status
    pub fn update_diversify_route_status(&self, route_id: i64, status: &str, error_message: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();

        if status == "completed" {
            conn.execute(
                "UPDATE diversify_routes SET status = ?1, completed_at = ?2 WHERE id = ?3",
                params![status, now, route_id],
            )?;
        } else if let Some(err) = error_message {
            conn.execute(
                "UPDATE diversify_routes SET status = ?1, error_message = ?2 WHERE id = ?3",
                params![status, err, route_id],
            )?;
        } else {
            conn.execute(
                "UPDATE diversify_routes SET status = ?1 WHERE id = ?2",
                params![status, route_id],
            )?;
        }

        Ok(())
    }

    /// Get pending diversify requests
    pub fn get_pending_diversify_requests(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id FROM diversify_requests WHERE status = 'pending' OR status = 'processing'"
        )?;

        let ids = stmt.query_map([], |row| row.get::<_, String>(0))?;

        ids.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| MazeError::DatabaseError(e.to_string()))
    }

    /// Complete diversify request
    pub fn complete_diversify_request(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "UPDATE diversify_requests SET status = 'completed', completed_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;

        Ok(())
    }


    // ============ MAZE PREFERENCES ============

    /// Get maze preferences for user
    pub fn get_maze_preferences(&self, owner_meta_hash: &str) -> Result<Option<MazePreferencesRow>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT hop_count, split_ratio, merge_strategy, delay_pattern, delay_ms, delay_scope, updated_at FROM maze_preferences WHERE owner_meta_hash = ?1"
        )?;

        let result = stmt.query_row(params![owner_meta_hash], |row| {
            Ok(MazePreferencesRow {
                owner_meta_hash: owner_meta_hash.to_string(),
                hop_count: row.get(0)?,
                split_ratio: row.get(1)?,
                merge_strategy: row.get(2)?,
                delay_pattern: row.get(3)?,
                delay_ms: row.get(4)?,
                delay_scope: row.get(5)?,
                updated_at: row.get(6)?,
            })
        });

        match result {
            Ok(prefs) => Ok(Some(prefs)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Save maze preferences for user (upsert)
    pub fn save_maze_preferences(&self, prefs: &MazePreferencesRow) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            r#"INSERT INTO maze_preferences (owner_meta_hash, hop_count, split_ratio, merge_strategy, delay_pattern, delay_ms, delay_scope, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
               ON CONFLICT(owner_meta_hash) DO UPDATE SET
                   hop_count = excluded.hop_count,
                   split_ratio = excluded.split_ratio,
                   merge_strategy = excluded.merge_strategy,
                   delay_pattern = excluded.delay_pattern,
                   delay_ms = excluded.delay_ms,
                   delay_scope = excluded.delay_scope,
                   updated_at = excluded.updated_at"#,
            params![
                prefs.owner_meta_hash,
                prefs.hop_count,
                prefs.split_ratio,
                prefs.merge_strategy,
                prefs.delay_pattern,
                prefs.delay_ms,
                prefs.delay_scope,
                prefs.updated_at,
            ],
        )?;

        Ok(())

    }
    /// Check if user has active Pro subscription
    pub fn is_pro_subscriber(&self, meta_address_hash: &str) -> bool {
        let conn = self.shared_conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        let result: std::result::Result<i32, rusqlite::Error> = conn.query_row(
            "SELECT COUNT(*) FROM subscriptions WHERE meta_address_hash = ?1 AND is_active = 1 AND expires_at > ?2",
            params![meta_address_hash, now],
            |row| row.get(0)
        );
        match result {
            Ok(count) => count > 0,
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_status() {
        assert_eq!(RequestStatus::Pending.as_str(), "pending");
        assert_eq!(RequestStatus::from_str("completed"), RequestStatus::Completed);
    }

    #[test]
    fn test_encryption_roundtrip() {
        std::env::set_var("DB_ENCRYPTION_KEY", "test_key_12345");
        let db = RelayDatabase::new(Some(":memory:")).unwrap();
        
        let plaintext = b"secret keypair data";
        let encrypted = db.encrypt(plaintext).unwrap();
        let decrypted = db.decrypt(&encrypted).unwrap();
        
        assert_eq!(plaintext.to_vec(), decrypted);
    }
}
