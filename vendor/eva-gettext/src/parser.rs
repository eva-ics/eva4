use std::borrow::Cow;
use std::default::Default;
use std::error;
use std::fmt;
use std::io;

use byteorder::{BigEndian, ByteOrder, LittleEndian};
use encoding_rs::{Encoding, UTF_8};

use crate::metadata::parse_metadata;
use crate::plurals::{Ast, Resolver};
use crate::{Catalog, Message};

fn decode(encoding: &'static Encoding, input: &[u8]) -> Result<String, Error> {
    encoding
        .decode_without_bom_handling_and_without_replacement(input)
        .map(Cow::into_owned)
        .ok_or(DecodingError)
}

/// Represents an error encountered while parsing an MO file.
#[derive(Debug)]
pub enum Error {
    /// An incorrect magic number has been encountered
    BadMagic,
    /// An invalid byte sequence for the given encoding has been encountered
    DecodingError,
    /// An unexpected EOF occured
    Eof,
    /// An I/O error occured
    Io(io::Error),
    /// Incorrect syntax encountered while parsing the meta information
    MalformedMetadata,
    /// Meta information string was not the first string in the catalog
    MisplacedMetadata,
    /// Invalid Plural-Forms metadata
    PluralParsing,
    /// An unknown encoding was specified in the metadata
    UnknownEncoding,
}
use crate::Error::*;

impl error::Error for Error {
    fn description(&self) -> &str {
        match *self {
            BadMagic => "bad magic number",
            DecodingError => "invalid byte sequence in a string",
            Eof => "unxpected end of file",
            Io(ref err) => err.description(),
            MalformedMetadata => "metadata syntax error",
            MisplacedMetadata => "misplaced metadata",
            UnknownEncoding => "unknown encoding specified",
            PluralParsing => "invalid plural expression",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let self_err: &error::Error = self;
        write!(fmt, "{}", self_err.description())
    }
}

impl From<io::Error> for Error {
    fn from(inner: io::Error) -> Error {
        Io(inner)
    }
}

/// ParseOptions allows setting options for parsing MO catalogs.
///
/// # Examples
/// ```ignore
/// use std::fs::File;
/// use encoding_rs::WINDOWS_1252;
///
/// let file = File::open("french.mo").unwrap();
/// let catalog = ParseOptions::new().force_encoding(WINDOWS_1252).parse(file).unwrap();
/// ```
#[allow(missing_debug_implementations)]
#[derive(Default)]
pub struct ParseOptions {
    force_encoding: Option<&'static Encoding>,
    force_plural: Option<fn(u64) -> usize>,
}

impl ParseOptions {
    /// Returns a new instance of ParseOptions with default options.
    pub fn new() -> Self {
        Default::default()
    }

    /// Tries to parse the catalog from the given reader using the specified options.
    pub fn parse<R: io::Read>(self, reader: R) -> Result<Catalog, Error> {
        parse_catalog(reader, self)
    }

    /// Forces a use of a specific encoding
    /// when parsing strings from a catalog.
    /// If this option is not enabled,
    /// the parser tries to use the encoding specified in the metadata
    /// or UTF-8 if metadata is non-existent.
    pub fn force_encoding(mut self, encoding: &'static Encoding) -> Self {
        self.force_encoding = Some(encoding);
        self
    }

    /// Forces a use of the given plural formula
    /// for deciding the proper plural form for a message.
    /// If this option is not enabled,
    /// the parser tries to use the plural formula specified in the metadata
    /// or `n != 1` if metadata is non-existent.
    pub fn force_plural(mut self, plural: fn(u64) -> usize) -> Self {
        self.force_plural = Some(plural);
        self
    }
}

/// According to the given magic number of a MO file,
/// returns the function which reads a `u32` in the relevant endianness.
fn get_read_u32_fn(magic: &[u8]) -> Option<fn(&[u8]) -> u32> {
    if magic == [0xde, 0x12, 0x04, 0x95] {
        Some(LittleEndian::read_u32)
    } else if magic == [0x95, 0x04, 0x12, 0xde] {
        Some(BigEndian::read_u32)
    } else {
        None
    }
}

pub fn parse_catalog<'a, R: io::Read>(mut file: R, opts: ParseOptions) -> Result<Catalog, Error> {
    let mut contents = vec![];
    let n = file.read_to_end(&mut contents)?;
    if n < 28 {
        return Err(Eof);
    }

    let read_u32 = get_read_u32_fn(&contents[0..4]).ok_or(BadMagic)?;

    // ignore hashing tables (bytes at 20..28)
    let num_strings = read_u32(&contents[8..12]) as usize;
    let mut off_otable = read_u32(&contents[12..16]) as usize;
    let mut off_ttable = read_u32(&contents[16..20]) as usize;
    if n < off_otable || n < off_ttable {
        return Err(Eof);
    }

    let mut catalog = Catalog::new();
    if let Some(f) = opts.force_plural {
        catalog.resolver = Resolver::Function(f);
    }
    let mut encoding = opts.force_encoding.unwrap_or(UTF_8);

