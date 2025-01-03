use crate::{COMPRESSION_BZIP2, COMPRESSION_NO};
use crate::{ENCRYPTION_AES_128_GCM, ENCRYPTION_AES_256_GCM, ENCRYPTION_NO};
use bzip2::read::{BzDecoder, BzEncoder};
use eva_common::{EResult, Error};
use genpass_native::aes_gcm_nonce;
use openssl::sha::Sha256;
use std::io::Read;
use std::sync::Arc;

pub fn parse_flags(flags: u8) -> EResult<(Encryption, Compression)> {
    let encryption: Encryption = (flags & 0b1111).try_into()?;
    let compression: Compression = (flags >> 4 & 0b11).try_into()?;
    Ok((encryption, compression))
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Default)]
pub enum Encryption {
    #[default]
    No,
    Aes128Gcm,
    Aes256Gcm,
}

impl Encryption {
    fn code(self) -> u8 {
        match self {
            Encryption::No => ENCRYPTION_NO,
            Encryption::Aes128Gcm => ENCRYPTION_AES_128_GCM,
            Encryption::Aes256Gcm => ENCRYPTION_AES_256_GCM,
        }
    }
}

impl TryFrom<u8> for Encryption {
    type Error = Error;
    fn try_from(code: u8) -> EResult<Self> {
        match code {
            ENCRYPTION_NO => Ok(Encryption::No),
            ENCRYPTION_AES_128_GCM => Ok(Encryption::Aes128Gcm),
            ENCRYPTION_AES_256_GCM => Ok(Encryption::Aes256Gcm),
            v => Err(Error::invalid_data(format!(
                "invaid encryption type code: {}",
                v
            ))),
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Default)]
#[repr(u8)]
pub enum Compression {
    #[default]
    No = COMPRESSION_NO,
    Bzip2 = COMPRESSION_BZIP2,
}

impl TryFrom<u8> for Compression {
    type Error = Error;
    fn try_from(code: u8) -> EResult<Self> {
        match code {
            COMPRESSION_NO => Ok(Compression::No),
            COMPRESSION_BZIP2 => Ok(Compression::Bzip2),
            v => Err(Error::invalid_data(format!(
                "invaid compression type code: {}",
                v
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EncryptionKey<'a> {
    id: &'a str,
    value: &'a str,
}

impl<'a> EncryptionKey<'a> {
    pub fn new(id: &'a str, value: &'a str) -> Self {
        Self { id, value }
    }
}

#[derive(Clone, Default)]
pub struct Options {
    encryption: Encryption,
    compression: Compression,
    key_id: Option<String>,
    key_hash: Option<Arc<Vec<u8>>>,
}

impl Options {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }
    #[inline]
    pub fn encryption(mut self, encryption: Encryption, key: &EncryptionKey) -> Self {
        let (key_hash, key_id) = match encryption {
            Encryption::No => (None, None),
            Encryption::Aes128Gcm => {
                let mut hasher = Sha256::new();
                hasher.update(key.value.as_bytes());
                let key_hash = &hasher.finish()[..16];
                (Some(Arc::new(key_hash.to_vec())), Some(key.id))
            }
            Encryption::Aes256Gcm => {
                let mut hasher = Sha256::new();
                hasher.update(key.value.as_bytes());
                let key_hash = &hasher.finish();
                (Some(Arc::new(key_hash.to_vec())), Some(key.id))
            }
        };
        self.key_hash = key_hash;
        self.key_id = key_id.map(ToOwned::to_owned);
        self.encryption = encryption;
        self
    }
    #[inline]
    pub fn compression(mut self, compression: Compression) -> Self {
        self.compression = compression;
        self
    }
    #[inline]
    pub fn flags(&self) -> u8 {
        ((self.compression as u8) << 4) + self.encryption.code()
    }
    #[inline]
    pub fn key_id(&self) -> Option<&str> {
        self.key_id.as_deref()
    }
    /// # Panics
    ///
    /// Should not panic
    pub async fn pack_payload(&self, payload: Vec<u8>) -> EResult<Vec<u8>> {
        let data = match self.compression {
            Compression::No => payload,
            Compression::Bzip2 => {
                let compr: EResult<Vec<u8>> = tokio::task::spawn_blocking(move || {
                    let cursor = std::io::Cursor::new(payload);
                    let mut compressor = BzEncoder::new(cursor, bzip2::Compression::default());
                    let mut buf = Vec::new();
                    compressor.read_to_end(&mut buf)?;
                    Ok(buf)
                })
                .await
                .map_err(Error::failed)?;
                compr?
            }
        };
        macro_rules! encrypt {
            ($cip: expr, $key_hash: expr) => {{
                let iv = aes_gcm_nonce()?;
                let mut digest = [0_u8; 16];
                let mut buf = openssl::symm::encrypt_aead(
                    $cip,
                    $key_hash,
                    Some(&iv),
                    &[],
                    &data,
                    &mut digest,
                )
                .map_err(Error::io)?;
                buf.reserve(28);
                buf.extend(digest);
                buf.extend(iv);
                Ok(buf)
            }};
        }
        let result = match self.encryption {
            Encryption::No => data,
            Encryption::Aes128Gcm => {
                let key_hash = self.key_hash.clone().unwrap();
                let encr: EResult<Vec<u8>> = tokio::task::spawn_blocking(move || {
                    let cip = openssl::symm::Cipher::aes_128_gcm();
                    encrypt!(cip, &key_hash)
                })
                .await
                .map_err(Error::failed)?;
                encr?
            }
            Encryption::Aes256Gcm => {
                let key_hash = self.key_hash.clone().unwrap();
                let encr: EResult<Vec<u8>> = tokio::task::spawn_blocking(move || {
                    let cip = openssl::symm::Cipher::aes_256_gcm();
                    encrypt!(cip, &key_hash)
                })
                .await
                .map_err(Error::failed)?;
                encr?
            }
        };
        Ok(result)
    }
    /// # Panics
    ///
    /// Should not panic
    pub async fn unpack_payload(
        &self,
        message: Box<dyn crate::pubsub::Message + Send + Sync>,
        payload_pos: usize,
    ) -> EResult<Vec<u8>> {
        if message.data().len() < payload_pos {
            return Err(Error::invalid_data("message too short"));
        }
        macro_rules! decrypt {
            ($cip: expr, $key_hash: expr, $payload: expr) => {{
                let len = $payload.len();
                if len < 28 {
                    return Err(Error::invalid_data("invalid payload"));
                }
                let iv = &$payload[len - 12..];
                let encpl = &$payload[..len - 28];
                let digest = &$payload[len - 28..len - 12];
                openssl::symm::decrypt_aead($cip, $key_hash, Some(iv), &[], &encpl, &digest)
                    .map_err(Error::io)
            }};
        }
        let data = match self.encryption {
            Encryption::No => message.data()[payload_pos..].to_vec(),
            Encryption::Aes128Gcm => {
                let key_hash = self.key_hash.clone().unwrap();
                let encr: EResult<Vec<u8>> = tokio::task::spawn_blocking(move || {
                    let cip = openssl::symm::Cipher::aes_128_gcm();
                    let payload = &message.data()[payload_pos..];
                    decrypt!(cip, &key_hash, payload)
                })
                .await?;
                encr?
            }
            Encryption::Aes256Gcm => {
                let key_hash = self.key_hash.clone().unwrap();
                let encr: EResult<Vec<u8>> = tokio::task::spawn_blocking(move || {
                    let cip = openssl::symm::Cipher::aes_256_gcm();
                    let payload = &message.data()[payload_pos..];
                    decrypt!(cip, &key_hash, payload)
                })
                .await
                .map_err(Error::failed)?;
                encr?
            }
        };
        let result = match self.compression {
            Compression::No => data,
            Compression::Bzip2 => {
                let compr: EResult<Vec<u8>> = tokio::task::spawn_blocking(move || {
                    let cursor = std::io::Cursor::new(data);
                    let mut compressor = BzDecoder::new(cursor);
                    let mut buf = Vec::new();
                    compressor.read_to_end(&mut buf)?;
                    Ok(buf)
                })
                .await
                .map_err(Error::failed)?;
                compr?
            }
        };
        Ok(result)
    }
}
