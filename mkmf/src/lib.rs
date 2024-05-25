use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
};

pub trait MapKeysMatchFormula<K, V> {
    fn keys_match_formula(&self, formula: &str) -> Vec<&K>;
    fn values_match_key_formula(&self, formula: &str) -> Vec<&V>;
}

enum Formula {
    Eq(i64),
    Ne(i64),
    Gt(i64),
    Lt(i64),
    Ge(i64),
    Le(i64),
    Ri(i64, i64),
}

impl Formula {
    fn matches<S>(&self, value: S) -> bool
    where
        S: AsRef<str>,
    {
        let Ok(value) = value.as_ref().parse::<i64>() else {
            return matches!(self, Formula::Ne(_));
        };
        match self {
            Formula::Eq(f) => value == *f,
            Formula::Ne(f) => value != *f,
            Formula::Gt(f) => value > *f,
            Formula::Lt(f) => value < *f,
            Formula::Ge(f) => value >= *f,
            Formula::Le(f) => value <= *f,
            Formula::Ri(f1, f2) => value >= *f1 && value <= *f2,
        }
    }
}

impl FromStr for Formula {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('(');
        let kind = parts.next().ok_or(())?;
        let value = parts.next().ok_or(())?;
        let Some(value) = value.strip_suffix(')') else {
            return Err(());
        };
        macro_rules! parse_single {
            ($value:expr) => {
                $value.parse().map_err(|_| ())?
            };
        }
        match kind {
            "eq" => Ok(Formula::Eq(parse_single!(value))),
            "ne" => Ok(Formula::Ne(parse_single!(value))),
            "gt" => Ok(Formula::Gt(parse_single!(value))),
            "lt" => Ok(Formula::Lt(parse_single!(value))),
            "ge" => Ok(Formula::Ge(parse_single!(value))),
            "le" => Ok(Formula::Le(parse_single!(value))),
            "ri" => {
                let mut parts = value.split("..");
                let f1 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
                let f2 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
                Ok(Formula::Ri(f1, f2))
            }
            _ => Err(()),
        }
    }
}

impl<K: std::hash::Hash + Eq, V, S: ::std::hash::BuildHasher> MapKeysMatchFormula<K, V>
    for HashMap<K, V, S>
where
    K: AsRef<str>,
{
    fn keys_match_formula(&self, formula: &str) -> Vec<&K> {
        let keys = self.keys();
        keys_match_formula(keys, formula)
    }
    fn values_match_key_formula(&self, formula: &str) -> Vec<&V> {
        values_match_key_formula(self.iter(), formula)
    }
}

impl<K, V> MapKeysMatchFormula<K, V> for BTreeMap<K, V>
where
    K: AsRef<str>,
{
    fn keys_match_formula(&self, formula: &str) -> Vec<&K> {
        let keys = self.keys();
        keys_match_formula(keys, formula)
    }
    fn values_match_key_formula(&self, formula: &str) -> Vec<&V> {
        values_match_key_formula(self.iter(), formula)
    }
}

fn keys_match_formula<'a, K, I>(keys: I, formula: &str) -> Vec<&'a K>
where
    K: AsRef<str>,
    I: Iterator<Item = &'a K>,
{
    let mut result = Vec::new();
    let Ok(f) = formula.parse::<Formula>() else {
        return result;
    };
    for key in keys {
        if f.matches(key) {
            result.push(key);
        }
    }
    result
}

fn values_match_key_formula<'a, K, V, I>(iter: I, formula: &str) -> Vec<&'a V>
where
    K: AsRef<str> + 'a,
    I: Iterator<Item = (&'a K, &'a V)>,
{
    let mut result = Vec::new();
    let Ok(f) = formula.parse::<Formula>() else {
        return result;
    };
    for (key, value) in iter {
        if f.matches(key) {
            result.push(value);
        }
    }
    result
}

#[allow(clippy::zero_sized_map_values)]
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::MapKeysMatchFormula as _;

    #[test]
    fn test_keys_matches_formula_eq() {
        let mut h: BTreeMap<String, ()> = BTreeMap::new();
        h.insert("hello".to_string(), ());
        h.insert("world".to_string(), ());
        h.insert("1".to_string(), ());
        h.insert("2".to_string(), ());
        h.insert("3".to_string(), ());
        h.insert("4".to_string(), ());
        h.insert("5".to_string(), ());
        assert_eq!(h.keys_match_formula("eq(1)"), vec!["1"]);
    }
    #[test]
    fn test_keys_matches_formula_ne() {
        let mut h: BTreeMap<String, ()> = BTreeMap::new();
        h.insert("hello".to_string(), ());
        h.insert("world".to_string(), ());
        h.insert("1".to_string(), ());
        h.insert("2".to_string(), ());
        h.insert("3".to_string(), ());
        h.insert("4".to_string(), ());
        h.insert("5".to_string(), ());
        assert_eq!(
            h.keys_match_formula("ne(1)"),
            vec!["2", "3", "4", "5", "hello", "world"]
        );
    }
    #[test]
    fn test_keys_matches_formula_gt() {
        let mut h: BTreeMap<String, ()> = BTreeMap::new();
        h.insert("hello".to_string(), ());
        h.insert("world".to_string(), ());
        h.insert("1".to_string(), ());
        h.insert("2".to_string(), ());
        h.insert("3".to_string(), ());
        h.insert("4".to_string(), ());
        h.insert("5".to_string(), ());
        assert_eq!(h.keys_match_formula("gt(3)"), vec!["4", "5"]);
    }
    #[test]
    fn test_keys_matches_formula_lt() {
        let mut h: BTreeMap<String, ()> = BTreeMap::new();
        h.insert("hello".to_string(), ());
        h.insert("world".to_string(), ());
        h.insert("1".to_string(), ());
        h.insert("2".to_string(), ());
        h.insert("3".to_string(), ());
        h.insert("4".to_string(), ());
        h.insert("5".to_string(), ());
        assert_eq!(h.keys_match_formula("lt(3)"), vec!["1", "2"]);
    }
    #[test]
    fn test_keys_matches_formula_ge() {
        let mut h: BTreeMap<String, ()> = BTreeMap::new();
        h.insert("hello".to_string(), ());
        h.insert("world".to_string(), ());
        h.insert("1".to_string(), ());
        h.insert("2".to_string(), ());
        h.insert("3".to_string(), ());
        h.insert("4".to_string(), ());
        h.insert("5".to_string(), ());
        assert_eq!(h.keys_match_formula("ge(3)"), vec!["3", "4", "5"]);
    }
    #[test]
    fn test_keys_matches_formula_le() {
        let mut h: BTreeMap<String, ()> = BTreeMap::new();
        h.insert("hello".to_string(), ());
        h.insert("world".to_string(), ());
        h.insert("1".to_string(), ());
        h.insert("2".to_string(), ());
        h.insert("3".to_string(), ());
        h.insert("4".to_string(), ());
        h.insert("5".to_string(), ());
        assert_eq!(h.keys_match_formula("le(3)"), vec!["1", "2", "3"]);
    }
    #[test]
    fn test_keys_matches_formula_ri() {
        let mut h: BTreeMap<String, ()> = BTreeMap::new();
        h.insert("hello".to_string(), ());
        h.insert("world".to_string(), ());
        h.insert("1".to_string(), ());
        h.insert("2".to_string(), ());
        h.insert("3".to_string(), ());
        h.insert("4".to_string(), ());
        h.insert("5".to_string(), ());
        assert_eq!(h.keys_match_formula("ri(2..4)"), vec!["2", "3", "4"]);
    }
}
