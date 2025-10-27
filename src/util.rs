use fancy_regex::Regex;
use serde::{Deserialize, Deserializer, de::Error};
use std::borrow::Cow;

use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};
#[cfg(windows)]
use std::sync::LazyLock;

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

#[cfg(windows)]
static WINDOWS_PATH_REGEXP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-zA-Z]:.*)$").unwrap());
#[cfg(windows)]
static UNC_WINDOWS_PATH_REGEXP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\/\\][\/\\](\.[\/\\])?(.*)$").unwrap());
#[cfg(windows)]
static PORTABLE_PATH_REGEXP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\/([a-zA-Z]:.*)$").unwrap());
#[cfg(windows)]
static UNC_PORTABLE_PATH_REGEXP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\/unc\/(\.dot\/)?(.*)$").unwrap());

fn from_portable_path<'a>(str: &'a str) -> Cow<'a, str> {
    #[cfg(windows)]
    {
        if let Ok(Some(caps)) = PORTABLE_PATH_REGEXP.captures(str) {
            return Cow::Borrowed(caps.get(1).unwrap().as_str());
        }

        if let Ok(Some(caps)) = UNC_PORTABLE_PATH_REGEXP.captures(str) {
            if caps.get(1).is_some() {
                return Cow::Owned(format!("\\\\.\\{}", caps.get(2).unwrap().as_str()));
            } else {
                return Cow::Owned(format!("\\\\{}", caps.get(2).unwrap().as_str()));
            }
        }
    }

    Cow::Borrowed(str)
}

fn to_portable_path<'a>(str: &'a str) -> Cow<'a, str> {
    #[cfg(windows)]
    {
        if let Ok(Some(caps)) = WINDOWS_PATH_REGEXP.captures(str) {
            return Cow::Owned(format!("/{}", caps.get(1).unwrap().as_str()));
        }

        if let Ok(Some(caps)) = UNC_WINDOWS_PATH_REGEXP.captures(str) {
            if caps.get(1).is_some() {
                return Cow::Owned(format!("/unc/.dot/{}", caps.get(2).unwrap().as_str()));
            } else {
                return Cow::Owned(format!("/unc/{}", caps.get(2).unwrap().as_str()));
            }
        }
    }

    Cow::Borrowed(str)
}

pub fn normalize_path<P: AsRef<str>>(original: P) -> String {
    let original_str = to_portable_path(original.as_ref());

    let check_str_root = original_str.strip_prefix('/');
    let str_minus_root = check_str_root.unwrap_or(original_str.as_ref());

    let components = str_minus_root.split(&['/', '\\'][..]);

    let mut out: Vec<&str> = Vec::new();

    for comp in components {
        match comp {
            "" | "." => {
                // Those components don't progress the path
            }

            ".." => match out.last() {
                None if check_str_root.is_some() => {
                    // No need to add a ".." since we're already at the root
                }

                Some(&"..") | None => {
                    out.push(comp);
                }

                Some(_) => {
                    out.pop();
                }
            },

            comp => out.push(comp),
        }
    }

    if check_str_root.is_some() {
        if out.is_empty() {
            return "/".to_string();
        } else {
            out.insert(0, "");
        }
    }

    let mut str = out.join("/");

    if out.is_empty() {
        return ".".to_string();
    }

    if (original_str.ends_with('/') || original_str.ends_with(MAIN_SEPARATOR_STR))
        && !str.ends_with('/')
    {
        str.push('/');
    }

    from_portable_path(&str).into_owned()
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
        assert_eq!(normalize_path("foo/../../bar"), "../bar");
        assert_eq!(normalize_path("./foo"), "foo");
        assert_eq!(normalize_path("../foo"), "../foo");
        assert_eq!(normalize_path("../D:/foo"), "../D:/foo");
        assert_eq!(normalize_path("/foo/bar"), "/foo/bar");
        assert_eq!(normalize_path("/foo/../../bar/baz"), "/bar/baz");
        assert_eq!(normalize_path("/../foo/bar"), "/foo/bar");
        assert_eq!(normalize_path("/../foo/bar//"), "/foo/bar/");
        assert_eq!(normalize_path("/foo/bar/"), "/foo/bar/");

        #[cfg(windows)]
        assert_eq!(normalize_path("D:\\foo\\..\\bar"), "D:/bar");
        #[cfg(windows)]
        assert_eq!(normalize_path("D:\\foo\\..\\..\\C:\\bar\\test"), "C:/bar/test");
        #[cfg(windows)]
        assert_eq!(normalize_path("\\\\server-name\\foo\\..\\bar"), "\\\\server-name/bar");
        #[cfg(windows)]
        assert_eq!(
            normalize_path("\\\\server-name\\foo\\..\\..\\..\\C:\\bar\\test"),
            "C:/bar/test"
        );
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
