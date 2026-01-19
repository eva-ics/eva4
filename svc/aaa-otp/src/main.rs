use bmart::tools::Sorting;
use eva_common::common_payloads::ParamsId;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex, OnceLock};

err_logger!();

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "OTP auth service";

static OTPS: LazyLock<Mutex<HashMap<String, Otp>>> = LazyLock::new(<_>::default);
static REG: OnceLock<Registry> = OnceLock::new();

/// full otp object, stored in the registry
#[derive(Debug, Serialize, Deserialize, Sorting)]
#[sorting(id = "login")]
#[serde(deny_unknown_fields)]
struct Otp {
    login: String,
    secret: String,
    active: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrInt {
    Str(String),
    Int(u32),
}

impl StringOrInt {
    fn into_string(self) -> String {
        match self {
            StringOrInt::Str(v) => v,
            StringOrInt::Int(v) => v.to_string(),
        }
    }
}

impl Otp {
    fn create(login: &str) -> EResult<Self> {
        let mut rand_otp = [0_u8; 16];
        openssl::rand::rand_bytes(&mut rand_otp)?;
        Ok(Self {
            login: login.to_owned(),
            secret: base32::encode(base32::Alphabet::RFC4648 { padding: false }, &rand_otp),
            active: false,
        })
    }
    fn verify(&self, otp_password: &str, svc_id: &str) -> EResult<()> {
        #[allow(deprecated)]
        let auth = boringauth::oath::TOTPBuilder::new()
            .base32_key(&self.secret)
            .output_len(6)
            .tolerance(1)
            .hash_function(boringauth::oath::HashFunction::Sha1)
            .finalize()
            .map_err(|e| Error::failed(format!("{:?}", e)))?;
        if auth.generate() == otp_password {
            Ok(())
        } else {
            Err(Error::access_more_data_required(format!(
                "|OTP|{}|INVALID",
                svc_id
            )))
        }
    }
}

/// otp info object, provided on list
#[derive(Serialize, Sorting)]
#[sorting(id = "login")]
struct OtpInfo<'a> {
    login: &'a str,
}

impl<'a> From<&'a Otp> for OtpInfo<'a> {
    fn from(otp: &'a Otp) -> OtpInfo<'a> {
        OtpInfo { login: &otp.login }
    }
}

struct Handlers {
    info: ServiceInfo,
    exclude: HashSet<String>,
    svc_id: String,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "otp.list" => {
                if payload.is_empty() {
                    let otps = OTPS.lock().unwrap();
                    let mut otp_info: Vec<OtpInfo> =
                        otps.values().filter(|v| v.active).map(Into::into).collect();
                    otp_info.sort();
                    Ok(Some(pack(&otp_info)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "otp.destroy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsId = unpack(payload)?;
                    let reg = REG.get().unwrap();
                    if OTPS.lock().unwrap().remove(p.i).is_some() {
                        reg.key_delete(&format!("otp/{}", p.i)).await?;
                        warn!("otp destroyed: {}", p.i);
                        Ok(None)
                    } else {
                        Err(Error::not_found(format!("otp {} not found", p.i)).into())
                    }
                }
            }
            "otp.check" => {
                #[derive(Deserialize)]
                struct ParamsCheck<'a> {
                    #[serde(borrow)]
                    login: &'a str,
                    otp: Option<StringOrInt>,
                }
                if payload.is_empty() {
                    return Err(RpcError::params(None));
                }
                let p: ParamsCheck = unpack(payload)?;
                if self.exclude.contains(p.login) {
                    Ok(None)
                } else {
                    let (res, val) = {
                        let mut otps = OTPS.lock().unwrap();
                        macro_rules! setup_otp {
                            () => {{
                                let otp = Otp::create(p.login)?;
                                let resp_str = format!("|OTP|{}|SETUP={}", self.svc_id, otp.secret);
                                otps.insert(p.login.to_owned(), otp);
                                (Err(Error::access_more_data_required(resp_str).into()), None)
                            }};
                        }
                        if let Some(otp) = otps.get_mut(p.login) {
                            if let Some(otp_password) = p.otp {
                                otp.verify(&otp_password.into_string(), &self.svc_id)?;
                                let val = if otp.active {
                                    None
                                } else {
                                    otp.active = true;
                                    Some(to_value(otp)?)
                                };
                                (Ok(None), val)
                            } else if !otp.active {
                                setup_otp!()
                            } else {
                                (
                                    Err(Error::access_more_data_required(format!(
                                        "|OTP|{}|REQ",
                                        self.svc_id
                                    ))
                                    .into()),
                                    None,
                                )
                            }
                        } else {
                            setup_otp!()
                        }
                    };
                    if let Some(value) = val {
                        let registry = REG.get().unwrap();
                        registry.key_set(&format!("otp/{}", p.login), value).await?;
                    }
                    res
                }
            }
            m => svc_handle_default_rpc(m, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    exclude: HashSet<String>,
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("otp.list"));
    info.add_method(ServiceMethod::new("otp.destroy").required("i"));
    let handlers = Handlers {
        info,
        exclude: config.exclude,
        svc_id: initial.id().to_owned(),
    };
    let rpc = initial.init_rpc(handlers).await?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    svc_init_logs(&initial, client.clone())?;
    let registry = initial.init_registry(&rpc);
    let reg_otps = registry.key_get_recursive("otp").await?;
    {
        let mut otps = OTPS.lock().unwrap();
        for (k, v) in reg_otps {
            debug!("otp loaded: {}", k);
            let otp: Otp = Otp::deserialize(v)?;
            otps.insert(otp.login.clone(), otp);
        }
    }
    REG.set(registry)
        .map_err(|_| Error::core("unable to set registry object"))?;
    svc_start_signal_handlers();
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
