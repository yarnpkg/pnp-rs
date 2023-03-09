use lazy_static::lazy_static;
use lru::LruCache;
use regex::bytes::Regex;
use serde::Deserialize;
use std::{path::{Path, PathBuf}, fs, io::{BufReader, Read}, collections::{HashSet, HashMap}};
use zip::{ZipArchive, result::ZipError};

use crate::util;

#[derive(Clone)]
#[derive(Debug)]
#[derive(Deserialize)]
#[derive(PartialEq)]
#[serde(untagged)]
pub enum PnpPath {
    Zip(PathBuf, Option<String>),
    Native(PathBuf),
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Entry not found")]
    EntryNotFound,

    #[error("Unsupported compression")]
    UnsupportedCompression,

    #[error(transparent)]
    IOError(#[from] std::io::Error),

    #[error(transparent)]
    ZipError(#[from] ZipError),
}

pub fn open_zip(p: &Path) -> Result<Zip, Error> {
    let file = fs::File::open(p)?;
    let reader = BufReader::new(file);

    let archive = ZipArchive::new(reader)?;
    let zip = Zip::new(archive)?;

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

            let name = util::normalize_path(entry.name());
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

    pub fn read_file(&mut self, p: &str) -> Result<Vec<u8>, Error> {
        let i = self.files.get(p)
            .ok_or(Error::EntryNotFound)?;
        
        let mut entry = self.archive.by_index_raw(*i)?;
        let mut data = Vec::new();

        entry.read_to_end(&mut data)?;

        match entry.compression() {
            zip::CompressionMethod::DEFLATE => {
                let mut out = Vec::new();
                out.resize(entry.size() as usize, 0);

                libdeflater::Decompressor::new().deflate_decompress(
                    &data,
                    &mut out,
                ).unwrap();

                Ok(out)
            }

            zip::CompressionMethod::STORE => {
                return Ok(data)
            }

            _ => {
                return Err(Error::UnsupportedCompression);
            }
        }
    }
}

pub trait ZipCache {
    fn act<T, F : FnOnce(&Zip) -> T>(&mut self, p: &Path, cb: F) -> Result<T, &Error>;
}

pub struct LruZipCache {
    lru: LruCache<PathBuf, Result<Zip, Error>>,
}

impl ZipCache for LruZipCache {
    fn act<T, F : FnOnce(&Zip) -> T>(&mut self, p: &Path, cb: F) -> Result<T, &Error> {
        let res = self.lru.get_or_insert(p.to_owned(), || {
            open_zip(p)
        });

        match res {
            Ok(zip) => Ok(cb(zip)),
            Err(err) => Err(&err),
        }
    }
}

/*
  static resolveVirtual(p: PortablePath): PortablePath {
    const match = p.match(VIRTUAL_REGEXP);
    if (!match || (!match[3] && match[5]))
      return p;

    const target = ppath.dirname(match[1] as PortablePath);
    if (!match[3] || !match[4])
      return target;

    const isnum = NUMBER_REGEXP.test(match[4]);
    if (!isnum)
      return p;

    const depth = Number(match[4]);
    const backstep = `../`.repeat(depth) as PortablePath;
    const subpath = (match[5] || `.`) as PortablePath;

    return VirtualFS.resolveVirtual(ppath.join(target, backstep, subpath));
  }
*/

pub fn path_to_pnp(p: &Path) -> Result<PnpPath, Box<dyn std::error::Error>> {
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

    let p_str = p.as_os_str()
        .to_string_lossy()
        .to_string();

    let mut p_bytes = util::normalize_path(p_str)
        .as_bytes().to_vec();

    if let Some(m) = VIRTUAL_RE.captures(&p_bytes) {
        if let (Some(target), Some(depth), Some(subpath)) = (m.get(1), m.get(4), m.get(5)) {
            if let Ok(depth_n) = str::parse(&std::str::from_utf8(depth.as_bytes())?) {
                let bytes = [
                    &target.as_bytes(),
                    &b"../".repeat(depth_n)[0..],
                    &subpath.as_bytes(),
                ].concat();

                p_bytes = util::normalize_path(std::str::from_utf8(&bytes)?)
                    .as_bytes().to_vec();    
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
                return Ok(PnpPath::Native(p.to_owned()))
            }

            if let Some(next_m) = ZIP_RE.find_at(&p_bytes, next_char_idx) {
                idx = next_m.start();
            } else {
                break;
            }
        }

        if p_bytes.len() > next_char_idx && p_bytes.get(next_char_idx) != Some(&b'/') {
            Ok(PnpPath::Native(p.to_owned()))
        } else {
            let zip_path = PathBuf::from(std::str::from_utf8(&p_bytes[0..next_char_idx])?);

            let sub_path = if next_char_idx + 1 < p_bytes.len() {
                Some(util::normalize_path(std::str::from_utf8(&p_bytes[next_char_idx + 1..])?))
            } else {
                None
            };

            Ok(PnpPath::Zip(zip_path, sub_path))
        }
    } else {
        Ok(PnpPath::Native(p.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn test_zip_list() {
        let zip = open_zip(&PathBuf::from("@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip"))
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
        let mut zip = open_zip(&PathBuf::from("@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip"))
            .unwrap();

        let res = zip.read_file("node_modules/@babel/plugin-syntax-dynamic-import/package.json")
            .unwrap();

        let res_str = std::str::from_utf8(&res)
            .unwrap();

        assert_eq!(res_str, "{\n  \"name\": \"@babel/plugin-syntax-dynamic-import\",\n  \"version\": \"7.8.3\",\n  \"description\": \"Allow parsing of import()\",\n  \"repository\": \"https://github.com/babel/babel/tree/master/packages/babel-plugin-syntax-dynamic-import\",\n  \"license\": \"MIT\",\n  \"publishConfig\": {\n    \"access\": \"public\"\n  },\n  \"main\": \"lib/index.js\",\n  \"keywords\": [\n    \"babel-plugin\"\n  ],\n  \"dependencies\": {\n    \"@babel/helper-plugin-utils\": \"^7.8.0\"\n  },\n  \"peerDependencies\": {\n    \"@babel/core\": \"^7.0.0-0\"\n  },\n  \"devDependencies\": {\n    \"@babel/core\": \"^7.8.0\"\n  }\n}\n");
    }

    #[test]
    fn test_path_to_pnp() {
        let tests: Vec<(PathBuf, Option<PnpPath>)> = serde_json::from_str(r#"[
            [".zip", null],
            ["foo", null],
            ["foo.zip", ["foo.zip", null]],
            ["foo.zip/bar", ["foo.zip", "bar"]],
            ["foo.zip/bar/baz", ["foo.zip", "bar/baz"]],
            ["/a/b/c/foo.zip", ["/a/b/c/foo.zip", null]],
            ["./a/b/c/foo.zip", ["a/b/c/foo.zip", null]],
            ["./a/b/__virtual__/abcdef/0/c/foo.zip", ["a/b/c/foo.zip", null]],
            ["./a/b/__virtual__/abcdef/1/c/foo.zip", ["a/c/foo.zip", null]],
            ["./a/b/c/.zip", null],
            ["./a/b/c/foo.zipp", null],
            ["./a/b/c/foo.zip/bar/baz/qux.zip", ["a/b/c/foo.zip", "bar/baz/qux.zip"]],
            ["./a/b/c/foo.zip-bar.zip", ["a/b/c/foo.zip-bar.zip", null]],
            ["./a/b/c/foo.zip-bar.zip/bar/baz/qux.zip", ["a/b/c/foo.zip-bar.zip", "bar/baz/qux.zip"]],
            ["./a/b/c/foo.zip-bar/foo.zip-bar/foo.zip-bar.zip/d", ["a/b/c/foo.zip-bar/foo.zip-bar/foo.zip-bar.zip", "d"]]
        ]"#).expect("Assertion failed: Expected the expectations to be loaded");

        for (input, expected) in tests.iter() {
            let expectation: PnpPath = match expected {
                Some(p) => p.clone(),
                None => PnpPath::Native(input.clone()),
            };

            match path_to_pnp(input) {
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
