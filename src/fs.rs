use lazy_static::lazy_static;
use regex::bytes::Regex;
use serde::Deserialize;
use std::{path::{Path, PathBuf}, str::Utf8Error};

use crate::zip::Zip;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileType {
    File,
    Directory,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ZipInfo {
    pub base_path: String,
    pub virtual_segments: Option<(String, String)>,
    pub zip_path: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VirtualInfo {
    pub base_path: String,
    pub virtual_segments: (String, String),
}

pub trait VPathInfo {
    fn physical_base_path(&self) -> PathBuf;
}

impl VPathInfo for ZipInfo {
    fn physical_base_path(&self) -> PathBuf {
        match &self.virtual_segments {
            None => PathBuf::from(&self.base_path),
            Some(segments) => PathBuf::from(&self.base_path).join(&segments.1),
        }
    }
}

impl VPathInfo for VirtualInfo {
    fn physical_base_path(&self) -> PathBuf {
        PathBuf::from(&self.base_path).join(&self.virtual_segments.1)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum VPath {
    Zip(ZipInfo),
    Virtual(VirtualInfo),
    Native(PathBuf),
}

impl VPath {
    pub fn from(p: &Path) -> std::io::Result<VPath> {
        vpath(p)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Entry not found")]
    EntryNotFound,

    #[error("Unsupported compression")]
    UnsupportedCompression,

    #[error("Decompression error")]
    DecompressionError,

    #[error(transparent)]
    Utf8Error(#[from] Utf8Error),

    #[error(transparent)]
    IOError(#[from] std::io::Error),
}

fn make_io_utf8_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "File did not contain valid UTF-8"
    )
}

fn io_bytes_to_str(vec: &[u8]) -> Result<&str, std::io::Error> {
    std::str::from_utf8(vec)
        .map_err(|_| make_io_utf8_error())
}

#[cfg(feature = "mmap")]
pub fn open_zip_via_mmap<P: AsRef<Path>>(p: P) -> Result<Zip<mmap_rs::Mmap>, std::io::Error> {
    let file = fs::File::open(p)?;

    let mmap_builder = mmap_rs::MmapOptions::new(file.metadata().unwrap().len().try_into().unwrap())
        .unwrap();

    let mmap = unsafe {
        mmap_builder
            .with_file(file, 0)
            .map()
            .unwrap()
    };

    let zip = Zip::new(mmap)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "Failed to read the zip file"))?;

    Ok(zip)
}

#[cfg(feature = "mmap")]
pub fn open_zip_via_mmap_p(p: &Path) -> Result<Zip<mmap_rs::Mmap>, std::io::Error> {
    open_zip_via_mmap(p)
}

pub fn open_zip_via_read<P: AsRef<Path>>(p: P) -> Result<Zip<Vec<u8>>, std::io::Error> {
    let data = std::fs::read(p)?;

    let zip = Zip::new(data)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "Failed to read the zip file"))?;

    Ok(zip)
}

pub fn open_zip_via_read_p(p: &Path) -> Result<Zip<Vec<u8>>, std::io::Error> {
    open_zip_via_read(p)
}

pub trait ZipCache<Storage>
where Storage: AsRef<[u8]> + Send + Sync {
    fn act<T, P: AsRef<Path>, F : FnOnce(&Zip<Storage>) -> T>(&self, p: P, cb: F) -> Result<T, std::io::Error>;

    fn file_type<P: AsRef<Path>, S: AsRef<str>>(&self, zip_path: P, sub: S) -> Result<FileType, std::io::Error>;
    fn read<P: AsRef<Path>, S: AsRef<str>>(&self, zip_path: P, sub: S) -> Result<Vec<u8>, std::io::Error>;
    fn read_to_string<P: AsRef<Path>, S: AsRef<str>>(&self, zip_path: P, sub: S) -> Result<String, std::io::Error>;
}

#[derive(Debug)]
pub struct LruZipCache<Storage>
where Storage: AsRef<[u8]> + Send + Sync {
    lru: concurrent_lru::sharded::LruCache<PathBuf, Zip<Storage>>,
    open: fn(&Path) -> std::io::Result<Zip<Storage>>,
}

impl<Storage> LruZipCache<Storage>
where Storage: AsRef<[u8]> + Send + Sync {
    pub fn new(n: u64, open: fn(&Path) -> std::io::Result<Zip<Storage>>) -> LruZipCache<Storage> {
        LruZipCache {
            lru: concurrent_lru::sharded::LruCache::new(n),
            open,
        }
    }
}

impl<Storage> ZipCache<Storage> for LruZipCache<Storage>
where Storage: AsRef<[u8]> + Send + Sync {
    fn act<T, P: AsRef<Path>, F: FnOnce(&Zip<Storage>) -> T>(&self, p: P, cb: F) -> Result<T, std::io::Error> {
        let zip = self.lru.get_or_try_init(p.as_ref().to_path_buf(), 1, |p| {
            (self.open)(&p)
        })?;

        Ok(cb(zip.value()))
    }

    fn file_type<P: AsRef<Path>, S: AsRef<str>>(&self, zip_path: P, p: S) -> Result<FileType, std::io::Error> {
        self.act(zip_path, |zip| zip.file_type(p.as_ref()))?
    }

    fn read<P: AsRef<Path>, S: AsRef<str>>(&self, zip_path: P, p: S) -> Result<Vec<u8>, std::io::Error> {
        self.act(zip_path, |zip| zip.read(p.as_ref()))?
    }

    fn read_to_string<P: AsRef<Path>, S: AsRef<str>>(&self, zip_path: P, p: S) -> Result<String, std::io::Error> {
        self.act(zip_path, |zip| zip.read_to_string(p.as_ref()))?
    }
}

fn split_zip(p_bytes: &[u8]) -> (&[u8], Option<&[u8]>) {
    lazy_static! {
        static ref ZIP_RE: Regex = Regex::new(r"\.zip").unwrap();
    }

    let mut search_offset = 0;

    while search_offset < p_bytes.len() {
        if let Some(m) = ZIP_RE.find_at(p_bytes, search_offset) {
            let idx = m.start();
            let next_char_idx = m.end();
    
            if idx == 0 || p_bytes.get(idx - 1) == Some(&b'/') || p_bytes.get(next_char_idx) != Some(&b'/') {
                search_offset = next_char_idx;
                continue;
            }
    
            let zip_path = &p_bytes[0..next_char_idx];
            let sub_path = p_bytes.get(next_char_idx + 1..);

            return (zip_path, sub_path);
        } else {
            break;
        }
    }

    (p_bytes, None)
}

fn split_virtual(p_bytes: &[u8]) -> std::io::Result<(usize, Option<(usize, usize)>)> {
    lazy_static! {
        static ref VIRTUAL_RE: Regex
            = Regex::new(
                "(?:^|/)((?:\\$\\$virtual|__virtual__)/(?:[^/]+)-[a-f0-9]+/([0-9]+)/)"
            ).unwrap();
    }

    if let Some(m) = VIRTUAL_RE.captures(p_bytes) {
        if let (Some(main), Some(depth)) = (m.get(1), m.get(2)) {
            if let Ok(depth_n) = str::parse(io_bytes_to_str(depth.as_bytes())?) {
                return Ok((main.start(), Some((main.end() - main.start(), depth_n))));
            }
        }
    }

    Ok((p_bytes.len(), None))
}

fn vpath(p: &Path) -> std::io::Result<VPath> {
    let p_str = arca::path::normalize_path(
        &p.as_os_str()
            .to_string_lossy()
    );

    let p_bytes = p_str
        .as_bytes().to_vec();

    let (archive_path_u8, zip_path_u8)
        = split_zip(&p_bytes);
    let (mut base_path_len, virtual_path_u8)
        = split_virtual(archive_path_u8)?;

    let mut base_path_u8 = archive_path_u8;
    let mut virtual_segments = None;

    if let Some((mut virtual_len, parent_depth)) = virtual_path_u8 {
        for _ in 0..parent_depth {
            if base_path_len == 1 {
                break;
            }

            base_path_len -= 1;
            virtual_len += 1;

            while let Some(c) = archive_path_u8.get(base_path_len - 1)  {
                if *c == b'/' {
                    break;
                } else {
                    base_path_len -= 1;
                    virtual_len += 1;
                }
            }
        }

        if let Some(c) = archive_path_u8.get(base_path_len - 1) {
            if *c != b'/' {
                return Err(std::io::Error::new(std::io::ErrorKind::Other, "Invalid virtual back-reference"))
            }
        } else {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "Invalid virtual back-reference"))
        }

        base_path_u8
            = &base_path_u8[0..base_path_len];

        // Trim the trailing slash
        if base_path_u8.len() > 1 {
            base_path_u8 = &base_path_u8[0..base_path_u8.len() - 1];
        }

        virtual_segments = Some((
            io_bytes_to_str(&archive_path_u8[base_path_len..archive_path_u8.len()])?.to_string(),
            io_bytes_to_str(&archive_path_u8[base_path_len + virtual_len..archive_path_u8.len()])?.to_string(),
        ));
    } else if zip_path_u8.is_none() {
        return Ok(VPath::Native(PathBuf::from(p_str)));
    }

    if let Some(zip_path_u8) = zip_path_u8 {
        Ok(VPath::Zip(ZipInfo {
            base_path: io_bytes_to_str(base_path_u8)?.to_string(),
            virtual_segments,
            zip_path: io_bytes_to_str(zip_path_u8)?.to_string(),
        }))
    } else {
        Ok(VPath::Virtual(VirtualInfo {
            base_path: io_bytes_to_str(base_path_u8)?.to_string(),
            virtual_segments: virtual_segments.unwrap(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_zip_type_api() {
        let zip = open_zip_via_read(&PathBuf::from(
            "data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip",
        ))
        .unwrap();

        assert_eq!(zip.file_type("node_modules").unwrap(), FileType::Directory);
        assert_eq!(zip.file_type("node_modules/").unwrap(), FileType::Directory);
    }

    #[test]
    #[should_panic(expected = "Kind(NotFound)")]
    fn test_zip_type_api_not_exist_dir_with_slash() {
        let zip = open_zip_via_read(&PathBuf::from(
            "data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip",
        ))
        .unwrap();

        zip.file_type("not_exists/").unwrap();
    }

    #[test]
    #[should_panic(expected = "Kind(NotFound)")]
    fn test_zip_type_api_not_exist_dir_without_slash() {
        let zip = open_zip_via_read(&PathBuf::from(
            "data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip",
        ))
        .unwrap();

        zip.file_type("not_exists").unwrap();
    }

    #[test]
    fn test_zip_list() {
        let zip = open_zip_via_read(&PathBuf::from("data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip"))
            .unwrap();

        let mut dirs: Vec<&String> = zip.dirs.iter().collect();
        let mut files: Vec<&String> = zip.files.keys().collect();

        dirs.sort();
        files.sort();

        assert_eq!(dirs, vec![
            "node_modules/",
            "node_modules/@babel/",
            "node_modules/@babel/plugin-syntax-dynamic-import/",
            "node_modules/@babel/plugin-syntax-dynamic-import/lib/",
        ]);

        assert_eq!(files, vec![
            "node_modules/@babel/plugin-syntax-dynamic-import/LICENSE",
            "node_modules/@babel/plugin-syntax-dynamic-import/README.md",
            "node_modules/@babel/plugin-syntax-dynamic-import/lib/index.js",
            "node_modules/@babel/plugin-syntax-dynamic-import/package.json",
        ]);
    }

    #[test]
    fn test_zip_read() {
        let zip = open_zip_via_read(&PathBuf::from("data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip"))
            .unwrap();

        let res = zip.read_to_string("node_modules/@babel/plugin-syntax-dynamic-import/package.json")
            .unwrap();

        assert_eq!(res, "{\n  \"name\": \"@babel/plugin-syntax-dynamic-import\",\n  \"version\": \"7.8.3\",\n  \"description\": \"Allow parsing of import()\",\n  \"repository\": \"https://github.com/babel/babel/tree/master/packages/babel-plugin-syntax-dynamic-import\",\n  \"license\": \"MIT\",\n  \"publishConfig\": {\n    \"access\": \"public\"\n  },\n  \"main\": \"lib/index.js\",\n  \"keywords\": [\n    \"babel-plugin\"\n  ],\n  \"dependencies\": {\n    \"@babel/helper-plugin-utils\": \"^7.8.0\"\n  },\n  \"peerDependencies\": {\n    \"@babel/core\": \"^7.0.0-0\"\n  },\n  \"devDependencies\": {\n    \"@babel/core\": \"^7.8.0\"\n  }\n}\n");
    }

    #[rstest]
    #[case(".zip", None)]
    #[case("foo", None)]
    #[case("foo.zip", None)]
    #[case("foo.zip/bar", Some(VPath::Zip(ZipInfo {
        base_path: "foo.zip".into(),
        virtual_segments: None,
        zip_path: "bar".into(),
    })))]
    #[case("foo.zip/bar/baz", Some(VPath::Zip(ZipInfo {
        base_path: "foo.zip".into(),
        virtual_segments: None,
        zip_path: "bar/baz".into(),
    })))]
    #[case("/a/b/c/foo.zip", None)]
    #[case("./a/b/c/foo.zip", None)]
    #[case("./a/b/__virtual__/foo-abcdef/0/c/d", Some(VPath::Virtual(VirtualInfo {
        base_path: "a/b".into(),
        virtual_segments: ("__virtual__/foo-abcdef/0/c/d".into(), "c/d".into()),
    })))]
    #[case("./a/b/__virtual__/foo-abcdef/1/c/d", Some(VPath::Virtual(VirtualInfo {
        base_path: "a".into(),
        virtual_segments: ("b/__virtual__/foo-abcdef/1/c/d".into(), "c/d".into()),
    })))]
    #[case("./a/b/__virtual__/foo-abcdef/0/c/foo.zip/bar", Some(VPath::Zip(ZipInfo {
        base_path: "a/b".into(),
        virtual_segments: Some(("__virtual__/foo-abcdef/0/c/foo.zip".into(), "c/foo.zip".into())),
        zip_path: "bar".into(),
    })))]
    #[case("./a/b/__virtual__/foo-abcdef/1/c/foo.zip/bar", Some(VPath::Zip(ZipInfo {
        base_path: "a".into(),
        virtual_segments: Some(("b/__virtual__/foo-abcdef/1/c/foo.zip".into(), "c/foo.zip".into())),
        zip_path: "bar".into(),
    })))]
    #[case("/a/b/__virtual__/foo-abcdef/1/c/foo.zip/bar", Some(VPath::Zip(ZipInfo {
        base_path: "/a".into(),
        virtual_segments: Some(("b/__virtual__/foo-abcdef/1/c/foo.zip".into(), "c/foo.zip".into())),
        zip_path: "bar".into(),
    })))]
    #[case("/a/b/__virtual__/foo-abcdef/2/c/foo.zip/bar", Some(VPath::Zip(ZipInfo {
        base_path: "/".into(),
        virtual_segments: Some(("a/b/__virtual__/foo-abcdef/2/c/foo.zip".into(), "c/foo.zip".into())),
        zip_path: "bar".into(),
    })))]
    #[case("/__virtual__/foo-abcdef/2/c/foo.zip/bar", Some(VPath::Zip(ZipInfo {
        base_path: "/".into(),
        virtual_segments: Some(("__virtual__/foo-abcdef/2/c/foo.zip".into(), "c/foo.zip".into())),
        zip_path: "bar".into(),
    })))]
    #[case("./a/b/c/.zip", None)]
    #[case("./a/b/c/foo.zipp", None)]
    #[case("./a/b/c/foo.zip/bar/baz/qux.zip", Some(VPath::Zip(ZipInfo {
        base_path: "a/b/c/foo.zip".into(),
        virtual_segments: None,
        zip_path: "bar/baz/qux.zip".into(),
    })))]
    #[case("./a/b/c/foo.zip-bar.zip", None)]
    #[case("./a/b/c/foo.zip-bar.zip/bar/baz/qux.zip", Some(VPath::Zip(ZipInfo {
        base_path: "a/b/c/foo.zip-bar.zip".into(),
        virtual_segments: None,
        zip_path: "bar/baz/qux.zip".into(),
    })))]
    #[case("./a/b/c/foo.zip-bar/foo.zip-bar/foo.zip-bar.zip/d", Some(VPath::Zip(ZipInfo {
        base_path: "a/b/c/foo.zip-bar/foo.zip-bar/foo.zip-bar.zip".into(),
        virtual_segments: None,
        zip_path: "d".into(),
    })))]
    fn test_path_to_pnp(#[case] input: &str, #[case] expected: Option<VPath>) {
        let expectation: VPath = match &expected {
            Some(p) => p.clone(),
            None => VPath::Native(PathBuf::from(arca::path::normalize_path(input))),
        };

        match vpath(&PathBuf::from(input)) {
            Ok(res) => {
                assert_eq!(res, expectation, "input='{:?}'", input);
            }
            Err(err) => {
                panic!("{:?}: {}", input, err);
            }
        }
    }
}
