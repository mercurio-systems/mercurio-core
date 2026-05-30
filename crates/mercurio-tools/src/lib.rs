use std::io::Read;
use std::path::{Path, PathBuf};

pub const MERCURIO_WORKSPACE_ROOT_ENV: &str = "MERCURIO_WORKSPACE_ROOT";
pub const MERCURIO_PILOT_ROOT_ENV: &str = "MERCURIO_PILOT_ROOT";
pub const MERCURIO_EXAMPLES_ROOT_ENV: &str = "MERCURIO_EXAMPLES_ROOT";

const PILOT_REPO_NAME: &str = "SysML-v2-Pilot-Implementation";
const EXAMPLES_REPO_NAME: &str = "mercurio-examples";

pub fn default_pilot_root() -> PathBuf {
    if let Some(path) = env_path(MERCURIO_PILOT_ROOT_ENV) {
        return path;
    }

    if let Some(workspace_root) = env_path(MERCURIO_WORKSPACE_ROOT_ENV) {
        let sibling = workspace_root.join(PILOT_REPO_NAME);
        if sibling.exists() {
            return sibling;
        }
        return workspace_root.join("external").join(PILOT_REPO_NAME);
    }

    let external = PathBuf::from("../external").join(PILOT_REPO_NAME);
    if external.exists() {
        external
    } else {
        PathBuf::from("..").join(PILOT_REPO_NAME)
    }
}

pub fn default_kerml_examples_root(fallback_in_core: impl Into<PathBuf>) -> PathBuf {
    let fallback_in_core = fallback_in_core.into();

    if let Some(path) = env_path(MERCURIO_EXAMPLES_ROOT_ENV) {
        let kerml_examples = path.join("kerml").join("examples");
        if kerml_examples.exists() {
            return kerml_examples;
        }
        return path;
    }

    if let Some(workspace_root) = env_path(MERCURIO_WORKSPACE_ROOT_ENV) {
        let examples_root = workspace_root
            .join(EXAMPLES_REPO_NAME)
            .join("kerml")
            .join("examples");
        if examples_root.exists() {
            return examples_root;
        }
    }

    fallback_in_core
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn sha256_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(sha256_hex(&bytes))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut state = Sha256::new();
    state.update(bytes);
    state
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Sha256 {
    state: [u32; 8],
    length_bits: u64,
    buffer: Vec<u8>,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            length_bits: 0,
            buffer: Vec::new(),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        self.length_bits = self.length_bits.wrapping_add((bytes.len() as u64) * 8);
        let mut input = bytes;

        if !self.buffer.is_empty() {
            let needed = 64 - self.buffer.len();
            let take = needed.min(input.len());
            self.buffer.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.buffer.len() == 64 {
                let block = <[u8; 64]>::try_from(self.buffer.as_slice()).expect("full block");
                self.compress(&block);
                self.buffer.clear();
            }
        }

        while input.len() >= 64 {
            let block = <[u8; 64]>::try_from(&input[..64]).expect("full block");
            self.compress(&block);
            input = &input[64..];
        }

        self.buffer.extend_from_slice(input);
    }

    fn finalize(mut self) -> [u8; 32] {
        self.buffer.push(0x80);
        while self.buffer.len() % 64 != 56 {
            self.buffer.push(0);
        }
        self.buffer
            .extend_from_slice(&self.length_bits.to_be_bytes());
        let blocks = self
            .buffer
            .chunks(64)
            .map(<[u8; 64]>::try_from)
            .collect::<Result<Vec<_>, _>>()
            .expect("sha256 padding yields full blocks");
        for block in blocks {
            self.compress(&block);
        }
        let mut out = [0u8; 32];
        for (index, value) in self.state.iter().enumerate() {
            out[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8]) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut w = [0u32; 64];
        for (index, chunk) in block.chunks_exact(4).take(16).enumerate() {
            w[index] = u32::from_be_bytes(chunk.try_into().expect("four bytes"));
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (slot, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn sha256_hex_matches_empty_digest() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
