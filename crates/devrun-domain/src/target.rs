use crate::DomainError;
use crate::identifier::validate_identifier;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetRef {
    Name(String),
    Profile(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetOp {
    Include(TargetRef),
    Exclude(TargetRef),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetExpr(Vec<TargetOp>);

impl TargetExpr {
    pub fn parse<I>(tokens: I) -> Result<Self, DomainError>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let mut ops = Vec::new();
        for token in tokens {
            for part in token.as_ref().split(',').map(str::trim) {
                if !part.is_empty() {
                    ops.push(parse_op(part)?);
                }
            }
        }
        Ok(Self(ops))
    }

    pub fn including<I>(names: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        Self(
            names
                .into_iter()
                .map(|name| TargetOp::Include(TargetRef::Name(name.into())))
                .collect(),
        )
    }

    pub fn ops(&self) -> &[TargetOp] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn parse_op(part: &str) -> Result<TargetOp, DomainError> {
    let invalid = || DomainError::InvalidTarget(part.to_owned());
    let (exclude, body) = match part.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, part.strip_prefix('+').unwrap_or(part)),
    };
    let target = match body.strip_prefix('@') {
        Some(profile) => {
            TargetRef::Profile(validate_identifier("profile", profile).map_err(|_| invalid())?)
        }
        None => TargetRef::Name(validate_identifier("target", body).map_err(|_| invalid())?),
    };
    Ok(if exclude {
        TargetOp::Exclude(target)
    } else {
        TargetOp::Include(target)
    })
}
