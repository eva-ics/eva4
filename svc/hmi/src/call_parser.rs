use eva_common::prelude::*;
use std::collections::BTreeMap;

fn parse_val(s: &str) -> EResult<Value> {
    if s.contains(',') {
        let mut result = Vec::new();
        for chunk in s.split(',') {
            if !chunk.is_empty() {
                result.push(parse_val(chunk)?);
            }
        }
        Ok(Value::Seq(result))
    } else {
        Ok(s.parse()?)
    }
}

fn parse_param(
    param: String,
    result: &mut BTreeMap<Value, Value>,
    default: Option<&str>,
) -> EResult<()> {
    let mut sp = param.splitn(2, '=');
    let mut key = sp.next().unwrap();
    if let Some(value) = sp.next() {
        let mut sp_key = key.splitn(2, '.');
        key = sp_key.next().unwrap();
        let val = parse_val(value)?;
        if let Some(n) = sp_key.next() {
            if let Some(v) = result.get_mut(&Value::String(key.to_owned())) {
                if let Value::Map(m) = v {
                    m.insert(Value::String(n.to_owned()), val);
                } else {
                    return Err(Error::invalid_params(format!("{key} is not a map")));
                }
            } else {
                let mut m = BTreeMap::new();
                m.insert(Value::String(n.to_owned()), val);
                result.insert(Value::String(key.to_owned()), Value::Map(m));
            }
        } else {
            result.insert(Value::String(key.to_owned()), val);
        }
    } else if let Some(d) = default {
        result.insert(Value::String(d.to_owned()), parse_val(key)?);
    } else {
        return Err(Error::invalid_params(format!(
            "{key} specified with no key"
        )));
    }
    Ok(())
}

pub fn parse_call_str(s: &str) -> EResult<(String, Value)> {
    let mut sp = shlex::split(s)
        .ok_or_else(|| Error::invalid_params("unable to parse input"))?
        .into_iter();
    let method = sp
        .next()
        .ok_or_else(|| Error::invalid_params("no method specified"))?;
    // parse the first param
    let mut params = BTreeMap::new();
    if let Some(p) = sp.next() {
        parse_param(p, &mut params, Some("i"))?;
    }
    for p in sp {
        parse_param(p, &mut params, None)?;
    }
    Ok((method, Value::Map(params)))
}
