use std::borrow::Cow;
use std::fmt;

const SAFE_PUNCTUATION: &str = "_./:@^=+,-";
const UNQUOTABLE: [char; 3] = ['\0', '\n', '\r'];

#[derive(Debug)]
pub struct UnquotableToken(String);

impl fmt::Display for UnquotableToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot quote {:?}: contains a newline or NUL character",
            self.0
        )
    }
}

pub fn join<I, S>(words: I) -> Result<String, UnquotableToken>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let quoted: Vec<String> = words
        .into_iter()
        .map(|word| quote(word.as_ref()).map(Cow::into_owned))
        .collect::<Result<_, _>>()?;
    Ok(quoted.join(" "))
}

pub fn quote(word: &str) -> Result<Cow<'_, str>, UnquotableToken> {
    if word.contains(UNQUOTABLE) {
        Err(UnquotableToken(word.to_owned()))
    } else if !word.is_empty() && word.chars().all(is_safe) {
        Ok(Cow::Borrowed(word))
    } else {
        Ok(Cow::Owned(format!("'{}'", word.replace('\'', r"'\''"))))
    }
}

fn is_safe(c: char) -> bool {
    c.is_ascii_alphanumeric() || SAFE_PUNCTUATION.contains(c)
}
