use fancy_regex::Regex;
use serde::{Deserialize, Deserializer, de::Error};
use std::borrow::Cow;

use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub struct Trie<T> {
    inner: radix_trie::Trie<String, (PathBuf, T)>,
}

impl<T> Trie<T> {
    fn key<P: AsRef<Path>>(&self, key: &P) -> String {
        let mut p = normalize_path(key.as_ref().to_string_lossy());

        if !p.ends_with('/') {
            p.push('/');
        }

        p
    }

    pub fn get_ancestor_value<P: AsRef<Path>>(&self, key: &P) -> Option<&T> {
        self.inner.get_ancestor_value(&self.key(&key)).map(|t| &t.1)
    }

    pub fn insert<P: AsRef<Path>>(&mut self, key: P, value: T) {
        let k = self.key(&key);
        let p = PathBuf::from(k.clone());

        self.inner.insert(k, (p, value)).map(|t| t.1);
    }
}

pub fn normalize_path<P: AsRef<str>>(original: P) -> String {
    let original_str
        = original.as_ref();

    let check_str_root
        = original_str.strip_prefix("/");
    let str_minus_root
        = check_str_root.unwrap_or(original_str);

    let components
        = str_minus_root.split(&['/', '\\'][..]);

    let mut out: Vec<&str>
        = Vec::new();

    for comp in components {
        match comp {
            "" | "." => {
                // Those components don't progress the path
            },

            ".." => match out.last() {
                None if check_str_root.is_some() => {
                    // No need to add a ".." since we're already at the root
                },

                Some(&"..") | None => {
                    out.push(comp);
                },

                Some(_) => {
                    out.pop();
                },
            },

            comp => {
                out.push(comp)
            },
        }
    }

    if check_str_root.is_some() {
        if out.is_empty() {
            return "/".to_string();
        } else {
            out.insert(0, "");
        }
    }

    let mut str
        = out.join("/");

    if out.is_empty() {
        return ".".to_string();
    }

    if (original_str.ends_with('/') || original_str.ends_with(MAIN_SEPARATOR_STR))
        && !str.ends_with('/')
    {
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
        assert_eq!(normalize_path("foo/..//bar"), "bar");
        assert_eq!(normalize_path("foo/bar/.."), "foo");
        assert_eq!(normalize_path("foo/../../bar"), "../bar");
        assert_eq!(normalize_path("../foo/../../bar"), "../../bar");
        assert_eq!(normalize_path("./foo"), "foo");
        assert_eq!(normalize_path("../foo"), "../foo");
        assert_eq!(normalize_path("../D:/foo"), "../D:/foo");
        assert_eq!(normalize_path("/foo/bar"), "/foo/bar");
        assert_eq!(normalize_path("/../foo/bar"), "/foo/bar");
        assert_eq!(normalize_path("/foo/../../bar/baz"), "/bar/baz");
        assert_eq!(normalize_path("/../foo/bar"), "/foo/bar");
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

#[derive(Clone, Debug)]
pub struct RegexDef(pub Regex);

impl<'de> Deserialize<'de> for RegexDef {
    fn deserialize<D>(d: D) -> Result<RegexDef, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = <Cow<str>>::deserialize(d)?;

        match strip_slash_escape(s.as_ref()).parse() {
            Ok(regex) => Ok(RegexDef(regex)),
            Err(err) => Err(D::Error::custom(err)),
        }
    }
}
