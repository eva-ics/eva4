use crate::{COMPRESSION_BZIP2, COMPRESSION_NO};
use crate::{ENCRYPTION_AES_128_GCM, ENCRYPTION_AES_256_GCM, ENCRYPTION_NO};
use eva_common::{EResult, Error};
use rand::Rng;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::sync::Arc;

use aes_gcm::{aead::Aead, Aes128Gcm, Aes256Gcm, Key, NewAead, Nonce};
use bzip2::read::{BzDecoder, BzEncoder};

#[inline]
fn generate_nonce() -> [u8; 12] {
    let aes_nonce: [u8; 12] = rand::thread_rng()
        .sample_iter(&rand::distributions::Uniform::new(0, 0xff))
        .take(12)
        .map(u8::from)
        .collect::<Vec<u8>>()
        .try_into()
        .unwrap();
    aes_nonce
}

pub fn parse_flags(flags: u8) -> EResult<(Encryption, Compression)> {
    let encryption: Encryption = (flags & 0b1111).try_into()?;
    let compression: Compression = (flags >> 4 & 0b11).try_into()?;
    Ok((encryption, compression))
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum Encryption {
    No,
    Aes128Gcm,
    Aes256Gcm,
}

impl Default for Encryption {
    #[inline]
    fn default() -> Self {
        Encryption::No
    }
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

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum Compression {
    No = COMPRESSION_NO,
    Bzip2 = COMPRESSION_BZIP2,
}

impl Default for Compression {
    #[inline]
    fn default() -> Self {
        Compression::No
    }
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

#[derive(Clone)]
enum Cipher {
    Aes128Gcm(Box<Aes128Gcm>),
    Aes256Gcm(Box<Aes256Gcm>),
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
    cip: Option<Arc<Cipher>>,
}

impl Options {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }
    #[inline]
    pub fn encryption(mut self, encryption: Encryption, key: &EncryptionKey) -> Self {
        let (cip, key_id) = match encryption {
            Encryption::No => (None, None),
            Encryption::Aes128Gcm => {
                let mut hasher = Sha256::new();
                hasher.update(&key.value);
                let key_hash = &hasher.finalize()[..16];
                let enc_key = Key::from_slice(key_hash);
                (
                    Some(Arc::new(Cipher::Aes128Gcm(Box::new(Aes128Gcm::new(
                        enc_key,
                    ))))),
                    Some(key.id),
                )
            }
            Encryption::Aes256Gcm => {
                let mut hasher = Sha256::new();
                hasher.update(&key.value);
                let key_hash = &hasher.finalize();
                let enc_key = Key::from_slice(key_hash);
                (
                    Some(Arc::new(Cipher::Aes256Gcm(Box::new(Aes256Gcm::new(
                        enc_key,
                    ))))),
                    Some(key.id),
                )
            }
        };
        self.cip = cip;
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
        let result = match self.encryption {
            Encryption::No => data,
            Encryption::Aes128Gcm => {
                let cip = self.cip.as_ref().unwrap().clone();
                let encr: EResult<Vec<u8>> = tokio::task::spawn_blocking(move || {
                    let nonce = generate_nonce();
                    if let Cipher::Aes128Gcm(cip) = cip.as_ref() {
                        let mut buf = cip
                            .encrypt(Nonce::from_slice(&nonce), data.as_ref())
                            .map_err(Error::failed)?;
                        buf.extend(nonce);
                        Ok(buf)
                    } else {
                        panic!()
                    }
                })
                .await
                .map_err(Error::failed)?;
                encr?
            }
            Encryption::Aes256Gcm => {
                let cip = self.cip.as_ref().unwrap().clone();
                let encr: EResult<Vec<u8>> = tokio::task::spawn_blocking(move || {
                    let nonce = generate_nonce();
                    if let Cipher::Aes256Gcm(cip) = cip.as_ref() {
                        let mut buf = cip
                            .encrypt(Nonce::from_slice(&nonce), data.as_ref())
                            .map_err(Error::failed)?;
                        buf.extend(nonce);
                        Ok(buf)
                    } else {
                        panic!()
                    }
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
        let data = match self.encryption {
            Encryption::No => message.data()[payload_pos..].to_vec(),
            Encryption::Aes128Gcm => {
                let cip = self.cip.as_ref().unwrap().clone();
                let encr: EResult<Vec<u8>> = tokio::task::spawn_blocking(move || {
                    let payload = &message.data()[payload_pos..];
                    if payload.len() < 13 {
                        return Err(Error::invalid_data("invalid payload"));
                    }
                    let nonce = &payload[payload.len() - 12..];
                    let encpl = &payload[..payload.len() - 12];
                    if let Cipher::Aes128Gcm(cip) = cip.as_ref() {
                        cip.decrypt(Nonce::from_slice(nonce), encpl)
                            .map_err(Error::failed)
                    } else {
                        panic!()
                    }
                })
                .await?;
                encr?
            }
            Encryption::Aes256Gcm => {
                let cip = self.cip.as_ref().unwrap().clone();
                let encr: EResult<Vec<u8>> = tokio::task::spawn_blocking(move || {
                    let payload = &message.data()[payload_pos..];
                    if payload.len() < 13 {
                        return Err(Error::invalid_data("invalid payload"));
                    }
                    let nonce = &payload[payload.len() - 12..];
                    let encpl = &payload[..payload.len() - 12];
                    if let Cipher::Aes256Gcm(cip) = cip.as_ref() {
                        cip.decrypt(Nonce::from_slice(nonce), encpl)
                            .map_err(Error::failed)
                    } else {
                        panic!()
                    }
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
