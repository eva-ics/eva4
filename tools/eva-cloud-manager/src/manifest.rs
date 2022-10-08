use eva_client::VersionInfo;
use eva_common::{EResult, Error};
use openssl::sha::Sha256;
use ring::signature;
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncReadExt;

const FNAME_PARSE_ERROR: &str = "unable to parse the file name";
const BUF_SIZE: usize = 16_384;

#[derive(Deserialize, Clone)]
pub struct Manifest {
    #[serde(flatten)]
    version_info: VersionInfo,
    #[serde(default)]
    content: HashMap<PathBuf, Entry>,
    //#[serde(skip)]
    //pvt_key: Option<Vec<u8>>,
    #[serde(skip)]
    pub_key: Option<Vec<u8>>,
}

impl Manifest {
    #[inline]
    pub fn files(&self) -> Vec<&Path> {
        self.content.keys().map(AsRef::as_ref).collect()
    }
    #[inline]
    pub fn contains(&self, file: &Path) -> EResult<bool> {
        Ok(self.content.contains_key(Path::new(
            file.file_name()
                .ok_or_else(|| Error::invalid_data(FNAME_PARSE_ERROR))?,
        )))
    }
    #[inline]
    pub fn version_info(&self) -> &VersionInfo {
        &self.version_info
    }
    //#[inline]
    //pub fn set_pvt_key(&mut self, key: Vec<u8>) {
    //self.pvt_key.replace(key);
    //}
    #[inline]
    pub fn set_pub_key(&mut self, key: Vec<u8>) {
        self.pub_key.replace(key);
    }
    #[inline]
    pub async fn verify0(&self, path: &Path) -> EResult<()> {
        let fname = path
            .file_name()
            .ok_or_else(|| Error::invalid_params("invalid file name"))?;
        let mut file = fs::File::open(path).await?;
        let mut buf = [0; BUF_SIZE];
        let mut hasher = Sha256::new();
        loop {
            let r = file.read(&mut buf).await?;
            if r == 0 {
                break;
            }
            hasher.update(&buf[..r]);
        }
        let meta = file.metadata().await?;
        let size = meta.len();
        drop(file);
        self.verify(Path::new(fname), size, &hasher.finish())
    }
    pub fn verify(&self, name: &Path, size: u64, sha256sum: &[u8]) -> EResult<()> {
        let f = Path::new(
            name.file_name()
                .ok_or_else(|| Error::invalid_data(FNAME_PARSE_ERROR))?,
        );
        if let Some(entry) = self.content.get(f) {
            if entry.size() != size {
                return Err(Error::invalid_data(format!(
                    "file {} size mismatch",
                    f.to_string_lossy()
                )));
            }
            if entry.sha256() != sha256sum {
                return Err(Error::invalid_data(format!(
                    "file {} checksum mismatch",
                    f.to_string_lossy()
                )));
            }
            if let Some(ref key) = self.pub_key {
                if rsa_verify(sha256sum, key, entry.signature()) {
                    Ok(())
                } else {
                    Err(Error::invalid_data(format!(
                        "file {} signature verification failed",
                        f.to_string_lossy()
                    )))
                }
            } else {
                Err(Error::invalid_params("manifest pub key is not set"))
            }
        } else {
            Err(Error::not_found(format!(
                "file {} not included into the manifest",
                f.to_string_lossy()
            )))
        }
    }
}

#[derive(Deserialize, Clone)]
struct Entry {
    size: u64,
    #[serde(deserialize_with = "de_sha256_from_hexstr")]
    sha256: Vec<u8>, // hex encoded string
    #[serde(deserialize_with = "de_from_b64")]
    signature: Vec<u8>, // base64
}

pub fn de_sha256_from_hexstr<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    let data = hex::decode(s).map_err(serde::de::Error::custom)?;
    if data.len() == 32 {
        Ok(data)
    } else {
        Err(serde::de::Error::custom("invalid sha256 length"))
    }
}

pub fn de_from_b64<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    base64::decode(s).map_err(serde::de::Error::custom)
}

impl Entry {
    #[inline]
    pub fn size(&self) -> u64 {
        self.size
    }
    #[inline]
    pub fn sha256(&self) -> &[u8] {
        &self.sha256
    }
    #[inline]
    pub fn signature(&self) -> &[u8] {
        &self.signature
    }
}

//fn rsa_sign(content: &[u8], pvt_key: &[u8]) -> EResult<Vec<u8>> {
//let key_pair = signature::RsaKeyPair::from_der(pvt_key).map_err(Error::invalid_data)?;
//let rng = ring::rand::SystemRandom::new();
//let mut signature = vec![0; key_pair.public_modulus_len()];
//key_pair
//.sign(&signature::RSA_PKCS1_SHA256, &rng, content, &mut signature)
//.map_err(Error::invalid_data)?;
//Ok(signature)
//}

fn rsa_verify(content: &[u8], pub_key: &[u8], signature: &[u8]) -> bool {
    let k = signature::UnparsedPublicKey::new(&signature::RSA_PKCS1_2048_8192_SHA256, pub_key);
    k.verify(content, signature).is_ok()
}
