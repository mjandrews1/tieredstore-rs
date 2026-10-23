//! Write-Ahead Log with binary format, checksums, and batched fsync.
//!
//! Every store mutation is appended to the WAL before the cache is updated.
//! On crash, WAL replay recovers all committed writes.
//!
//! Entry format (binary):
//!   magic(4B) | seq(8B) | op(1B) | keyLen(2B) | valLen(4B) | key | val | crc(4B)

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

const MAGIC: u32 = 0x5741_4C44; // "WALD"
const HEADER_SIZE: usize = 4 + 8 + 1 + 2 + 4; // magic+seq+op+keyLen+valLen
const CRC_SIZE: usize = 4;

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Set { key: String, value: String },
    Kill { key: String },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub seq: u64,
    pub op: Op,
}

pub struct WriteAheadLog {
    path: String,
    writer: Mutex<BufWriter<File>>,
    next_seq: u64,
    last_fsync: std::time::Instant,
    fsync_interval: std::time::Duration,
}

impl WriteAheadLog {
    pub fn new(path: &str, fsync_interval_ms: u64) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        // Scan existing entries to find next sequence
        let next_seq = Self::scan_next_seq(path)? + 1;

        Ok(Self {
            path: path.to_string(),
            writer: Mutex::new(BufWriter::new(file)),
            next_seq,
            last_fsync: std::time::Instant::now(),
            fsync_interval: std::time::Duration::from_millis(fsync_interval_ms),
        })
    }

    /// Append a SET entry. Returns sequence number.
    pub fn append_set(&mut self, key: &str, value: &str) -> std::io::Result<u64> {
        let op = Op::Set {
            key: key.to_string(),
            value: value.to_string(),
        };
        self.append(&op)
    }

    /// Append a KILL entry. Returns sequence number.
    pub fn append_kill(&mut self, key: &str) -> std::io::Result<u64> {
        let op = Op::Kill {
            key: key.to_string(),
        };
        self.append(&op)
    }

    fn append(&mut self, op: &Op) -> std::io::Result<u64> {
        let (key, value, op_byte) = match op {
            Op::Set { key, value } => (key.as_bytes(), value.as_bytes(), 1u8),
            Op::Kill { key } => (key.as_bytes(), &b""[..], 2u8),
        };

        let seq = self.next_seq;
        self.next_seq += 1;

        // Build entry
        let mut buf = Vec::with_capacity(HEADER_SIZE + key.len() + value.len() + CRC_SIZE);
        buf.extend_from_slice(&MAGIC.to_be_bytes());
        buf.extend_from_slice(&seq.to_be_bytes());
        buf.push(op_byte);
        buf.extend_from_slice(&(key.len() as u16).to_be_bytes());
        buf.extend_from_slice(&(value.len() as u32).to_be_bytes());
        buf.extend_from_slice(key);
        buf.extend_from_slice(value);

        // CRC over everything except the CRC field itself
        let crc = crc32fast::hash(&buf);
        buf.extend_from_slice(&crc.to_be_bytes());

        {
            let mut writer = self.writer.lock().unwrap();
            writer.write_all(&buf)?;
        }

        // Batched fsync
        self.maybe_fsync()?;
        Ok(seq)
    }

    fn maybe_fsync(&mut self) -> std::io::Result<()> {
        if self.last_fsync.elapsed() >= self.fsync_interval {
            let mut writer = self.writer.lock().unwrap();
            writer.flush()?;
            writer.get_ref().sync_all()?;
            self.last_fsync = std::time::Instant::now();
        }
        Ok(())
    }

    /// Force fsync now (for shutdown).
    pub fn fsync(&mut self) -> std::io::Result<()> {
        let mut writer = self.writer.lock().unwrap();
        writer.flush()?;
        writer.get_ref().sync_all()?;
        self.last_fsync = std::time::Instant::now();
        Ok(())
    }

    /// Replay all valid entries. Stops at first bad CRC (torn write).
    pub fn replay(&self) -> std::io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        let file = File::open(&self.path)?;
        let mut reader = BufReader::new(file);

        loop {
            // Read header
            let mut header = [0u8; HEADER_SIZE];
            match reader.read_exact(&mut header) {
                Ok(_) => {}
                Err(_) => break, // EOF or corruption
            }

            let magic = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
            if magic != MAGIC {
                break; // corruption
            }

            let seq = u64::from_be_bytes([
                header[4], header[5], header[6], header[7],
                header[8], header[9], header[10], header[11],
            ]);
            let op_byte = header[12];
            let key_len = u16::from_be_bytes([header[13], header[14]]) as usize;
            let val_len = u32::from_be_bytes([header[15], header[16], header[17], header[18]]) as usize;

            // Read key + value + crc
            let payload_size = key_len + val_len + CRC_SIZE;
            let mut payload = vec![0u8; payload_size];
            match reader.read_exact(&mut payload) {
                Ok(_) => {}
                Err(_) => break, // truncated
            }

            let key_bytes = &payload[..key_len];
            let val_bytes = &payload[key_len..key_len + val_len];
            let stored_crc = u32::from_be_bytes([
                payload[key_len + val_len],
                payload[key_len + val_len + 1],
                payload[key_len + val_len + 2],
                payload[key_len + val_len + 3],
            ]);

            // Verify CRC
            let mut crc_data = header.to_vec();
            crc_data.extend_from_slice(key_bytes);
            crc_data.extend_from_slice(val_bytes);
            let computed_crc = crc32fast::hash(&crc_data);
            if computed_crc != stored_crc {
                break; // torn write
            }

            let key = String::from_utf8_lossy(key_bytes).to_string();
            let value = String::from_utf8_lossy(val_bytes).to_string();

            let op = match op_byte {
                1 => Op::Set { key, value },
                2 => Op::Kill { key },
                _ => break,
            };

            entries.push(Entry { seq, op });
        }

        Ok(entries)
    }

    /// Truncate WAL (all entries now on disk).
    pub fn truncate(&mut self) -> std::io::Result<()> {
        let mut writer = self.writer.lock().unwrap();
        writer.flush()?;
        drop(writer);

        let file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        drop(file);

        // Reopen for appending
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.writer = Mutex::new(BufWriter::new(file));
        self.fsync()
    }

    fn scan_next_seq(path: &str) -> std::io::Result<u64> {
        if !Path::new(path).exists() {
            return Ok(0);
        }

        let mut max_seq = 0u64;
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);

        loop {
            let mut header = [0u8; HEADER_SIZE];
            match reader.read_exact(&mut header) {
                Ok(_) => {}
                Err(_) => break,
            }

            let magic = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
            if magic != MAGIC {
                break;
            }

            let seq = u64::from_be_bytes([
                header[4], header[5], header[6], header[7],
                header[8], header[9], header[10], header[11],
            ]);
            if seq > max_seq {
                max_seq = seq;
            }

            let key_len = u16::from_be_bytes([header[13], header[14]]) as usize;
            let val_len = u32::from_be_bytes([header[15], header[16], header[17], header[18]]) as usize;
            let skip = key_len + val_len + CRC_SIZE;
            reader.seek(SeekFrom::Current(skip as i64))?;
        }

        Ok(max_seq)
    }
}
