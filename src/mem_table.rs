use std::{
    collections::HashMap,
    sync::{Mutex, atomic::AtomicU64},
};

use crate::{entry::Entry, error::Error, table::StreamTable};

pub type MemTableArc = std::sync::Arc<MemTable>;
type GetStreamOffsetHandler = Box<dyn Fn(u64) -> Result<u64, Error> + Send + Send>;
pub struct MemTable {
    stream_tables: Mutex<HashMap<u64, StreamTable>>,
    first_entry: AtomicU64,
    last_entry: AtomicU64,
    size: AtomicU64,
    get_stream_offset: Mutex<Option<GetStreamOffsetHandler>>,
}

impl MemTable {
    pub fn new() -> Self {
        MemTable {
            stream_tables: Mutex::new(HashMap::new()),
            first_entry: AtomicU64::new(0),
            last_entry: AtomicU64::new(0),
            size: AtomicU64::new(0),
            get_stream_offset: Mutex::new(None),
        }
    }

    pub fn get_first_entry(&self) -> u64 {
        self.first_entry.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn get_last_entry(&self) -> u64 {
        self.last_entry.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn get_size(&self) -> u64 {
        self.size.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn get_stream_tables(&self) -> std::sync::MutexGuard<HashMap<u64, StreamTable>> {
        self.stream_tables.lock().unwrap()
    }

    pub fn get_stream_range(&self, stream_id: u64) -> Option<(u64, u64)> {
        let guard = self.stream_tables.lock().unwrap();
        if let Some(stream_table) = guard.get(&stream_id) {
            return stream_table.get_stream_range();
        }
        None
    }

    pub fn read_stream_data(
        &self,
        stream_id: u64,
        offset: u64,
        size: u64,
    ) -> Result<Vec<u8>, Error> {
        let guard = self.stream_tables.lock().unwrap();
        if let Some(stream_table) = guard.get(&stream_id) {
            return stream_table.read_stream_data(offset, size);
        }
        Err(Error::StreamNotFound)
    }

    pub fn append(&self, entry: &Entry) -> Result<(), Error> {
        let data_len = entry.data.len() as u64;

        let mut guard = self.stream_tables.lock().unwrap();

        let res = guard.get_mut(&entry.stream_id);
        if res.is_none() {
            let offset = match self.get_stream_offset.lock().unwrap().as_ref() {
                Some(ref handler) => handler(entry.stream_id)?,
                None => 0,
            };
            guard
                .insert(entry.stream_id, StreamTable::new(entry.stream_id, offset))
                .unwrap();
        } else {
            // Append the data to the stream table
            res.unwrap().append(&entry.data)?;
        }

        // Update the stream table
        self.size
            .fetch_add(data_len, std::sync::atomic::Ordering::SeqCst);

        self.last_entry
            .store(entry.id, std::sync::atomic::Ordering::SeqCst);

        if self.first_entry.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            self.first_entry
                .store(entry.id, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
}

fn assert_send_sync<T: Send + Sync>() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_data() {
        assert_send_sync::<MemTable>();
    }
}
