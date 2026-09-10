use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use super::Error;
use crate::Error::MalformedMetadata;

#[derive(Debug)]
pub struct MetadataMap<'a>(HashMap<&'a str, &'a str>);

impl<'a> MetadataMap<'a> {
    /// Returns a string that indicates the character set.
    pub fn charset(&self) -> Option<&'a str> {
        self.get("Content-Type")
            .and_then(|x| x.split("charset=").skip(1).next())
    }

    /// Returns the number of different plurals and the boolean
    /// expression to determine the form to use depending on
    /// the number of elements.
    ///
    /// Defaults to `n_plurals = 2` and `plural = n!=1` (as in English).
    pub fn plural_forms(&self) -> (Option<usize>, Option<&'a str>) {
        self.get("Plural-Forms")
            .map(|f| {
                f.split(';').fold((None, None), |(n_pl, pl), prop| {
                    match prop.chars().position(|c| c == '=') {
                        Some(index) => {
                            let (name, value) = prop.split_at(index);
                            let value = value[1..value.len()].trim();
                            match name.trim() {
                                "n_plurals" => (usize::from_str_radix(value, 10).ok(), pl),
                                "plural" => (n_pl, Some(value)),
                                _ => (n_pl, pl),
                            }
                        }
                        None => (n_pl, pl),
                    }
                })
            }).unwrap_or((None, None))
    }
}

impl<'a> Deref for MetadataMap<'a> {
    type Target = HashMap<&'a str, &'a str>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> DerefMut for MetadataMap<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub fn parse_metadata(blob: &str) -> Result<MetadataMap, Error> {
    let mut map = MetadataMap(HashMap::new());
    for line in blob.split('\n').filter(|s| s != &"") {
        let pos = match line.bytes().position(|b| b == b':') {
            Some(p) => p,
            None => return Err(MalformedMetadata),
        };
        map.insert(line[..pos].trim(), line[pos + 1..].trim());
    }
    Ok(map)
}

#[test]
fn test_metadatamap_charset() {
    {
        let mut map = MetadataMap(HashMap::new());
        assert!(map.charset().is_none());
        map.insert("Content-Type", "");
        assert!(map.charset().is_none());
        map.insert("Content-Type", "abc");
        assert!(map.charset().is_none());
        map.insert("Content-Type", "text/plain; charset=utf-42");
        assert_eq!(map.charset().unwrap(), "utf-42");
    }
}

#[test]
fn test_metadatamap_plural() {
    {
        let mut map = MetadataMap(HashMap::new());
        assert_eq!(map.plural_forms(), (None, None));

        map.insert("Plural-Forms", "");
        assert_eq!(map.plural_forms(), (None, None));
        // n_plural
        map.insert("Plural-Forms", "n_plurals=42");
        assert_eq!(map.plural_forms(), (Some(42), None));
        // plural is specified
        map.insert("Plural-Forms", "n_plurals=2; plural=n==12");
        assert_eq!(map.plural_forms(), (Some(2), Some("n==12")));
        // plural before n_plurals
        map.insert("Plural-Forms", "plural=n==12; n_plurals=2");
        assert_eq!(map.plural_forms(), (Some(2), Some("n==12")));
        // with spaces
        map.insert("Plural-Forms", " n_plurals = 42 ; plural = n >  10   ");
        assert_eq!(map.plural_forms(), (Some(42), Some("n >  10")));
    }
}