    for i in 0..num_strings {
        // Parse the original string
        if n < off_otable + 8 {
            return Err(Eof);
        }
        let len = read_u32(&contents[off_otable..off_otable + 4]) as usize;
        let off = read_u32(&contents[off_otable + 4..off_otable + 8]) as usize;
        // +1 compensates for the ending NUL byte which is not included in length
        if n < off + len + 1 {
            return Err(Eof);
        }
        let mut original = &contents[off..off + len + 1];
        // check for context
        let context = match original.iter().position(|x| *x == 4) {
            Some(idx) => {
                let ctx = &original[..idx];
                original = &original[idx + 1..];
                Some(decode(encoding, ctx)?)
            }
            None => None,
        };
        // extract msg_id singular, ignoring the plural
        let id = match original
            .iter()
            .position(|x| *x == 0)
            .map(|i| &original[..i])
        {
            Some(b) => decode(encoding, b)?,
            None => return Err(Eof),
        };
        if id == "" && i != 0 {
            return Err(MisplacedMetadata);
        }

        // Parse the translation strings
        if n < off_ttable + 8 {
            return Err(Eof);
        }
        let len = read_u32(&contents[off_ttable..off_ttable + 4]) as usize;
        let off = read_u32(&contents[off_ttable + 4..off_ttable + 8]) as usize;
        // +1 compensates for the ending NUL byte which is not included in length
        if n < off + len + 1 {
            return Err(Eof);
        }
        let translated = contents[off..off + len]
            .split(|x| *x == 0)
            .map(|b| decode(encoding, b))
            .collect::<Result<Vec<_>, _>>()?;
        if id == "" {
            let map = parse_metadata(&*translated[0])?;
            if let (Some(c), None) = (map.charset(), opts.force_encoding) {
                encoding = Encoding::for_label(c.as_bytes()).ok_or(UnknownEncoding)?;
            }
            if opts.force_plural.is_none() {
                if let Some(p) = map.plural_forms().1 {
                    catalog.resolver = Ast::parse(p).map(Resolver::Expr)?;
                }
            }
        }

        catalog.insert(Message::new(id, context, translated));

        off_otable += 8;
        off_ttable += 8;
    }

    Ok(catalog)
}

/// The default plural resolver.
///
/// It will be used if not `Plural-Forms` header is found in the .mo file, and if
/// `ParseOptions::force_plural` was not called.
///
/// It is valid for English and similar languages: plural will be used for any quantity
/// different of 1.
pub fn default_resolver(n: u64) -> usize {
    if n == 1 {
        0
    } else {
        1
    }
}

#[test]
fn test_get_read_u32_fn() {
    use std::mem;

    assert!(get_read_u32_fn(&[]).is_none());
    assert!(get_read_u32_fn(&[0xde, 0x12, 0x04, 0x95, 0x00]).is_none());

    {
        let le_ptr: *const ();
        let ret_ptr;
        unsafe {
            le_ptr = mem::transmute(LittleEndian::read_u32 as usize);
            ret_ptr = mem::transmute(get_read_u32_fn(&[0xde, 0x12, 0x04, 0x95]).unwrap());
        }
        assert_eq!(le_ptr, ret_ptr);
    }

    {
        let be_ptr: *const ();
        let ret_ptr;
        unsafe {
            be_ptr = mem::transmute(BigEndian::read_u32 as usize);
            ret_ptr = mem::transmute(get_read_u32_fn(&[0x95, 0x04, 0x12, 0xde]).unwrap());
        }
        assert_eq!(be_ptr, ret_ptr);
    }
}

#[test]
fn test_parse_catalog() {
    macro_rules! assert_variant {
        ($value:expr, $variant:path) => {
            match $value {
                $variant => (),
                _ => panic!("Expected {:?}, got {:?}", $variant, $value),
            }
        };
    }

    let fluff = [0; 24]; // zeros to pad our magic test cases to satisfy the length requirements

    {
        let mut reader = vec![1u8, 2, 3];
        reader.extend(fluff.iter().cloned());
        let err = parse_catalog(&reader[..], ParseOptions::new()).unwrap_err();
        assert_variant!(err, Eof);
    }

    {
        let mut reader = vec![1u8, 2, 3, 4];
        reader.extend(fluff.iter().cloned());
        let err = parse_catalog(&reader[..], ParseOptions::new()).unwrap_err();
        assert_variant!(err, BadMagic);
    }

    {
        let mut reader = vec![0x95, 0x04, 0x12, 0xde];
        reader.extend(fluff.iter().cloned());
        assert!(parse_catalog(&reader[..], ParseOptions::new()).is_ok());
    }

    {
        let mut reader = vec![0xde, 0x12, 0x04, 0x95];
        reader.extend(fluff.iter().cloned());
        assert!(parse_catalog(&reader[..], ParseOptions::new()).is_ok());
    }

    {
        let reader: &[u8] = include_bytes!("../test_cases/1.mo");
        let catalog = parse_catalog(reader, ParseOptions::new()).unwrap();
        assert_eq!(catalog.strings.len(), 1);
        assert_eq!(
            catalog.strings["this is context\x04Text"],
            Message::new("Text", Some("this is context"), vec!["Tekstas", "Tekstai"])
        );
    }

    {
        let reader: &[u8] = include_bytes!("../test_cases/2.mo");
        let catalog = parse_catalog(reader, ParseOptions::new()).unwrap();
        assert_eq!(catalog.strings.len(), 2);
        assert_eq!(
            catalog.strings["Image"],
            Message::new("Image", None, vec!["Nuotrauka", "Nuotraukos"])
        );
    }

    {
        let reader: &[u8] = include_bytes!("../test_cases/invalid_utf8.mo");
        let err = parse_catalog(reader, ParseOptions::new()).unwrap_err();
        assert_variant!(err, DecodingError);
    }
}
