use std::{borrow::Cow, path::{PathBuf}};
use fancy_regex::Regex;
use path_slash::PathBufExt;
use serde::{de::Error, Deserialize, Deserializer};

pub fn normalize_path<P: AsRef<str>>(original: P) -> String {
    let original_str = original.as_ref();

    let p = PathBuf::from(original_str);
    let mut str = clean_path::clean(p)
        .to_slash_lossy()
        .to_string();

    if original_str.ends_with('/') && !str.ends_with('/') {
        str.push('/');
    }

    str
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path(""), ".");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path("foo"), "foo");
        assert_eq!(normalize_path("foo/bar"), "foo/bar");
        assert_eq!(normalize_path("foo//bar"), "foo/bar");
        assert_eq!(normalize_path("foo/./bar"), "foo/bar");
        assert_eq!(normalize_path("foo/../bar"), "bar");
        assert_eq!(normalize_path("foo/bar/.."), "foo");
        assert_eq!(normalize_path("foo/../../bar"), "../bar");
        assert_eq!(normalize_path("../foo/../../bar"), "../../bar");
        assert_eq!(normalize_path("./foo"), "foo");
        assert_eq!(normalize_path("../foo"), "../foo");
        assert_eq!(normalize_path("/foo/bar"), "/foo/bar");
        assert_eq!(normalize_path("/foo/bar/"), "/foo/bar/");
    }
}

fn strip_slash_escape(str: &str) -> String {
    let mut res = String::default();
    res.reserve_exact(str.len());

    let mut iter = str.chars().peekable();
    let mut escaped = false;

    while let Some(c) = iter.next() {
        if !escaped && c == '\\' {
            if iter.peek() == Some(&'/') {
                continue;
            }

            escaped = true;
        } else {
            escaped = false;
        }

        res.push(c);
    }

    res
}

#[derive(Clone)]
pub struct RegexDef(pub Regex);

impl<'de> Deserialize<'de> for RegexDef {
    fn deserialize<D>(d: D) -> Result<RegexDef, D::Error>
    where D: Deserializer<'de>,
    {
        let s = <Cow<str>>::deserialize(d)?;

        match strip_slash_escape(s.as_ref()).parse() {
            Ok(regex) => Ok(RegexDef(regex)),
            Err(err) => Err(D::Error::custom(err)),
        }
    }
}
