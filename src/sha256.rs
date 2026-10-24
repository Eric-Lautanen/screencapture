//! SHA-256 (FIPS 180-4) in pure `std`: the bundled engine's integrity
//! record (`vendor/ffmpeg/engine.sha256`) is computed and verified with
//! this, so an updated engine binary is caught at `--check` before it
//! ships.

/// Streaming hasher: `update` reads files in chunks, `finish` pads once.
pub struct Sha256 {
    /// The 1-based absolutely-compressible state: `H` words.
    state: [u32; 8],
    /// Wound-up block bytes; flushed when `block.len() == BLOCK`.
    block: [u8; BLOCK],
    /// Bytes fed since construction, modulo the padding arithmetic.
    total: u64,
    /// How much of `block` is pending.
    pending: usize,
}

/// One SHA-256 compression block, in bytes.
const BLOCK: usize = 64;

// FIPS 180-4 (4.2.2): fractional parts of the cube roots of the
// first 64 primes.
const ROUND: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            // FIPS 180-4: the fractional parts of the square roots of the
            // first 8 primes.
            state: [
                0x6a09_e667,
                0xbb67_ae85,
                0x3c6e_f372,
                0xa54f_f53a,
                0x510e_527f,
                0x9b05_688c,
                0x1f83_d9ab,
                0x5be0_cd19,
            ],
            block: [0; BLOCK],
            total: 0,
            pending: 0,
        }
    }

    /// Feed more bytes; buffering stays inside one block.
    pub fn update(&mut self, bytes: &[u8]) {
        self.total = self
            .total
            .wrapping_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let mut cursor = bytes;
        if !cursor.is_empty() && self.pending > 0 {
            let fill = (BLOCK - self.pending).min(cursor.len());
            self.block[self.pending..self.pending + fill].copy_from_slice(&cursor[..fill]);
            self.pending += fill;
            cursor = &cursor[fill..];
            if self.pending == BLOCK {
                let block = self.block;
                self.compression(block);
                self.pending = 0;
            }
        }
        for chunk in cursor.as_chunks::<BLOCK>().0 {
            let mut block = [0; BLOCK];
            block.copy_from_slice(chunk);
            self.compression(block);
        }
        if self.pending > 0 {
            // The partial fill consumed everything the cursor had left.
            return;
        }
        let tail = cursor.len() % BLOCK;
        self.block[..tail].copy_from_slice(&cursor[cursor.len() - tail..]);
        self.pending = tail;
    }

    /// One round: message schedule then the 64-step compression.
    fn compression(&mut self, block: [u8; BLOCK]) {
        let mut words = [0_u32; 64];
        for (word, piece) in words.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*piece);
        }
        for index in 16..64 {
            let left = words[index - 15];
            let right = words[index - 2];
            words[index] = sigma_small_1(right)
                .wrapping_add(words[index - 7])
                .wrapping_add(sigma_small_0(left))
                .wrapping_add(words[index - 16]);
        }
        // FIPS notes the eight running registers as a..h; longer names
        // here map one-to-one.
        let [
            mut alpha,
            mut beta,
            mut gamma,
            mut delta,
            mut eps,
            mut zeta,
            mut eta,
            mut theta,
        ] = self.state;
        for (&word, &constant) in words.iter().zip(ROUND.iter()) {
            let compressed = theta
                .wrapping_add(sigma_big_1(eps))
                .wrapping_add(choice(eps, zeta, eta))
                .wrapping_add(constant)
                .wrapping_add(word);
            let votes = sigma_big_0(alpha).wrapping_add(majority(alpha, beta, gamma));
            theta = eta;
            eta = zeta;
            zeta = eps;
            eps = delta.wrapping_add(compressed);
            delta = gamma;
            gamma = beta;
            beta = alpha;
            alpha = compressed.wrapping_add(votes);
        }
        let state = &mut self.state;
        let steps = [alpha, beta, gamma, delta, eps, zeta, eta, theta];
        for (slot, step) in state.iter_mut().zip(steps) {
            *slot = slot.wrapping_add(step);
        }
    }

    /// Completed digest with one-pad and the 64-bit length.
    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.total.wrapping_mul(8);
        self.update(&[0x80]);
        while self.pending != 56 {
            self.update(&[0]);
        }
        let length = bits.to_be_bytes();
        // The final block: commit the length directly, bypassing the
        // accounting records (update would wrap the 64-bit meter).
        self.block[56..64].copy_from_slice(&length);
        let block = self.block;
        self.compression(block);
        let mut digest = [0; 32];
        for (piece, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *piece = word.to_be_bytes();
        }
        digest
    }
}

/// Lowercase hex, the wire format of the `.sha256` record.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        text.push(DIGITS[usize::from(byte >> 4)] as char);
        text.push(DIGITS[usize::from(byte & 0x0F)] as char);
    }
    text
}

/// σ₀ (FIPS 180-4 formula 4.3).
fn sigma_small_0(word: u32) -> u32 {
    word.rotate_right(7) ^ word.rotate_right(18) ^ (word >> 3)
}

/// σ₁.
fn sigma_small_1(word: u32) -> u32 {
    word.rotate_right(17) ^ word.rotate_right(19) ^ (word >> 10)
}

/// Σ₀.
fn sigma_big_0(word: u32) -> u32 {
    word.rotate_right(2) ^ word.rotate_right(13) ^ word.rotate_right(22)
}

/// Σ₁.
fn sigma_big_1(word: u32) -> u32 {
    word.rotate_right(6) ^ word.rotate_right(11) ^ word.rotate_right(25)
}

/// f: (x and y) xor (not x and z).
fn choice(cond: u32, when_true: u32, when_false: u32) -> u32 {
    (cond & when_true) | (!cond & when_false)
}

/// Maj: bitwise vote of three words.
fn majority(a: u32, b: u32, c: u32) -> u32 {
    (a & b) | (a & c) | (b & c)
}

#[cfg(test)]
mod tests {
    use super::{Sha256, hex};

    #[test]
    fn empty_input_matches_the_standard_vector() {
        assert_eq!(
            hex(&Sha256::new().finish()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn short_input_matches_the_standard_vector() {
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        assert_eq!(
            hex(&hasher.finish()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn one_million_a_marks_match_the_standard_vector() {
        // The million-'a' vector (RFC 6234 corpus), fed in quarter chunks
        // to exercise multi-block updates.
        let chunk = vec![b'a'; 250_000];
        let mut hasher = Sha256::new();
        for _ in 0..4 {
            hasher.update(&chunk);
        }
        assert_eq!(
            hex(&hasher.finish()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn incremental_and_one_shot_agree() {
        let payload: Vec<u8> = (0..10_000_u32)
            .map(|index| u8::try_from(index % 251).unwrap_or(0))
            .collect();
        let mut hasher = Sha256::new();
        for piece in payload.chunks(37) {
            hasher.update(piece);
        }
        let mut one_shot = Sha256::new();
        one_shot.update(&payload);
        assert_eq!(hasher.finish(), one_shot.finish());
    }
}
