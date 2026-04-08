//! Integrity verification primitives.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use sha2::{Digest, Sha256};

const HASH_BUFFER_SIZE: usize = 256 * 1024;

/// Fixed-size SHA-256 digest length in bytes.
pub const SHA256_LEN: usize = 32;

/// Raw SHA-256 digest bytes.
pub type Sha256Hash = [u8; SHA256_LEN];

/// Summary of a checksum operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegrityReport {
    /// Number of bytes included in the checksum.
    pub bytes_hashed: u64,
    /// Rolling checksum value.
    pub rolling_checksum: u64,
}

/// Incremental SHA-256 state for streaming verification.
#[derive(Debug, Default)]
pub struct Sha256State {
    hasher: Sha256,
    bytes_hashed: u64,
}

impl Sha256State {
    /// Creates a fresh SHA-256 state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds additional bytes to the digest state.
    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
        self.bytes_hashed = self.bytes_hashed.saturating_add(data.len() as u64);
    }

    /// Finalizes the digest and returns the SHA-256 hash bytes.
    pub fn finalize(self) -> Sha256Hash {
        self.hasher.finalize().into()
    }

    /// Returns the total number of hashed bytes.
    pub fn bytes_hashed(&self) -> u64 {
        self.bytes_hashed
    }
}

/// Computes a lightweight checksum for a byte slice.
pub fn checksum(data: &[u8]) -> IntegrityReport {
    let mut state = 0xcbf2_9ce4_8422_2325_u64;

    for &byte in data {
        state ^= u64::from(byte);
        state = state.wrapping_mul(0x0000_0001_0000_01b3);
    }

    IntegrityReport {
        bytes_hashed: data.len() as u64,
        rolling_checksum: state,
    }
}

/// Compares expected and computed checksums.
pub fn verify_checksum(data: &[u8], expected: u64) -> bool {
    checksum(data).rolling_checksum == expected
}

/// Computes a SHA-256 digest for an in-memory byte slice.
pub fn sha256_bytes(data: &[u8]) -> Sha256Hash {
    let mut state = Sha256State::new();
    state.update(data);
    state.finalize()
}

/// Computes a SHA-256 digest for a reader without loading all data into memory.
pub fn sha256_reader<R>(reader: &mut R) -> io::Result<Sha256Hash>
where
    R: Read,
{
    let mut state = Sha256State::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        state.update(&buffer[..read]);
    }

    Ok(state.finalize())
}

/// Computes a SHA-256 digest for a file path.
pub fn sha256_file(path: &Path) -> io::Result<Sha256Hash> {
    let mut file = File::open(path)?;
    sha256_reader(&mut file)
}

/// Formats a SHA-256 hash as lowercase hexadecimal.
pub fn format_sha256(hash: &Sha256Hash) -> String {
    let mut hex = String::with_capacity(SHA256_LEN * 2);
    for byte in hash {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

/// Compares two SHA-256 digests for equality.
pub fn verify_sha256(actual: &Sha256Hash, expected: &Sha256Hash) -> bool {
    actual == expected
}

#[cfg(test)]
mod tests {
    use super::{format_sha256, sha256_bytes, verify_sha256};

    #[test]
    fn sha256_of_known_message_matches_expected_hex() {
        let digest = sha256_bytes(b"fasttransfer");
        assert_eq!(format_sha256(&digest), "9c85fef864e53117e3cfd1c8d6c74d6515ff3b7a6f20b95007ae5f84f266c8f8");
        assert!(verify_sha256(&digest, &digest));
    }
}
