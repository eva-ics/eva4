use pgrx::prelude::*;

pgrx::pg_module_magic!();

const WILDCARDS: &[&str] = &["*", "#"];
const MATCH_ANY: &[&str] = &["+", "?"];

#[pg_extern]
fn oid_match(oid: &str, mask: &str) -> bool {
    if WILDCARDS.contains(&mask) {
        return true;
    }
    let mut sp = oid.splitn(2, ':');
    let kind = sp.next().unwrap();
    let Some(full_id) = sp.next() else {
        ereport!(
            PgLogLevel::ERROR,
            PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
            "Invalid OID"
        );
        return false;
    };
    let mut mask_sp = mask.splitn(2, ':');
    let mask_kind = mask_sp.next().unwrap();
    let Some(mask_full_id) = mask_sp.next() else {
        ereport!(
            PgLogLevel::ERROR,
            PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
            "Invalid OID mask"
        );
        return false;
    };
    if !MATCH_ANY.contains(&mask_kind) && mask_kind != kind {
        return false;
    }
    let mut mask_sp = mask_full_id.split('/');
    for chunk in full_id.split('/') {
        let Some(mask_chunk) = mask_sp.next() else {
            return false;
        };
        if WILDCARDS.contains(&mask_chunk) {
            return true;
        }
        if !MATCH_ANY.contains(&mask_chunk) && mask_chunk != chunk {
            return false;
        }
    }
    mask_sp.next().is_none()
}
