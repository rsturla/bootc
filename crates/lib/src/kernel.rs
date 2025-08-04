//! Kernel command line parsing utilities.
//!
//! This module provides functionality for parsing and working with kernel command line
//! arguments, supporting both key-only switches and key-value pairs with proper quote handling.

use std::borrow::Cow;

use anyhow::Result;

/// This is used by dracut.
pub(crate) const INITRD_ARG_PREFIX: &[u8] = b"rd.";
/// The kernel argument for configuring the rootfs flags.
pub(crate) const ROOTFLAGS: &[u8] = b"rootflags";

/// A parsed kernel command line.
///
/// Wraps the raw command line bytes and provides methods for parsing and iterating
/// over individual parameters. Uses copy-on-write semantics to avoid unnecessary
/// allocations when working with borrowed data.
pub(crate) struct Cmdline<'a>(Cow<'a, [u8]>);

impl<'a, T: AsRef<[u8]> + ?Sized> From<&'a T> for Cmdline<'a> {
    /// Creates a new `Cmdline` from any type that can be referenced as bytes.
    ///
    /// Uses borrowed data when possible to avoid unnecessary allocations.
    fn from(input: &'a T) -> Self {
        Self(Cow::Borrowed(input.as_ref()))
    }
}

impl<'a> Cmdline<'a> {
    /// Reads the kernel command line from `/proc/cmdline`.
    ///
    /// Returns an error if the file cannot be read or if there are I/O issues.
    pub fn from_proc() -> Result<Self> {
        Ok(Self(Cow::Owned(std::fs::read("/proc/cmdline")?)))
    }

