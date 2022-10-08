use base64::encode;
use eva_common::prelude::*;
use openssl::rand::rand_bytes;
use std::fmt::Write as _;

const BUF_SIZE: usize = 16;

pub fn random_string(len: usize) -> EResult<String> {
    let mut s = String::with_capacity(len);
    'outer: loop {
        let mut buf = [0; BUF_SIZE];
        rand_bytes(&mut buf).map_err(Error::core)?;
        for c in encode(buf).chars() {
            match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' => {
                    write!(s, "{}", c)?;
                    if s.len() == len {
                        break 'outer;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(s)
}

/// # Panics
///
/// Will panic if the system clock is not accessible
#[allow(clippy::cast_sign_loss)]
#[allow(clippy::cast_possible_truncation)]
pub fn aes_gcm_nonce() -> EResult<[u8; 12]> {
    let mut result = [0_u8; 12];
    let ns = (nix::time::clock_gettime(nix::time::ClockId::CLOCK_REALTIME)
        .unwrap()
        .tv_nsec() as u32)
        .to_le_bytes();
    let mut buf = [0; 8];
    rand_bytes(&mut buf).map_err(Error::core)?;
    result[..4].clone_from_slice(&ns);
    result[4..].clone_from_slice(&buf);
    Ok(result)
}

#[cfg(test)]
mod test {
    use super::{aes_gcm_nonce, random_string};
    #[test]
    fn test_random_string() {
        for _ in 0..1000 {
            let s = random_string(20).unwrap();
            assert_eq!(s.len(), 20);
        }
    }
    #[test]
    fn test_aes_gcm_nonce() {
        let n = aes_gcm_nonce().unwrap();
        assert_eq!(n.len(), 12);
    }
}
