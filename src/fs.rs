use lazy_static::lazy_static;
use lru::LruCache;
use regex::bytes::Regex;
use serde::Deserialize;
use std::{path::{Path, PathBuf}, fs, io::{BufReader, Read}, collections::{HashSet, HashMap}, str::Utf8Error, num::NonZeroUsize};
use zip::{ZipArchive, result::ZipError};

#[derive(Clone)]
#[derive(Debug)]
#[derive(Deserialize)]
#[derive(PartialEq)]
#[serde(untagged)]
pub enum VPath {
    Zip(PathBuf, String),
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

    #[error(transparent)]
    ZipError(#[from] ZipError),
}

fn make_io_utf8_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "File did not contain valid UTF-8"
    )
}

fn io_bytes_to_str(vec: &[u8]) -> Result<&str, std::io::Error> {
    std::str::from_utf8(&vec)
        .map_err(|_| make_io_utf8_error())
}

pub fn open_zip(p: &Path) -> Result<Zip, std::io::Error> {
    let file = fs::File::open(p)?;
    let reader = BufReader::new(file);

    let archive = ZipArchive::new(reader)?;
    let zip = Zip::new(archive)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "Failed to read the zip file"))?;

    Ok(zip)
}

pub struct Zip {
    archive: ZipArchive<BufReader<fs::File>>,
    files: HashMap<String, usize>,
    dirs: HashSet<String>,
}

impl Zip {
    pub fn new(archive: ZipArchive<BufReader<fs::File>>) -> Result<Zip, Error> {
        let mut zip = Zip {
            archive,
            files: Default::default(),
            dirs: Default::default(),
        };

        for i in 0..zip.archive.len() {
            let entry = zip.archive.by_index_raw(i)?;

            let name = arca::path::normalize_path(entry.name());
            let segments: Vec<&str> = name.split('/').collect();

            for t in 1..segments.len() - 1 {
                let dir = segments[0..t].to_vec().join("/");
                zip.dirs.insert(dir + "/");
            }

            if entry.is_dir() {
                zip.dirs.insert(name);
            } else if entry.is_file() {
                zip.files.insert(name, i);
            }
        }

        Ok(zip)
    }

    pub fn is_dir(&self, p: &str) -> bool {
        self.dirs.contains(p)
    }

    pub fn is_file(&self, p: &str) -> bool {
        self.files.contains_key(p)
    }

    pub fn read(&mut self, p: &str) -> Result<Vec<u8>, std::io::Error> {
        let i = self.files.get(p)
            .ok_or(std::io::Error::from(std::io::ErrorKind::NotFound))?;

        let mut entry = self.archive.by_index_raw(*i)?;
        let mut data = Vec::new();

        entry.read_to_end(&mut data)?;

        match entry.compression() {
            zip::CompressionMethod::DEFLATE => {
                let decompressed_data = miniz_oxide::inflate::decompress_to_vec(&data)
                    .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "Error during decompression"))?;

                Ok(decompressed_data)
            }

            zip::CompressionMethod::STORE => {
                Ok(data)
            }

            _ => {
                Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Unsupported compression algorithm"))
            }
        }
    }

    pub fn read_to_string(&mut self, p: &str) -> Result<String, std::io::Error> {
        let data = self.read(p)?;

        Ok(io_bytes_to_str(data.as_slice())?.to_string())
    }
}

pub trait ZipCache {
    fn act<T, F : FnOnce(&mut Zip) -> T>(&mut self, p: &Path, cb: F) -> Result<T, std::io::Error>;

    fn canonicalize(&mut self, zip_path: &Path, sub: &str) -> Result<PathBuf, std::io::Error>;

    fn is_dir(&mut self, zip_path: &Path, sub: &str) -> bool;
    fn is_file(&mut self, zip_path: &Path, sub: &str) -> bool;

    fn read(&mut self, zip_path: &Path, sub: &str) -> Result<Vec<u8>, std::io::Error>;
    fn read_to_string(&mut self, zip_path: &Path, sub: &str) -> Result<String, std::io::Error>;
}

