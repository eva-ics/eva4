use super::Metric;
use eva_common::events::{RawStateEventOwned, RAW_STATE_TOPIC};
use eva_common::prelude::*;
use eva_common::ITEM_STATUS_ERROR;
use eva_sdk::prelude::*;
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashSet;

static OID_PREFIX: OnceCell<OID> = OnceCell::new();
static OIDS_CREATED: Lazy<Mutex<HashSet<OID>>> = Lazy::new(<_>::default);

pub fn set_oid_prefix(prefix: OID) -> EResult<()> {
    OID_PREFIX
        .set(prefix)
        .map_err(|_| Error::core("Unable to set OID_PREFIX"))
}

impl<'a> Metric<'a> {
    pub(super) async fn send_report<S: Serialize>(&self, value: S) -> EResult<()> {
        let oid = if let Some(n) = &self.subgroup {
            format!(
                "{}/{}/{}/{}",
                OID_PREFIX.get().unwrap(),
                self.group,
                n,
                self.resource
            )
        } else {
            format!(
                "{}/{}/{}",
                OID_PREFIX.get().unwrap(),
                self.group,
                self.resource
            )
        }
        .parse::<OID>()?;
        if !OIDS_CREATED.lock().contains(&oid) {
            eapi_bus::create_items(&[&oid]).await?;
            OIDS_CREATED.lock().insert(oid.clone());
        }
        let ev = if self.status == ITEM_STATUS_ERROR {
            RawStateEventOwned::new0(self.status)
        } else {
            RawStateEventOwned::new(self.status, to_value(value)?)
        };
        eapi_bus::publish(
            &format!("{}{}", RAW_STATE_TOPIC, oid.as_path()),
            pack(&ev)?.into(),
        )
        .await?;
        Ok(())
    }
}
