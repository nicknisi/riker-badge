//! Fixed-memory PCM queue. A full queue stops capture; it never overwrites old audio.
use std::collections::VecDeque;
use zeroize::Zeroize;
pub const RATE: usize = 16_000;
pub const CHUNK: usize = RATE * 10;
pub const SLOTS: usize = 4;
pub const MAX_SAMPLES: u64 = RATE as u64 * 8 * 3600;
pub struct AudioChunk {
    pub seq: u32,
    pub pcm: Vec<i16>,
}
impl Drop for AudioChunk {
    fn drop(&mut self) {
        self.pcm.zeroize();
    }
}
pub struct AudioBuffer {
    free: Vec<Vec<i16>>,
    ready: VecDeque<AudioChunk>,
    building: Option<Vec<i16>>,
    next_seq: u32,
    total: u64,
    overflow: bool,
    limit: bool,
}
impl AudioBuffer {
    pub fn new() -> Result<Self, std::collections::TryReserveError> {
        let mut free = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            let mut v = Vec::new();
            v.try_reserve_exact(CHUNK)?;
            free.push(v);
        }
        Ok(Self {
            free,
            ready: VecDeque::with_capacity(SLOTS),
            building: None,
            next_seq: 0,
            total: 0,
            overflow: false,
            limit: false,
        })
    }
    pub fn push(&mut self, data: &[i16]) -> bool {
        if self.overflow || self.limit {
            return false;
        }
        if self.total + data.len() as u64 > MAX_SAMPLES {
            self.limit = true;
            return false;
        }
        if self.building.is_none() {
            let Some(buffer) = self.free.pop() else {
                self.overflow = true;
                return false;
            };
            self.building = Some(buffer);
        }
        let buffer = self.building.as_mut().unwrap();
        // Production frames are 320 samples and divide CHUNK exactly.
        if data.len() > CHUNK - buffer.len() {
            self.overflow = true;
            return false;
        }
        buffer.extend_from_slice(data);
        self.total += data.len() as u64;
        if buffer.len() == CHUNK {
            self.flush();
        }
        true
    }
    pub fn flush(&mut self) {
        if let Some(buffer) = self.building.take() {
            if buffer.is_empty() {
                self.free.push(buffer);
            } else {
                self.ready.push_back(AudioChunk {
                    seq: self.next_seq,
                    pcm: buffer,
                });
                self.next_seq += 1;
            }
        }
    }
    pub fn take(&mut self) -> Option<AudioChunk> {
        self.ready.pop_front()
    }
    pub fn recycle(&mut self, mut chunk: AudioChunk) {
        chunk.pcm.zeroize();
        self.free.push(std::mem::take(&mut chunk.pcm));
    }
    pub fn total(&self) -> u64 {
        self.total
    }
    pub fn chunks(&self) -> u32 {
        self.next_seq
    }
    pub fn interrupted(&self) -> bool {
        self.overflow || self.limit
    }
    pub fn buffered_samples(&self) -> usize {
        self.ready.iter().map(|c| c.pcm.len()).sum::<usize>()
            + self.building.as_ref().map_or(0, Vec::len)
    }
}
impl Drop for AudioBuffer {
    fn drop(&mut self) {
        for b in &mut self.free {
            b.zeroize();
        }
        for c in &mut self.ready {
            c.pcm.zeroize();
        }
        if let Some(b) = self.building.as_mut() {
            b.zeroize();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_and_lossless_until_full() {
        let mut b = AudioBuffer::new().unwrap();
        let frame = [42; 320];
        for _ in 0..SLOTS * CHUNK / 320 {
            assert!(b.push(&frame));
        }
        assert!(!b.push(&frame));
        assert!(b.interrupted());
        assert_eq!(b.total(), (SLOTS * CHUNK) as u64);
        for seq in 0..SLOTS {
            let c = b.take().unwrap();
            assert_eq!(c.seq, seq as u32);
            assert_eq!(c.pcm, vec![42; CHUNK]);
        }
        assert!(b.take().is_none());
    }
    #[test]
    fn four_hours_of_producer_recycling() {
        let mut b = AudioBuffer::new().unwrap();
        let frame = [17; 320];
        let mut received = 0u64;
        let mut seq = 0;
        for _ in 0..RATE * 4 * 3600 / 320 {
            assert!(b.push(&frame));
            if let Some(c) = b.take() {
                assert_eq!(c.seq, seq);
                seq += 1;
                received += c.pcm.len() as u64;
                b.recycle(c);
            }
        }
        b.flush();
        assert_eq!(received, RATE as u64 * 4 * 3600);
        assert_eq!(b.total(), received);
        assert!(!b.interrupted());
    }
    #[test]
    fn partial_final_chunk_keeps_its_identity() {
        let mut b = AudioBuffer::new().unwrap();
        assert!(b.push(&[3; 320]));
        b.flush();
        let c = b.take().unwrap();
        assert_eq!(c.seq, 0);
        assert_eq!(c.pcm.len(), 320);
        assert_eq!(b.chunks(), 1);
        b.recycle(c);
    }
}
