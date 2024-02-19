use base64::{decode, encode};
use eva_common::prelude::*;
use once_cell::sync::OnceCell;
use openssl::pkcs5;
use openssl::rand::rand_bytes;
use openssl::sha::{Sha256, Sha512};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{self, Write as _};
use std::str::FromStr;
use std::time::{Duration, Instant};

const DEFAULT_MIN_CTIME: Duration = Duration::from_millis(200);

static MIN_CTIME: OnceCell<Duration> = OnceCell::new();

/// Must be called only once
pub fn set_min_ctime(duration: Duration) -> EResult<()> {
    MIN_CTIME
        .set(duration)
        .map_err(|_| Error::failed("unable to set MIN_CTIME"))
}

pub fn get_min_ctime() -> Duration {
    MIN_CTIME.get().map_or(DEFAULT_MIN_CTIME, |v| *v)
}

const BUF_SIZE: usize = 16;
const PBKDF2_ITERS: usize = 100_000;

const ERR_INVALID_PASSWORD_HASH: &str = "Invalid password hash";

pub fn random_string(len: usize) -> EResult<String> {
    let mut s = String::with_capacity(len);
    'outer: loop {
        let mut buf = [0; BUF_SIZE];
        rand_bytes(&mut buf).map_err(Error::core)?;
        for c in encode(buf).chars() {
            if c.is_alphanumeric() {
                write!(s, "{}", c)?;
                if s.len() == len {
                    break 'outer;
                }
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

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Password {
    Sha256([u8; 32]),
    Sha512([u8; 64]),
    Pbkdf2([u8; 16], [u8; 32]),
}

impl fmt::Display for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Password::Sha256(hash) => {
                write!(f, "{}", hex::encode(hash))
            }
            Password::Sha512(hash) => {
                write!(f, "{}", hex::encode(hash))
            }
            Password::Pbkdf2(salt, hash) => {
                write!(f, "$1${}${}", base64::encode(salt), base64::encode(hash))
            }
        }
    }
}

impl FromStr for Password {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(pkcs_str) = s.strip_prefix("$1$") {
            let mut sp = pkcs_str.splitn(2, '$');
            let salt = decode(sp.next().unwrap()).map_err(Error::invalid_data)?;
            let hash = decode(
                sp.next()
                    .ok_or_else(|| Error::invalid_data(ERR_INVALID_PASSWORD_HASH))?,
            )
            .map_err(Error::invalid_data)?;
            Ok(Self::Pbkdf2(
                salt.try_into()
                    .map_err(|_| Error::invalid_data(ERR_INVALID_PASSWORD_HASH))?,
                hash.try_into()
                    .map_err(|_| Error::invalid_data(ERR_INVALID_PASSWORD_HASH))?,
            ))
        } else {
            match s.len() {
                64 => Ok(Self::Sha256(
                    hex::decode(s)
                        .map_err(|_| Error::invalid_data(ERR_INVALID_PASSWORD_HASH))?
                        .try_into()
                        .map_err(|_| Error::invalid_data(ERR_INVALID_PASSWORD_HASH))?,
                )),
                128 => Ok(Self::Sha512(
                    hex::decode(s)
                        .map_err(|_| Error::invalid_data(ERR_INVALID_PASSWORD_HASH))?
                        .try_into()
                        .map_err(|_| Error::invalid_data(ERR_INVALID_PASSWORD_HASH))?,
                )),
                _ => Err(Error::invalid_data(ERR_INVALID_PASSWORD_HASH)),
            }
        }
    }
}

macro_rules! op_delay {
    ($start: expr, $result: expr) => {{
        let elapsed = $start.elapsed();
        let min_c_time = get_min_ctime();
        if elapsed < min_c_time {
            tokio::time::sleep(min_c_time - elapsed).await;
        }
        $result
    }};
}

impl Password {
    pub async fn new_sha256(password: &str) -> Self {
        let op_start = Instant::now();
        let mut hasher = Sha256::new();
        hasher.update(password.as_bytes());
        op_delay!(op_start, Self::Sha256(hasher.finish()))
    }
    pub async fn new_sha512(password: &str) -> Self {
        let op_start = Instant::now();
        let mut hasher = Sha512::new();
        hasher.update(password.as_bytes());
        op_delay!(op_start, Self::Sha512(hasher.finish()))
    }
    pub async fn new_pbkdf2(password: &str) -> EResult<Self> {
        let op_start = Instant::now();
        let mut salt = [0; 16];
        let mut hash = [0; 32];
        rand_bytes(&mut salt).map_err(Error::core)?;
        pkcs5::pbkdf2_hmac(
            password.as_bytes(),
            &salt,
            PBKDF2_ITERS,
            openssl::hash::MessageDigest::sha256(),
            &mut hash,
        )
        .map_err(Error::core)?;
        op_delay!(op_start, Ok(Self::Pbkdf2(salt, hash)))
    }
    pub async fn verify(&self, password: &str) -> EResult<bool> {
        let op_start = Instant::now();
        op_delay!(
            op_start,
            Ok(match self {
                Password::Sha256(hash) => {
                    let mut hasher = Sha256::new();
                    hasher.update(password.as_bytes());
                    &hasher.finish() == hash
                }
                Password::Sha512(hash) => {
                    let mut hasher = Sha512::new();
                    hasher.update(password.as_bytes());
                    &hasher.finish() == hash
                }
                Password::Pbkdf2(salt, hash) => {
                    let mut password_hash = [0; 32];
                    pkcs5::pbkdf2_hmac(
                        password.as_bytes(),
                        salt,
                        PBKDF2_ITERS,
                        openssl::hash::MessageDigest::sha256(),
                        &mut password_hash,
                    )
                    .map_err(Error::core)?;
                    &password_hash == hash
                }
            })
        )
    }
}

impl<'de> Deserialize<'de> for Password {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Password, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl Serialize for Password {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod test {
    use super::{aes_gcm_nonce, random_string, Password};
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
    #[tokio::test]
    async fn test_password() {
        let p = "letmein";
        let password: Password = Password::new_sha256(p).await.to_string().parse().unwrap();
        assert!(password.verify(p).await.unwrap());
        let password: Password = Password::new_sha512(p).await.to_string().parse().unwrap();
        assert!(password.verify(p).await.unwrap());
        let password: Password = Password::new_pbkdf2(p)
            .await
            .unwrap()
            .to_string()
            .parse()
            .unwrap();
        assert!(password.verify(p).await.unwrap());
    }
}
