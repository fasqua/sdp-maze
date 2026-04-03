//! Utility functions

/// Lamports per SOL
pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Convert lamports to SOL
pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / LAMPORTS_PER_SOL as f64
}

/// Convert SOL to lamports
pub fn sol_to_lamports(sol: f64) -> u64 {
    (sol * LAMPORTS_PER_SOL as f64) as u64
}

/// Generate deterministic random from seed
pub fn seeded_random(seed: &[u8; 32], index: u64) -> u64 {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(seed);
    hasher.update(&index.to_le_bytes());
    let result = hasher.finalize();
    u64::from_le_bytes(result[0..8].try_into().unwrap())
}

/// Generate Fibonacci number at index (cached)
pub fn fibonacci(n: u8) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        _ => {
            let mut a = 0u64;
            let mut b = 1u64;
            for _ in 2..=n {
                let tmp = a + b;
                a = b;
                b = tmp;
            }
            b
        }
    }
}

/// Apply golden ratio split to amount
pub fn golden_split(amount: u64, ratio: f64) -> (u64, u64) {
    let part1 = (amount as f64 / ratio) as u64;
    let part2 = amount - part1;
    (part1, part2)
}

/// Add noise to amount (for obfuscation)
pub fn add_noise(amount: u64, noise_percent: f64, seed: &[u8; 32], index: u64) -> u64 {
    let random = seeded_random(seed, index);
    let noise_factor = (random % 1000) as f64 / 1000.0; // 0.0 to 0.999
    let noise_range = (amount as f64 * noise_percent / 100.0) as i64;
    let noise = ((noise_factor * 2.0 - 1.0) * noise_range as f64) as i64;
    (amount as i64 + noise).max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lamports_conversion() {
        assert_eq!(lamports_to_sol(1_000_000_000), 1.0);
        assert_eq!(sol_to_lamports(1.0), 1_000_000_000);
        assert_eq!(sol_to_lamports(0.5), 500_000_000);
    }

    #[test]
    fn test_fibonacci() {
        assert_eq!(fibonacci(0), 0);
        assert_eq!(fibonacci(1), 1);
        assert_eq!(fibonacci(2), 1);
        assert_eq!(fibonacci(10), 55);
        assert_eq!(fibonacci(20), 6765);
    }

    #[test]
    fn test_golden_split() {
        let (a, b) = golden_split(1000, 1.618);
        assert_eq!(a + b, 1000);
        assert!(a > 0 && b > 0);
    }

    #[test]
    fn test_seeded_random_deterministic() {
        let seed = [42u8; 32];
        let r1 = seeded_random(&seed, 0);
        let r2 = seeded_random(&seed, 0);
        assert_eq!(r1, r2);
        
        let r3 = seeded_random(&seed, 1);
        assert_ne!(r1, r3);
    }
}