    /// Returns an iterator over all parameters in the command line.
    ///
    /// Properly handles quoted values containing whitespace and splits on
    /// unquoted whitespace characters. Parameters are parsed as either
    /// key-only switches or key=value pairs.
    pub fn iter(&'a self) -> impl Iterator<Item = Parameter<'a>> {
        let mut in_quotes = false;

        self.0
            .split(move |c| {
                if *c == b'"' {
                    in_quotes = !in_quotes;
                }
                !in_quotes && c.is_ascii_whitespace()
            })
            .map(Parameter::from)
    }

    /// Locate a kernel argument with the given key name.
    ///
    /// Returns the first parameter matching the given key, or `None` if not found.
    /// Key comparison treats dashes and underscores as equivalent.
    pub fn find(&'a self, key: impl AsRef<[u8]>) -> Option<Parameter<'a>> {
        let key = ParameterKey(key.as_ref());
        self.iter().find(|p| p.key == key)
    }

    /// Locate the value of the kernel argument with the given key name.
    ///
    /// Returns the first value matching the given key, or `None` if not found.
    /// Key comparison treats dashes and underscores as equivalent.
    pub fn value_of(&'a self, key: impl AsRef<[u8]>) -> Option<&'a [u8]> {
        self.find(key).and_then(|p| p.value)
    }

    /// Locate the UTF-8 value of the kernel argument with the given key name.
    ///
    /// Returns the first value matching the given key, or `None` if not found.
    /// Key comparison treats dashes and underscores as equivalent.
    pub fn value_of_utf8(&'a self, key: &str) -> Result<Option<&'a str>, std::str::Utf8Error> {
        self.value_of(key).map(std::str::from_utf8).transpose()
    }

    /// Find the value of the kernel argument with the provided name, which must be present.
    ///
    /// Otherwise the same as [`Self::value_of`].
    #[cfg(test)]
    pub fn require_value_of(&'a self, key: impl AsRef<[u8]>) -> Result<&'a [u8]> {
        let key = key.as_ref();
        self.value_of(key).ok_or_else(|| {
            let key = String::from_utf8_lossy(key);
            anyhow::anyhow!("Failed to find kernel argument '{key}'")
        })
    }

    /// Find the value of the kernel argument with the provided name, which must be present.
    ///
    /// Otherwise the same as [`Self::value_of`].
    #[cfg(test)]
    pub fn require_value_of_utf8(&'a self, key: &str) -> Result<&'a str> {
        self.value_of_utf8(key)?
            .ok_or_else(|| anyhow::anyhow!("Failed to find kernel argument '{key}'"))
    }
}

/// A single kernel command line parameter key
///
/// Handles quoted values and treats dashes and underscores in keys as equivalent.
#[derive(Debug, Eq)]
pub(crate) struct ParameterKey<'a>(&'a [u8]);

impl<'a> std::ops::Deref for ParameterKey<'a> {
    type Target = [u8];

    fn deref(&self) -> &'a Self::Target {
        self.0
    }
}

impl<'a> From<&'a [u8]> for ParameterKey<'a> {
    fn from(value: &'a [u8]) -> Self {
        Self(value)
    }
}

/// A single kernel command line parameter.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Parameter<'a> {
    /// The parameter key as raw bytes
    pub key: ParameterKey<'a>,
    /// The parameter value as raw bytes, if present
    pub value: Option<&'a [u8]>,
}

impl<'a> Parameter<'a> {
    /// Create a new parameter with the provided key and value.
    #[cfg(test)]
    pub fn new_kv<'k: 'a, 'v: 'a>(key: &'k [u8], value: &'v [u8]) -> Self {
        Self {
            key: ParameterKey(key),
            value: Some(value),
        }
    }

    /// Create a new parameter with the provided key.
    #[cfg(test)]
    pub fn new_key(key: &'a [u8]) -> Self {
        Self {
            key: ParameterKey(key),
            value: None,
        }
    }

    /// Returns the key as a lossy UTF-8 string.
    ///
    /// Invalid UTF-8 sequences are replaced with the Unicode replacement character.
    pub fn key_lossy(&self) -> String {
        String::from_utf8_lossy(&self.key).to_string()
    }

    /// Returns the value as a lossy UTF-8 string.
    ///
    /// Invalid UTF-8 sequences are replaced with the Unicode replacement character.
    /// Returns an empty string if no value is present.
    pub fn value_lossy(&self) -> String {
        String::from_utf8_lossy(self.value.unwrap_or(&[])).to_string()
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> From<&'a T> for Parameter<'a> {
    /// Parses a parameter from raw bytes.
    ///
    /// Splits on the first `=` character to separate key and value.
    /// Strips only the outermost pair of double quotes from values.
    /// If no `=` is found, treats the entire input as a key-only parameter.
    fn from(input: &'a T) -> Self {
        let input = input.as_ref();
        let equals = input.iter().position(|b| *b == b'=');

        match equals {
            None => Self {
                key: ParameterKey(input),
                value: None,
            },
            Some(i) => {
                let (key, mut value) = input.split_at(i);
                let key = ParameterKey(key);

                // skip `=`, we know it's the first byte because we
                // found it above
                value = &value[1..];

                // *Only* the first and last double quotes are stripped
                value = value
                    .strip_prefix(b"\"")
                    .unwrap_or(value)
                    .strip_suffix(b"\"")
                    .unwrap_or(value);

                Self {
                    key,
                    value: Some(value),
                }
            }
        }
    }
}

impl PartialEq for ParameterKey<'_> {
    /// Compares two parameter keys for equality.
    ///
    /// Keys are compared with dashes and underscores treated as equivalent.
    /// This comparison is case-sensitive.
    fn eq(&self, other: &Self) -> bool {
        let dedashed = |&c: &u8| {
            if c == b'-' {
                b'_'
            } else {
                c
            }
        };

        // We can't just zip() because leading substrings will match
        //
        // For example, "foo" == "foobar" since the zipped iterator
        // only compares the first three chars.
        let our_iter = self.0.iter().map(dedashed);
        let other_iter = other.0.iter().map(dedashed);
        our_iter.eq(other_iter)
    }
}

impl std::fmt::Display for Parameter<'_> {
    /// Formats the parameter for display.
    ///
    /// Key-only parameters are displayed as just the key.
    /// Key-value parameters are displayed as `key=value`.
    /// Values containing whitespace are automatically quoted.
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let key = self.key_lossy();

        if self.value.is_some() {
            let value = self.value_lossy();

            if value.chars().any(|c| c.is_ascii_whitespace()) {
                write!(f, "{key}=\"{value}\"")
            } else {
                write!(f, "{key}={value}")
            }
        } else {
            write!(f, "{key}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parameter_simple() {
        let switch = Parameter::from("foo");
        assert_eq!(switch.key.0, b"foo");
        assert_eq!(switch.value, None);

        let kv = Parameter::from("bar=baz");
        assert_eq!(kv.key.0, b"bar");
        assert_eq!(kv.value, Some(b"baz".as_slice()));
    }

    #[test]
    fn test_parameter_quoted() {
        let p = Parameter::from("foo=\"quoted value\"");
        assert_eq!(p.value, Some(b"quoted value".as_slice()));
    }

    #[test]
    fn test_parameter_pathological() {
        // valid things that certified insane people would do

        // quotes don't get removed from keys
        let p = Parameter::from("\"\"\"");
        assert_eq!(p.key.0, b"\"\"\"");

        // quotes only get stripped from the absolute ends of values
        let p = Parameter::from("foo=\"internal \" quotes \" are ok\"");
        assert_eq!(p.value, Some(b"internal \" quotes \" are ok".as_slice()));

        // non-UTF8 things are in fact valid
        let non_utf8_byte = b"\xff";
        #[allow(invalid_from_utf8)]
        let failed_conversion = str::from_utf8(non_utf8_byte);
        assert!(failed_conversion.is_err());
        let mut p = b"foo=".to_vec();
        p.push(non_utf8_byte[0]);
        let p = Parameter::from(&p);
        assert_eq!(p.value, Some(non_utf8_byte.as_slice()));

        // lossy replacement sanity check
        assert_eq!(p.value_lossy(), char::REPLACEMENT_CHARACTER.to_string());
    }

    #[test]
    fn test_parameter_equality() {
        // substrings are not equal
        let foo = Parameter::from("foo");
        let bar = Parameter::from("foobar");
        assert_ne!(foo, bar);
        assert_ne!(bar, foo);

        // dashes and underscores are treated equally
        let dashes = Parameter::from("a-delimited-param");
        let underscores = Parameter::from("a_delimited_param");
        assert_eq!(dashes, underscores);

        // same key, same values is equal
        let dashes = Parameter::from("a-delimited-param=same_values");
        let underscores = Parameter::from("a_delimited_param=same_values");
        assert_eq!(dashes, underscores);

        // same key, different values is not equal
        let dashes = Parameter::from("a-delimited-param=different_values");
        let underscores = Parameter::from("a_delimited_param=DiFfErEnT_valUEZ");
        assert_ne!(dashes, underscores);

        // mixed variants are never equal
        let switch = Parameter::from("same_key");
        let keyvalue = Parameter::from("same_key=but_with_a_value");
        assert_ne!(switch, keyvalue);
    }

    #[test]
    fn test_kargs_simple() {
        // example taken lovingly from:
        // https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/kernel/params.c?id=89748acdf226fd1a8775ff6fa2703f8412b286c8#n160
        let kargs = Cmdline::from(b"foo=bar,bar2 baz=fuz wiz".as_slice());
        let mut iter = kargs.iter();

        assert_eq!(iter.next(), Some(Parameter::new_kv(b"foo", b"bar,bar2")));

        assert_eq!(
            iter.next(),
            Some(Parameter::new_kv(b"baz", b"fuz".as_slice()))
        );

        assert_eq!(iter.next(), Some(Parameter::new_key(b"wiz")));
        assert_eq!(iter.next(), None);

        // Test the find API
        assert_eq!(kargs.find("foo").unwrap().value.unwrap(), b"bar,bar2");
        assert!(kargs.find("nothing").is_none());
    }

    #[test]
    fn test_kargs_from_proc() {
        let kargs = Cmdline::from_proc().unwrap();

        // Not really a good way to test this other than assume
        // there's at least one argument in /proc/cmdline wherever the
        // tests are running
        assert!(kargs.iter().count() > 0);
    }

    #[test]
    fn test_kargs_find_dash_hyphen() {
        let kargs = Cmdline::from(b"a-b=1 a_b=2".as_slice());
        // find should find the first one, which is a-b=1
        let p = kargs.find("a_b").unwrap();
        assert_eq!(p.key.0, b"a-b");
        assert_eq!(p.value.unwrap(), b"1");
        let p = kargs.find("a-b").unwrap();
        assert_eq!(p.key.0, b"a-b");
        assert_eq!(p.value.unwrap(), b"1");

        let kargs = Cmdline::from(b"a_b=2 a-b=1".as_slice());
        // find should find the first one, which is a_b=2
        let p = kargs.find("a_b").unwrap();
        assert_eq!(p.key.0, b"a_b");
        assert_eq!(p.value.unwrap(), b"2");
        let p = kargs.find("a-b").unwrap();
        assert_eq!(p.key.0, b"a_b");
        assert_eq!(p.value.unwrap(), b"2");
    }

    #[test]
    fn test_value_of() {
        let kargs = Cmdline::from(b"foo=bar baz=qux switch".as_slice());

        // Test existing key with value
        assert_eq!(kargs.value_of("foo"), Some(b"bar".as_slice()));
        assert_eq!(kargs.value_of("baz"), Some(b"qux".as_slice()));

        // Test key without value
        assert_eq!(kargs.value_of("switch"), None);

        // Test non-existent key
        assert_eq!(kargs.value_of("missing"), None);

        // Test dash/underscore equivalence
        let kargs = Cmdline::from(b"dash-key=value1 under_key=value2".as_slice());
        assert_eq!(kargs.value_of("dash_key"), Some(b"value1".as_slice()));
        assert_eq!(kargs.value_of("under-key"), Some(b"value2".as_slice()));
    }

    #[test]
    fn test_value_of_utf8() {
        let kargs = Cmdline::from(b"foo=bar baz=qux switch".as_slice());

        // Test existing key with UTF-8 value
        assert_eq!(kargs.value_of_utf8("foo").unwrap(), Some("bar"));
        assert_eq!(kargs.value_of_utf8("baz").unwrap(), Some("qux"));

        // Test key without value
        assert_eq!(kargs.value_of_utf8("switch").unwrap(), None);

        // Test non-existent key
        assert_eq!(kargs.value_of_utf8("missing").unwrap(), None);

        // Test dash/underscore equivalence
        let kargs = Cmdline::from(b"dash-key=value1 under_key=value2".as_slice());
        assert_eq!(kargs.value_of_utf8("dash_key").unwrap(), Some("value1"));
        assert_eq!(kargs.value_of_utf8("under-key").unwrap(), Some("value2"));

        // Test invalid UTF-8
        let mut invalid_utf8 = b"invalid=".to_vec();
        invalid_utf8.push(0xff);
        let kargs = Cmdline::from(&invalid_utf8);
        assert!(kargs.value_of_utf8("invalid").is_err());
    }

    #[test]
    fn test_require_value_of() {
        let kargs = Cmdline::from(b"foo=bar baz=qux switch".as_slice());

        // Test existing key with value
        assert_eq!(kargs.require_value_of("foo").unwrap(), b"bar");
        assert_eq!(kargs.require_value_of("baz").unwrap(), b"qux");

        // Test key without value should fail
        let err = kargs.require_value_of("switch").unwrap_err();
        assert!(err
            .to_string()
            .contains("Failed to find kernel argument 'switch'"));

        // Test non-existent key should fail
        let err = kargs.require_value_of("missing").unwrap_err();
        assert!(err
            .to_string()
            .contains("Failed to find kernel argument 'missing'"));

        // Test dash/underscore equivalence
        let kargs = Cmdline::from(b"dash-key=value1 under_key=value2".as_slice());
        assert_eq!(kargs.require_value_of("dash_key").unwrap(), b"value1");
        assert_eq!(kargs.require_value_of("under-key").unwrap(), b"value2");
    }

    #[test]
    fn test_require_value_of_utf8() {
        let kargs = Cmdline::from(b"foo=bar baz=qux switch".as_slice());

        // Test existing key with UTF-8 value
        assert_eq!(kargs.require_value_of_utf8("foo").unwrap(), "bar");
        assert_eq!(kargs.require_value_of_utf8("baz").unwrap(), "qux");

        // Test key without value should fail
        let err = kargs.require_value_of_utf8("switch").unwrap_err();
        assert!(err
            .to_string()
            .contains("Failed to find kernel argument 'switch'"));

        // Test non-existent key should fail
        let err = kargs.require_value_of_utf8("missing").unwrap_err();
        assert!(err
            .to_string()
            .contains("Failed to find kernel argument 'missing'"));

        // Test dash/underscore equivalence
        let kargs = Cmdline::from(b"dash-key=value1 under_key=value2".as_slice());
        assert_eq!(kargs.require_value_of_utf8("dash_key").unwrap(), "value1");
        assert_eq!(kargs.require_value_of_utf8("under-key").unwrap(), "value2");

        // Test invalid UTF-8 should fail
        let mut invalid_utf8 = b"invalid=".to_vec();
        invalid_utf8.push(0xff);
        let kargs = Cmdline::from(&invalid_utf8);
        assert!(kargs.require_value_of_utf8("invalid").is_err());
    }
}
