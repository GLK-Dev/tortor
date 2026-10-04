use anyhow::Result;
use async_trait::async_trait;

#[async_trait(?Send)]
pub trait AsyncDiskIO {
    async fn write_piece(&mut self, piece_index: u32, data: Vec<u8>) -> Result<()>;
    async fn read_piece(&mut self, piece_index: u32, offset: u32, len: u32) -> Result<Vec<u8>>;

    /// Forces data written so far to stable storage. Writes are not synced
    /// one by one; callers flush before recording progress as durable.
    async fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
