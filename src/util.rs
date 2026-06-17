use regress::Regex;
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
        if let Some(caps) = PORTABLE_PATH_REGEXP.find(str) {
            return Cow::Borrowed(&str[caps.group(1).unwrap()]);
        }

        if let Some(caps) = UNC_PORTABLE_PATH_REGEXP.find(str) {
            if caps.group(1).is_some() {
                return Cow::Owned(format!("\\\\.\\{}", &str[caps.group(2).unwrap()]));
            } else {
                return Cow::Owned(format!("\\\\{}", &str[caps.group(2).unwrap()]));
            }
        }
    }

    Cow::Borrowed(str)
}

fn to_portable_path<'a>(str: &'a str) -> Cow<'a, str> {
    #[cfg(windows)]
    {
        if let Some(caps) = WINDOWS_PATH_REGEXP.find(str) {
            return Cow::Owned(format!("/{}", &str[caps.group(1).unwrap()]));
        }

        if let Some(caps) = UNC_WINDOWS_PATH_REGEXP.find(str) {
            if caps.group(1).is_some() {
                return Cow::Owned(format!("/unc/.dot/{}", &str[caps.group(2).unwrap()]));
            } else {
                return Cow::Owned(format!("/unc/{}", &str[caps.group(2).unwrap()]));
            }
        }
    }

    Cow::Borrowed(str)
}

pub(crate) fn is_drive_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

pub fn normalize_path<P: AsRef<str>>(original: P) -> String {
    let original_str = to_portable_path(original.as_ref());

    let rooted = original_str.starts_with('/');
    let body = original_str.strip_prefix('/').unwrap_or(&original_str);

    let mut components = body.split(['/', '\\']).peekable();

    // A leading drive prefix (`C:`, `D:`, …) is treated as part of the root,
    // so `..` overshoot can't pop the drive letter and leave a rootless path
    // that downstream consumers misread as drive-relative. See #9.
    // Only gated on Windows: on Unix, segments like `C:` are ordinary names.
    let mut drive: Option<&str> =
        if cfg!(windows) && rooted && components.peek().is_some_and(|c| is_drive_prefix(c)) {
            components.next()
        } else {
            None
        };

    let mut out: Vec<&str> = Vec::new();
    for comp in components {
        // A mid-path drive prefix replaces the current root, matching Windows
        // semantics where `D:\foo\..\..\C:\bar` resolves to `C:\bar`.
        if drive.is_some() && is_drive_prefix(comp) {
            out.clear();
            drive = Some(comp);
            continue;
        }

        match comp {
            "" | "." => {}
            ".." => match out.last() {
                None if rooted => { /* clamp at root */ }
                Some(&"..") | None => out.push(comp),
                Some(_) => {
                    out.pop();
                }
            },
            c => out.push(c),
        }
    }

    let mut result = String::new();
    if rooted {
        result.push('/');
    }
    if let Some(d) = drive {
        // Always emit the slash after the drive — `C:\..` is `C:\` (drive
        // root), not `C:` (drive-relative).
        result.push_str(d);
        result.push('/');
    }
    result.push_str(&out.join("/"));

    if result.is_empty() {
        return ".".to_string();
    }

    if (original_str.ends_with('/') || original_str.ends_with(MAIN_SEPARATOR_STR))
        && !result.ends_with('/')
    {
        result.push('/');
    }

    from_portable_path(&result).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_pattern_lookahead_behaviour() {
        // The real `.yarn/sdks/**` output Yarn emits via `micromatch.makeRe`
        // (negative-lookahead dot-guards). regress evaluates it with the same
        // ECMAScript semantics Yarn uses at runtime via `new RegExp(...)`.
        let sdks = Regex::new(
            r"(^(?:\.yarn\/sdks(?:\/(?!\.{1,2}(?:\/|$))(?:(?:(?!(?:^|\/)\.{1,2}(?:\/|$)).)*?)|$))$)",
        )
        .unwrap();
        for hit in
            [".yarn/sdks", ".yarn/sdks/", ".yarn/sdks/typescript/lib/tsc.js", ".yarn/sdks/.bin"]
        {
            assert!(sdks.find(hit).is_some(), "should match {hit:?}");
        }
        for miss in ["node_modules/foo", ".yarn/sdksx", ".yarn/sdk"] {
            assert!(sdks.find(miss).is_none(), "should not match {miss:?}");
        }
    }

    #[test]
    fn regexdef_deserializes_js_slash_escape() {
        // `ignorePatternData` uses JS `\/`; regress accepts it, so no
        // slash-unescaping pre-pass is needed. (JSON `\\/` -> regex `\/`.)
        let def: RegexDef = serde_json::from_str(r#""^a\\/b$""#).unwrap();
        assert!(def.0.find("a/b").is_some());
        assert!(def.0.find("axb").is_none());
    }

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

        // Drive root is the floor — `..` past it must clamp, not consume the drive.
        // Repro for https://github.com/yarnpkg/pnp-rs/issues/9 (same-drive overshoot).
        #[cfg(windows)]
        assert_eq!(
            normalize_path("C:\\dev\\project\\..\\..\\..\\Users\\USERNAME\\foo"),
            "C:/Users/USERNAME/foo"
        );
        #[cfg(windows)]
        assert_eq!(normalize_path("C:\\.."), "C:/");
        #[cfg(windows)]
        assert_eq!(normalize_path("C:\\foo\\..\\..\\.."), "C:/");
        #[cfg(windows)]
        assert_eq!(normalize_path("C:/dev/project/../../Users/USERNAME"), "C:/Users/USERNAME");
    }
}

#[derive(Clone, Debug)]
pub struct RegexDef(pub Regex);

impl<'de> Deserialize<'de> for RegexDef {
    fn deserialize<D>(d: D) -> Result<RegexDef, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = <Cow<str>>::deserialize(d)?;

        // `ignorePatternData` is an ECMAScript regex generated by Yarn's
        // `micromatch.makeRe(...)` (it uses negative lookahead), so parse it
        // with the ECMAScript engine — including JS-style `\/` escapes, which
        // is why no slash-unescaping pre-pass is needed.
        match Regex::new(s.as_ref()) {
            Ok(regex) => Ok(RegexDef(regex)),
            Err(err) => Err(D::Error::custom(err)),
        }
    }
}
