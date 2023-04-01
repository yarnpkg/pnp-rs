use lazy_static::lazy_static;
use regex::bytes::Regex;
use serde::Deserialize;
use std::{path::{Path, PathBuf}, str::Utf8Error};

use crate::zip::Zip;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VPathInfo {
    pub base_path: String,
    pub virtual_segments: Option<(String, String)>,
    pub zip_path: Option<String>,
}

impl VPathInfo {
    pub fn physical_base_path(&self) -> PathBuf {
        match &self.virtual_segments {
            None => PathBuf::from(&self.base_path),
            Some(segments) => PathBuf::from(&self.base_path).join(&segments.1),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum VPath {
    Virtual(VPathInfo),
    Native(PathBuf),
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
pub fn open_zip_via_mmap(p: &Path) -> Result<Zip<mmap_rs::Mmap>, std::io::Error> {
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

pub fn open_zip_via_read(p: &Path) -> Result<Zip<Vec<u8>>, std::io::Error> {
    let data = std::fs::read(p)?;

    let zip = Zip::new(data)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "Failed to read the zip file"))?;

    Ok(zip)
}

pub trait ZipCache<Storage>
where Storage : AsRef<[u8]> + Send + Sync {
    fn act<T, F : FnOnce(&Zip<Storage>) -> T>(&self, p: &Path, cb: F) -> Result<T, std::io::Error>;

    fn canonicalize(&self, zip_path: &Path, sub: &str) -> Result<PathBuf, std::io::Error>;

    fn is_dir(&self, zip_path: &Path, sub: &str) -> bool;
    fn is_file(&self, zip_path: &Path, sub: &str) -> bool;

    fn read(&self, zip_path: &Path, sub: &str) -> Result<Vec<u8>, std::io::Error>;
    fn read_to_string(&self, zip_path: &Path, sub: &str) -> Result<String, std::io::Error>;
}

pub struct LruZipCache<Storage>
where Storage : AsRef<[u8]> + Send + Sync {
    lru: concurrent_lru::sharded::LruCache<PathBuf, Zip<Storage>>,
    open: fn(&Path) -> std::io::Result<Zip<Storage>>,
}

impl<Storage> LruZipCache<Storage>
where Storage : AsRef<[u8]> + Send + Sync {
    pub fn new(n: u64, open: fn(&Path) -> std::io::Result<Zip<Storage>>) -> LruZipCache<Storage> {
        LruZipCache {
            lru: concurrent_lru::sharded::LruCache::new(n),
            open,
        }
    }
}

impl<Storage> ZipCache<Storage> for LruZipCache<Storage>
where Storage : AsRef<[u8]> + Send + Sync {
    fn act<T, F : FnOnce(&Zip<Storage>) -> T>(&self, p: &Path, cb: F) -> Result<T, std::io::Error> {
        let zip = self.lru.get_or_try_init(p.to_path_buf(), 1, |p| {
            (self.open)(&p)
        })?;

        Ok(cb(zip.value()))
    }

    fn canonicalize(&self, zip_path: &Path, sub: &str) -> Result<PathBuf, std::io::Error> {
        let res = std::fs::canonicalize(zip_path)?;

        Ok(res.join(sub))
    }

    fn is_dir(&self, zip_path: &Path, p: &str) -> bool {
        self.act(zip_path, |zip| zip.is_dir(p)).unwrap_or(false)
    }

    fn is_file(&self, zip_path: &Path, p: &str) -> bool {
        self.act(zip_path, |zip| zip.is_file(p)).unwrap_or(false)
    }

    fn read(&self, zip_path: &Path, p: &str) -> Result<Vec<u8>, std::io::Error> {
        self.act(zip_path, |zip| zip.read(p))?
    }

    fn read_to_string(&self, zip_path: &Path, p: &str) -> Result<String, std::io::Error> {
        self.act(zip_path, |zip| zip.read_to_string(p))?
    }
}

pub fn split_zip(p_bytes: &[u8]) -> (&[u8], Option<&[u8]>) {
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

pub fn split_virtual(p_bytes: &[u8]) -> std::io::Result<(usize, Option<(usize, usize)>)> {
    lazy_static! {
        static ref VIRTUAL_RE: Regex = Regex::new("(?:^|/)((?:\\$\\$virtual|__virtual__)/(?:[^/]+)-[a-f0-9]+/([0-9]+)/)").unwrap();
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

pub fn vpath(p: &Path) -> std::io::Result<VPath> {
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

        base_path_u8 = &archive_path_u8[0..base_path_len - 1];

        virtual_segments = Some((
            io_bytes_to_str(&archive_path_u8[base_path_len..archive_path_u8.len()])?.to_string(),
            io_bytes_to_str(&archive_path_u8[base_path_len + virtual_len..archive_path_u8.len()])?.to_string(),
        ));
    } else if zip_path_u8.is_none() {
        return Ok(VPath::Native(PathBuf::from(p_str)));
    }

    Ok(VPath::Virtual(VPathInfo {
        base_path: io_bytes_to_str(base_path_u8)?.to_string(),
        virtual_segments,
        zip_path: zip_path_u8.map(|data| {
            io_bytes_to_str(data).map(|str| str.to_string())
        }).transpose()?,
    }))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

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

    #[test]
    fn test_path_to_pnp() {
        let tests: Vec<(String, Option<VPath>)> = serde_json::from_str(r#"[
            [".zip", null],
            ["foo", null],
            ["foo.zip", null],
            ["foo.zip/bar", {
                "basePath": "foo.zip",
                "virtualSegments": null,
                "zipPath": "bar"
            }],
            ["foo.zip/bar/baz", {
                "basePath": "foo.zip",
                "virtualSegments": null,
                "zipPath": "bar/baz"
            }],
            ["/a/b/c/foo.zip", null],
            ["./a/b/c/foo.zip", null],
            ["./a/b/__virtual__/foo-abcdef/0/c/d", {
                "basePath": "a/b",
                "virtualSegments": ["__virtual__/foo-abcdef/0/c/d", "c/d"],
                "zipPath": null
            }],
            ["./a/b/__virtual__/foo-abcdef/1/c/d", {
                "basePath": "a",
                "virtualSegments": ["b/__virtual__/foo-abcdef/1/c/d", "c/d"],
                "zipPath": null
            }],
            ["./a/b/__virtual__/foo-abcdef/0/c/foo.zip/bar", {
                "basePath": "a/b",
                "virtualSegments": ["__virtual__/foo-abcdef/0/c/foo.zip", "c/foo.zip"],
                "zipPath": "bar"
            }],
            ["./a/b/__virtual__/foo-abcdef/1/c/foo.zip/bar", {
                "basePath": "a",
                "virtualSegments": ["b/__virtual__/foo-abcdef/1/c/foo.zip", "c/foo.zip"],
                "zipPath": "bar"
            }],
            ["./a/b/c/.zip", null],
            ["./a/b/c/foo.zipp", null],
            ["./a/b/c/foo.zip/bar/baz/qux.zip", {
                "basePath": "a/b/c/foo.zip",
                "virtualSegments": null,
                "zipPath": "bar/baz/qux.zip"
            }],
            ["./a/b/c/foo.zip-bar.zip", null],
            ["./a/b/c/foo.zip-bar.zip/bar/baz/qux.zip", {
                "basePath": "a/b/c/foo.zip-bar.zip",
                "virtualSegments": null,
                "zipPath": "bar/baz/qux.zip"
            }],
            ["./a/b/c/foo.zip-bar/foo.zip-bar/foo.zip-bar.zip/d", {
                "basePath": "a/b/c/foo.zip-bar/foo.zip-bar/foo.zip-bar.zip",
                "virtualSegments": null,
                "zipPath": "d"
            }]
        ]"#).expect("Assertion failed: Expected the expectations to be loaded");

        for (input, expected) in tests.iter() {
            let expectation: VPath = match expected {
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
}