pub struct LruZipCache {
    lru: LruCache<PathBuf, Zip>,
}

impl Default for LruZipCache {
    fn default() -> LruZipCache {
        LruZipCache::new(50)
    }
}

impl LruZipCache {
    pub fn new(n: usize) -> LruZipCache {
        LruZipCache {
            lru: LruCache::new(NonZeroUsize::new(n).unwrap()),
        }
    }
}

impl ZipCache for LruZipCache {
    fn act<T, F : FnOnce(&mut Zip) -> T>(&mut self, p: &Path, cb: F) -> Result<T, std::io::Error> {
        if let Some(zip) = self.lru.get_mut(p) {
            return Ok(cb(zip));
        }

        let zip = open_zip(p)?;
        self.lru.put(p.to_owned(), zip);

        Ok(cb(self.lru.get_mut(p).unwrap()))
    }

    fn canonicalize(&mut self, zip_path: &Path, sub: &str) -> Result<PathBuf, std::io::Error> {
        let res = std::fs::canonicalize(zip_path)?;

        Ok(res.join(sub))
    }

    fn is_dir(&mut self, zip_path: &Path, p: &str) -> bool {
        self.act(zip_path, |zip| zip.is_dir(p)).unwrap_or(false)
    }

    fn is_file(&mut self, zip_path: &Path, p: &str) -> bool {
        self.act(zip_path, |zip| zip.is_file(p)).unwrap_or(false)
    }

    fn read(&mut self, zip_path: &Path, p: &str) -> Result<Vec<u8>, std::io::Error> {
        self.act(zip_path, |zip| zip.read(p))?
    }

    fn read_to_string(&mut self, zip_path: &Path, p: &str) -> Result<String, std::io::Error> {
        self.act(zip_path, |zip| zip.read_to_string(p))?
    }
}

