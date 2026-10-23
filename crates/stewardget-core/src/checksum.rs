use std::fmt;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::Error;

/// Supported hash algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HashAlgo {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

impl HashAlgo {
    pub fn name(self) -> &'static str {
        match self {
            HashAlgo::Md5 => "md5",
            HashAlgo::Sha1 => "sha1",
            HashAlgo::Sha256 => "sha256",
            HashAlgo::Sha512 => "sha512",
        }
    }

    fn hex_len(self) -> usize {
        match self {
            HashAlgo::Md5 => 32,
            HashAlgo::Sha1 => 40,
            HashAlgo::Sha256 => 64,
            HashAlgo::Sha512 => 128,
        }
    }
}

/// An expected digest, written as `algo:hex` (e.g. `sha256:9f86d0…`) or as bare
/// hex, in which case the algorithm is inferred from its length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checksum {
    pub algo: HashAlgo,
    pub hex: String,
}

impl FromStr for Checksum {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let s = s.trim();
        let (algo, hex) = match s.split_once([':', '=']) {
            Some((name, hex)) => {
                let algo = match name.trim().to_ascii_lowercase().replace('-', "").as_str() {
                    "md5" => HashAlgo::Md5,
                    "sha1" => HashAlgo::Sha1,
                    "sha256" => HashAlgo::Sha256,
                    "sha512" => HashAlgo::Sha512,
                    other => return Err(Error::InvalidChecksum(format!("unknown algorithm `{other}`"))),
                };
                (algo, hex.trim())
            }
            None => {
                let algo = [HashAlgo::Md5, HashAlgo::Sha1, HashAlgo::Sha256, HashAlgo::Sha512]
                    .into_iter()
                    .find(|a| a.hex_len() == s.len())
                    .ok_or_else(|| Error::InvalidChecksum("cannot infer algorithm; use `sha256:<hex>`".into()))?;
                (algo, s)
            }
        };
        if hex.len() != algo.hex_len() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::InvalidChecksum(format!("expected {} hex digits for {}", algo.hex_len(), algo.name())));
        }
        Ok(Checksum { algo, hex: hex.to_ascii_lowercase() })
    }
}

impl fmt::Display for Checksum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algo.name(), self.hex)
    }
}

impl Checksum {
    /// Hashes `path` (blocking) and compares. Returns the actual digest on mismatch.
    pub fn verify(&self, path: &Path) -> Result<(), Error> {
        let actual = hash_file(self.algo, path)?;
        if actual == self.hex { Ok(()) } else { Err(Error::ChecksumMismatch { expected: self.hex.clone(), actual }) }
    }
}

/// Computes the lowercase hex digest of a file, streaming it in 1 MiB blocks.
pub fn hash_file(algo: HashAlgo, path: &Path) -> std::io::Result<String> {
    fn run<D: Digest>(mut file: std::fs::File) -> std::io::Result<String> {
        let mut hasher = D::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(to_hex(&hasher.finalize()))
    }
    let file = std::fs::File::open(path)?;
    match algo {
        HashAlgo::Md5 => run::<md5::Md5>(file),
        HashAlgo::Sha1 => run::<sha1::Sha1>(file),
        HashAlgo::Sha256 => run::<sha2::Sha256>(file),
        HashAlgo::Sha512 => run::<sha2::Sha512>(file),
    }
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0xf)] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        let c: Checksum = "SHA256:E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855".parse().unwrap();
        assert_eq!(c.algo, HashAlgo::Sha256);
        let c: Checksum = "d41d8cd98f00b204e9800998ecf8427e".parse().unwrap();
        assert_eq!(c.algo, HashAlgo::Md5);
        assert!("sha256:abc".parse::<Checksum>().is_err());
        assert!("crc:abc".parse::<Checksum>().is_err());
    }

    #[test]
    fn hashes_empty_file() {
        let dir = std::env::temp_dir().join(format!("stewardget-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty");
        std::fs::write(&path, b"").unwrap();
        assert_eq!(
            hash_file(HashAlgo::Sha256, &path).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(hash_file(HashAlgo::Md5, &path).unwrap(), "d41d8cd98f00b204e9800998ecf8427e");
        std::fs::remove_dir_all(dir).ok();
    }
}
