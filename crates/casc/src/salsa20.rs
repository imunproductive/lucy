//! Salsa20/20 with a 16-byte key, the cipher of encrypted BLTE blocks.

pub(crate) struct Salsa20 {
    state: [u32; 16],
}

const TAU: &[u8; 16] = b"expand 16-byte k";

impl Salsa20 {
    pub fn new(key: &[u8; 16], iv: &[u8; 8]) -> Salsa20 {
        let word = |bytes: &[u8], at: usize| {
            u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
        };
        let mut state = [0; 16];
        for (slot, at) in [(0, 0), (5, 4), (10, 8), (15, 12)] {
            state[slot] = word(TAU, at);
        }
        for index in 0..4 {
            state[1 + index] = word(key, 4 * index);
            state[11 + index] = word(key, 4 * index);
        }
        state[6] = word(iv, 0);
        state[7] = word(iv, 4);
        Salsa20 { state }
    }

    pub fn apply(&mut self, data: &mut [u8]) {
        for chunk in data.chunks_mut(64) {
            let stream = self.block();
            chunk
                .iter_mut()
                .zip(stream)
                .for_each(|(byte, key)| *byte ^= key);
        }
    }

    fn block(&mut self) -> [u8; 64] {
        let mut x = self.state;
        let quarter = |x: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize| {
            x[b] ^= x[a].wrapping_add(x[d]).rotate_left(7);
            x[c] ^= x[b].wrapping_add(x[a]).rotate_left(9);
            x[d] ^= x[c].wrapping_add(x[b]).rotate_left(13);
            x[a] ^= x[d].wrapping_add(x[c]).rotate_left(18);
        };
        for _ in 0..10 {
            quarter(&mut x, 0, 4, 8, 12);
            quarter(&mut x, 5, 9, 13, 1);
            quarter(&mut x, 10, 14, 2, 6);
            quarter(&mut x, 15, 3, 7, 11);
            quarter(&mut x, 0, 1, 2, 3);
            quarter(&mut x, 5, 6, 7, 4);
            quarter(&mut x, 10, 11, 8, 9);
            quarter(&mut x, 15, 12, 13, 14);
        }
        let mut out = [0; 64];
        for (index, word) in x.iter().enumerate() {
            let sum = word.wrapping_add(self.state[index]);
            out[4 * index..4 * index + 4].copy_from_slice(&sum.to_le_bytes());
        }
        self.state[8] = self.state[8].wrapping_add(1);
        if self.state[8] == 0 {
            self.state[9] = self.state[9].wrapping_add(1);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 128-bit key vector set 1, vector 0 of the eSTREAM Salsa20/20 tests.
    #[test]
    fn matches_the_estream_test_vector() {
        let mut key = [0; 16];
        key[0] = 0x80;
        let mut data = [0; 64];
        Salsa20::new(&key, &[0; 8]).apply(&mut data);
        let expected = "4dfa5e481da23ea09a31022050859936da52fcee218005164f267cb65f5cfd7f\
                        2b4f97e0ff16924a52df269515110a07f9e460bc65ef95da58f740b7d1dbb0aa";
        let hex: String = data.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, expected);
    }
}
