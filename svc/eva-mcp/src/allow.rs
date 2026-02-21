use std::collections::HashMap;

use serde::de::{self, Deserialize, Deserializer, Visitor};

#[derive(Clone, Debug)]
pub enum Allow {
    All,
    Rules(AllowRules),
}

#[derive(Clone, Debug)]
pub(crate) struct AllowRules(HashMap<String, MethodsList>);

#[derive(Clone, Debug)]
pub(crate) struct MethodsList(Vec<String>);

impl Allow {
    pub fn allows(&self, target: &str, method: &str) -> bool {
        match self {
            Allow::All => true,
            Allow::Rules(r) => r.0.get(target).is_some_and(|methods| {
                methods.0.iter().any(|m| m == "*" || m == "#" || m == method)
            }),
        }
    }
}

impl<'de> Deserialize<'de> for MethodsList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MethodsVisitor;
        impl<'de> Visitor<'de> for MethodsVisitor {
            type Value = MethodsList;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(r##""*" or "#" or ["method", ...]"##)
            }

            fn visit_str<E>(self, v: &str) -> Result<MethodsList, E>
            where
                E: de::Error,
            {
                if v == "*" || v == "#" {
                    Ok(MethodsList(vec![v.to_string()]))
                } else {
                    Err(de::Error::custom(
                        "allow method value must be \"*\" or \"#\" (all methods) or a list",
                    ))
                }
            }

            fn visit_seq<A>(self, seq: A) -> Result<MethodsList, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let v = Vec::deserialize(de::value::SeqAccessDeserializer::new(seq))?;
                Ok(MethodsList(v))
            }
        }
        deserializer.deserialize_any(MethodsVisitor)
    }
}

impl<'de> Deserialize<'de> for Allow {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct AllowVisitor;
        impl<'de> Visitor<'de> for AllowVisitor {
            type Value = Allow;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(r##"allow: "*" or "#" or { "target": "*"|"#"|["method", ...], ... }"##)
            }

            fn visit_str<E>(self, v: &str) -> Result<Allow, E>
            where
                E: de::Error,
            {
                if v == "*" || v == "#" {
                    Ok(Allow::All)
                } else {
                    Err(de::Error::custom(
                        "allow string must be \"*\" or \"#\" (synonyms for allow all)",
                    ))
                }
            }

            fn visit_map<A>(self, map: A) -> Result<Allow, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let rules = HashMap::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(Allow::Rules(AllowRules(rules)))
            }
        }
        deserializer.deserialize_any(AllowVisitor)
    }
}
