use std::borrow::Cow;

use anyhow::Result;

/// This is used by dracut.
pub(crate) const INITRD_ARG_PREFIX: &[u8] = b"rd.";
/// The kernel argument for configuring the rootfs flags.
pub(crate) const ROOTFLAGS: &[u8] = b"rootflags";

pub(crate) struct Cmdline<'a>(Cow<'a, [u8]>);

impl<'a, T: AsRef<[u8]> + ?Sized> From<&'a T> for Cmdline<'a> {
    fn from(input: &'a T) -> Self {
        Self(Cow::Borrowed(input.as_ref()))
    }
}

impl<'a> Cmdline<'a> {
    pub fn from_proc() -> Result<Self> {
        Ok(Self(Cow::Owned(std::fs::read("/proc/cmdline")?)))
    }

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
}

#[derive(Debug, Eq)]
pub(crate) struct Parameter<'a> {
    pub key: &'a [u8],
    pub value: Option<&'a [u8]>,
}

impl<'a> Parameter<'a> {
    pub fn key_lossy(&self) -> String {
        String::from_utf8_lossy(self.key).to_string()
    }

    pub fn value_lossy(&self) -> String {
        String::from_utf8_lossy(self.value.unwrap_or(&[])).to_string()
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> From<&'a T> for Parameter<'a> {
    fn from(input: &'a T) -> Self {
        let input = input.as_ref();
        let equals = input.iter().position(|b| *b == b'=');

        match equals {
            None => Self {
                key: input,
                value: None,
            },
            Some(i) => {
                let (key, mut value) = input.split_at(i);

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

impl PartialEq for Parameter<'_> {
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
        let our_iter = self.key.iter().map(dedashed);
        let other_iter = other.key.iter().map(dedashed);
        if !our_iter.eq(other_iter) {
            return false;
        }

        match (self.value, other.value) {
            (Some(ours), Some(other)) => ours == other,
            (None, None) => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for Parameter<'_> {
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
        assert_eq!(switch.key, b"foo");
        assert_eq!(switch.value, None);

        let kv = Parameter::from("bar=baz");
        assert_eq!(kv.key, b"bar");
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
        assert_eq!(p.key, b"\"\"\"");

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

        assert_eq!(
            iter.next(),
            Some(Parameter {
                key: b"foo",
                value: Some(b"bar,bar2".as_slice())
            })
        );

        assert_eq!(
            iter.next(),
            Some(Parameter {
                key: b"baz",
                value: Some(b"fuz".as_slice())
            })
        );

        assert_eq!(
            iter.next(),
            Some(Parameter {
                key: b"wiz",
                value: None,
            })
        );

        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_kargs_from_proc() {
        let kargs = Cmdline::from_proc().unwrap();

        // Not really a good way to test this other than assume
        // there's at least one argument in /proc/cmdline wherever the
        // tests are running
        assert!(kargs.iter().count() > 0);
    }
}
