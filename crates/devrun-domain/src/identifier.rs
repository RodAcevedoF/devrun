use std::fmt;
use std::str::FromStr;

use crate::DomainError;

const MAX_LEN: usize = 64;

fn is_lead_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

fn is_body_char(c: char) -> bool {
    is_lead_char(c) || matches!(c, '-' | '_' | '.')
}

pub(crate) fn validate_identifier(kind: &'static str, raw: &str) -> Result<String, DomainError> {
    let mut chars = raw.chars();
    let valid =
        raw.len() <= MAX_LEN && chars.next().is_some_and(is_lead_char) && chars.all(is_body_char);
    if valid {
        Ok(raw.to_owned())
    } else {
        Err(DomainError::InvalidName {
            kind,
            value: raw.to_owned(),
        })
    }
}

fn slugify(raw: &str) -> String {
    let mut slug = String::with_capacity(raw.len());
    for c in raw.chars().map(|c| c.to_ascii_lowercase()) {
        let c = if is_body_char(c) { c } else { '-' };
        if !(c == '-' && slug.ends_with('-')) {
            slug.push(c);
        }
    }
    slug.trim_matches(|c: char| !is_lead_char(c))
        .chars()
        .take(MAX_LEN)
        .collect()
}

macro_rules! identifier {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn parse(raw: &str) -> Result<Self, DomainError> {
                validate_identifier($kind, raw).map(Self)
            }

            pub fn slug(raw: &str) -> Result<Self, DomainError> {
                Self::parse(&slugify(raw)).map_err(|_| DomainError::InvalidName {
                    kind: $kind,
                    value: raw.to_owned(),
                })
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(raw: &str) -> Result<Self, Self::Err> {
                Self::parse(raw)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

identifier!(ServiceName, "service");
identifier!(AliasName, "alias");
identifier!(ProfileName, "profile");
