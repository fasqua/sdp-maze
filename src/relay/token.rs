//! Token operations for SDP Maze
//!
//! Handles SPL token transfers and Jupiter swaps

use solana_sdk::pubkey::Pubkey;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::config::{USDC_MINT, KAUSA_MINT, USDC_DECIMALS, KAUSA_DECIMALS};

/// Token info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub mint: String,
    pub symbol: String,
    pub decimals: u8,
    pub name: String,
}

impl TokenInfo {
    pub fn usdc() -> Self {
        Self {
            mint: USDC_MINT.to_string(),
            symbol: "USDC".to_string(),
            decimals: USDC_DECIMALS,
            name: "USD Coin".to_string(),
        }
    }

    pub fn kausa() -> Self {
        Self {
            mint: KAUSA_MINT.to_string(),
            symbol: "KAUSA".to_string(),
            decimals: KAUSA_DECIMALS,
            name: "KausaLayer".to_string(),
        }
    }

    pub fn sol() -> Self {
        Self {
            mint: "So11111111111111111111111111111111111111112".to_string(),
            symbol: "SOL".to_string(),
            decimals: 9,
            name: "Solana".to_string(),
        }
    }
}

/// Get Associated Token Account address
pub fn get_ata_address(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    let seeds = &[
        owner.as_ref(),
        &spl_token::id().to_bytes(),
        mint.as_ref(),
    ];
    let (ata, _bump) = Pubkey::find_program_address(
        seeds,
        &spl_associated_token_account::id(),
    );
    ata
}

/// Parse mint string to Pubkey
pub fn parse_mint(mint_str: &str) -> Option<Pubkey> {
    Pubkey::from_str(mint_str).ok()
}

/// Format token amount with decimals
pub fn format_token_amount(amount: u64, decimals: u8) -> String {
    let divisor = 10u64.pow(decimals as u32);
    let whole = amount / divisor;
    let frac = amount % divisor;
    
    if frac == 0 {
        format!("{}", whole)
    } else {
        let frac_str = format!("{:0width$}", frac, width = decimals as usize);
        let trimmed = frac_str.trim_end_matches('0');
        format!("{}.{}", whole, trimmed)
    }
}

/// Parse token amount string to raw amount
pub fn parse_token_amount(amount_str: &str, decimals: u8) -> Option<u64> {
    let parts: Vec<&str> = amount_str.split('.').collect();
    let multiplier = 10u64.pow(decimals as u32);
    
    match parts.len() {
        1 => {
            let whole: u64 = parts[0].parse().ok()?;
            Some(whole * multiplier)
        }
        2 => {
            let whole: u64 = parts[0].parse().ok()?;
            let frac_str = parts[1];
            let frac_len = frac_str.len().min(decimals as usize);
            let frac_padded = format!("{:0<width$}", &frac_str[..frac_len], width = decimals as usize);
            let frac: u64 = frac_padded.parse().ok()?;
            Some(whole * multiplier + frac)
        }
        _ => None,
    }
}

/// Jupiter swap quote request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapQuoteRequest {
    pub input_mint: String,
    pub output_mint: String,
    pub amount: u64,
    pub slippage_bps: u16,
}

/// Jupiter swap quote response (simplified)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapQuote {
    pub input_mint: String,
    pub output_mint: String,
    pub in_amount: String,
    pub out_amount: String,
    pub price_impact_pct: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_token_amount() {
        assert_eq!(format_token_amount(1_000_000, 6), "1");
        assert_eq!(format_token_amount(1_500_000, 6), "1.5");
        assert_eq!(format_token_amount(1_234_567, 6), "1.234567");
        assert_eq!(format_token_amount(1_000_000_000, 9), "1");
    }

    #[test]
    fn test_parse_token_amount() {
        assert_eq!(parse_token_amount("1", 6), Some(1_000_000));
        assert_eq!(parse_token_amount("1.5", 6), Some(1_500_000));
        assert_eq!(parse_token_amount("1.234567", 6), Some(1_234_567));
        assert_eq!(parse_token_amount("1", 9), Some(1_000_000_000));
    }

    #[test]
    fn test_token_info() {
        let usdc = TokenInfo::usdc();
        assert_eq!(usdc.symbol, "USDC");
        assert_eq!(usdc.decimals, 6);

        let sol = TokenInfo::sol();
        assert_eq!(sol.symbol, "SOL");
        assert_eq!(sol.decimals, 9);
    }
}
