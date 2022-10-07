use base64::encode;
use eva_common::prelude::*;
use openssl::rand::rand_bytes;
use std::fmt::Write as _;

const BUF_SIZE: usize = 16;

pub fn random_string(len: usize) -> EResult<String> {
    let mut s = String::with_capacity(len);
    while s.len() < len {
        let mut buf = [0; BUF_SIZE];
        rand_bytes(&mut buf).map_err(Error::core)?;
        for c in encode(buf).chars() {
            match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' => {
                    write!(s, "{}", c)?;
                }
                _ => {}
            }
        }
    }
    Ok(s)
}

#[cfg(test)]
mod test {
    use super::random_string;
    #[test]
    fn test1() {
        let _s = random_string(20).unwrap();
    }
}