pub fn vpath(p: &Path) -> Result<VPath, std::io::Error> {
    lazy_static! {
        // $0: full path
        // $1: virtual folder
        // $2: virtual segment
        // $3: hash
        // $4: depth
        // $5: subpath
        static ref VIRTUAL_RE: Regex = Regex::new("(/?(?:[^/]+/)*?)(?:\\$\\$virtual|__virtual__)((?:/((?:[^/]+-)?[a-f0-9]+)(?:/([^/]+))?)?((?:/.*)?))$").unwrap();
        static ref ZIP_RE: Regex = Regex::new("\\.zip").unwrap();
    }

    let mut p_str = p.as_os_str()
        .to_string_lossy()
        .to_string();

    let mut p_bytes = arca::path::normalize_path(p_str.clone())
        .as_bytes().to_vec();

    if let Some(m) = VIRTUAL_RE.captures(&p_bytes) {
        if let (Some(target), Some(depth), Some(subpath)) = (m.get(1), m.get(4), m.get(5)) {
            if let Ok(depth_n) = str::parse(io_bytes_to_str(&depth.as_bytes())?) {
                let bytes = [
                    &target.as_bytes(),
                    &b"../".repeat(depth_n)[0..],
                    &subpath.as_bytes(),
                ].concat();

                p_str = arca::path::normalize_path(io_bytes_to_str(&bytes)?);
                p_bytes = p_str.as_bytes().to_vec();    
            }
        }
    }

    if let Some(m) = ZIP_RE.find(&p_bytes) {
        let mut idx = m.start();
        let mut next_char_idx;
        loop {
            next_char_idx = idx + 4;
            if p_bytes.get(next_char_idx) == Some(&b'/') {
                break;
            }

            if idx == 0 || p_bytes.get(idx - 1) == Some(&b'/') {
                return Ok(VPath::Native(p.to_owned()))
            }

            if let Some(next_m) = ZIP_RE.find_at(&p_bytes, next_char_idx) {
                idx = next_m.start();
            } else {
                break;
            }
        }

        if p_bytes.len() > next_char_idx && p_bytes.get(next_char_idx) != Some(&b'/') {
            Ok(VPath::Native(PathBuf::from(p_str)))
        } else {
            let zip_path = PathBuf::from(io_bytes_to_str(&p_bytes[0..next_char_idx])?);

            let sub_path = if next_char_idx + 1 < p_bytes.len() {
                arca::path::normalize_path(io_bytes_to_str(&p_bytes[next_char_idx + 1..])?)
            } else {
                return Ok(VPath::Native(zip_path))
            };

            Ok(VPath::Zip(zip_path, sub_path))
        }
    } else {
        Ok(VPath::Native(PathBuf::from(p_str)))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn test_zip_list() {
        let zip = open_zip(&PathBuf::from("data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip"))
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
        let mut zip = open_zip(&PathBuf::from("data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip"))
            .unwrap();

        let res = zip.read_to_string("node_modules/@babel/plugin-syntax-dynamic-import/package.json")
            .unwrap();

        assert_eq!(res, "{\n  \"name\": \"@babel/plugin-syntax-dynamic-import\",\n  \"version\": \"7.8.3\",\n  \"description\": \"Allow parsing of import()\",\n  \"repository\": \"https://github.com/babel/babel/tree/master/packages/babel-plugin-syntax-dynamic-import\",\n  \"license\": \"MIT\",\n  \"publishConfig\": {\n    \"access\": \"public\"\n  },\n  \"main\": \"lib/index.js\",\n  \"keywords\": [\n    \"babel-plugin\"\n  ],\n  \"dependencies\": {\n    \"@babel/helper-plugin-utils\": \"^7.8.0\"\n  },\n  \"peerDependencies\": {\n    \"@babel/core\": \"^7.0.0-0\"\n  },\n  \"devDependencies\": {\n    \"@babel/core\": \"^7.8.0\"\n  }\n}\n");
    }

    #[test]
    fn test_path_to_pnp() {
        let tests: Vec<(PathBuf, Option<VPath>)> = serde_json::from_str(r#"[
            [".zip", null],
            ["foo", null],
            ["foo.zip", "foo.zip"],
            ["foo.zip/bar", ["foo.zip", "bar"]],
            ["foo.zip/bar/baz", ["foo.zip", "bar/baz"]],
            ["/a/b/c/foo.zip", "/a/b/c/foo.zip"],
            ["./a/b/c/foo.zip", "a/b/c/foo.zip"],
            ["./a/b/__virtual__/abcdef/0/c/d", "a/b/c/d"],
            ["./a/b/__virtual__/abcdef/1/c/d", "a/c/d"],
            ["./a/b/__virtual__/abcdef/0/c/foo.zip/bar", ["a/b/c/foo.zip", "bar"]],
            ["./a/b/__virtual__/abcdef/1/c/foo.zip/bar", ["a/c/foo.zip", "bar"]],
            ["./a/b/c/.zip", null],
            ["./a/b/c/foo.zipp", null],
            ["./a/b/c/foo.zip/bar/baz/qux.zip", ["a/b/c/foo.zip", "bar/baz/qux.zip"]],
            ["./a/b/c/foo.zip-bar.zip", "a/b/c/foo.zip-bar.zip"],
            ["./a/b/c/foo.zip-bar.zip/bar/baz/qux.zip", ["a/b/c/foo.zip-bar.zip", "bar/baz/qux.zip"]],
            ["./a/b/c/foo.zip-bar/foo.zip-bar/foo.zip-bar.zip/d", ["a/b/c/foo.zip-bar/foo.zip-bar/foo.zip-bar.zip", "d"]]
        ]"#).expect("Assertion failed: Expected the expectations to be loaded");

        for (input, expected) in tests.iter() {
            let expectation: VPath = match expected {
                Some(p) => p.clone(),
                None => VPath::Native(input.clone()),
            };

            match vpath(input) {
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
