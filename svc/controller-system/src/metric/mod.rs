use eva_common::prelude::*;
use eva_common::{err_logger, ITEM_STATUS_ERROR};

err_logger!();

#[cfg(feature = "agent")]
pub mod client;
#[cfg(feature = "service")]
pub mod svc;

pub struct Metric {
    #[cfg(feature = "service")]
    oid: Option<OID>,
    #[cfg(not(feature = "service"))]
    i: String,
    status: ItemStatus,
}

impl Metric {
    #[inline]
    pub fn failed(mut self) -> Self {
        self.status = ITEM_STATUS_ERROR;
        self
    }
}
