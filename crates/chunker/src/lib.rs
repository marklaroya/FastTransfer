//! Deterministic chunk planning utilities.

use protocol::ChunkDescriptor;

/// Fixed-size chunking strategy for predictable resume semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedChunker {
    total_bytes: u64,
    chunk_size: u32,
}

impl FixedChunker {
    /// Creates a new chunk planner.
    pub fn new(total_bytes: u64, chunk_size: u32) -> Self {
        Self {
            total_bytes,
            chunk_size: chunk_size.max(1),
        }
    }

    /// Produces chunk descriptors for the entire file.
    pub fn descriptors(&self) -> Vec<ChunkDescriptor> {
        if self.total_bytes == 0 {
            return Vec::new();
        }

        let chunk_size = u64::from(self.chunk_size);
        let chunk_count = self.total_bytes.div_ceil(chunk_size);
        let mut descriptors = Vec::with_capacity(chunk_count as usize);

        for index in 0..chunk_count {
            let offset = index * chunk_size;
            let remaining = self.total_bytes.saturating_sub(offset);
            let size = remaining.min(chunk_size) as u32;

            descriptors.push(ChunkDescriptor { index, offset, size });
        }

        descriptors
    }
}
