use dashmap::DashMap;
use lazy_static::lazy_static;
use lru::LruCache;
use regex::bytes::Regex;
use serde::Deserialize;
use simple_error::bail;
use zip::ZipArchive;
use std::{path::{Path, PathBuf}, os::{unix::prelude::OsStrExt, macos::fs::MetadataExt}, fs, io::{BufReader}, collections::{HashSet, HashMap}};

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

type Error = Box<dyn std::error::Error>;

pub enum FileType {
    Unknown,
    File,
    Directory,
}

pub fn mode_to_file_type(mode: u32) -> FileType {
    match mode {
        _ => FileType::Unknown,
    }
}

struct ZipEntry {
    start: usize,
    end: usize,
    inflated_size: usize,
}

pub struct Zip {
    files: HashMap<String, ZipEntry>,
    dirs: HashSet<String>,
}

impl Zip {
    fn new(archive: &mut ZipArchive<BufReader<fs::File>>) -> Result<Zip, Error> {
        let mut zip = Zip {
            files: Default::default(),
            dirs: Default::default(),
        };

        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i)?;

            match entry.compression() {
                zip::CompressionMethod::DEFLATE => {}
                zip::CompressionMethod::STORE => {}

                _ => {
                    bail!("Assertion failed: Only Deflate files are supported");
                }
            }

            let name = util::normalize_path(entry.name());
            let segments: Vec<&str> = name.split('/').collect();

            for t in 1..segments.len() - 1 {
                let dir = segments[0..t].to_vec().join("/");
                zip.dirs.insert(dir + "/");
            }

            if entry.is_dir() {
                zip.dirs.insert(name);
            } else if entry.is_file() {
                zip.files.insert(name, ZipEntry {
                    start: entry.data_start() as usize,
                    end: (entry.data_start() + entry.compressed_size()) as usize,
                    inflated_size: entry.size() as usize,
                });
            }
        }

        Ok(zip)
    }

    fn is_dir(&self, p: &str) -> bool {
        self.dirs.contains(p)
    }

    fn is_file(&self, p: &str) -> bool {
        self.files.contains_key(p)
    }
}

pub trait ZipCache {
    fn act<T, F : FnOnce(&Zip) -> T>(&mut self, p: &Path, cb: F) -> Result<T, &Error>;
}

pub struct LruZipCache {
    lru: LruCache<PathBuf, Result<Zip, Error>>,
}

pub fn open_zip(p: &Path) -> Result<Zip, Error> {
    let file = fs::File::open(p)?;
    let reader = BufReader::new(file);

    let mut archive = ZipArchive::new(reader)?;
    let zip = Zip::new(&mut archive)?;

    Ok(zip)
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

fn dirent_type<C : ZipCache>(zip_cache: &mut C, path: &Path) -> std::io::Result<FileType> {
    match path_to_pnp(path) {
        Ok(PnpPath::Native(native_path)) => {
            let path: &Path = path.as_ref();
            Ok(mode_to_file_type(path.metadata()?.st_mode()))
        }

        Ok(PnpPath::Zip(zip_path, None)) => {
            let path: &Path = zip_path.as_ref();
            Ok(mode_to_file_type(path.metadata()?.st_mode()))
        }

        Ok(PnpPath::Zip(zip_path, Some(sub_path))) => {
            let path: &Path = zip_path.as_ref();

            zip_cache.act(zip_path.as_ref(), |zip| {
            });

            Ok(mode_to_file_type(path.metadata()?.st_mode()))
        }

        Err(err) => {
            Err(std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))
        }
    }
}

pub fn path_to_pnp(p: &Path) -> Result<PnpPath, Box<dyn std::error::Error>> {
    lazy_static! {
        static ref RE: Regex = Regex::new("\\.zip").unwrap();
    }

    let str = p.as_os_str()
        .as_bytes();

    if let Some(m) = RE.find(str) {
        let mut idx = m.start();
        let mut next_char_idx;
        loop {
            next_char_idx = idx + 4;
            if str.get(next_char_idx) == Some(&b'/') {
                break;
            }

            if idx == 0 || str.get(idx - 1) == Some(&b'/') {
                return Ok(PnpPath::Native(p.to_owned()))
            }

            if let Some(next_m) = RE.find_at(str, next_char_idx) {
                idx = next_m.start();
            } else {
                break;
            }
        }

        if str.len() > next_char_idx && str.get(next_char_idx) != Some(&b'/') {
            Ok(PnpPath::Native(p.to_owned()))
        } else {
            let zip_path = PathBuf::from(std::str::from_utf8(&str[0..next_char_idx])?);

            let sub_path = if next_char_idx + 1 < str.len() {
                Some(util::normalize_path(std::str::from_utf8(&str[next_char_idx + 1..])?))
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
    fn test_path_to_pnp() {
        let tests: Vec<(PathBuf, Option<PnpPath>)> = serde_json::from_str(r#"[
            [".zip", null],
            ["foo", null],
            ["foo.zip", ["foo.zip", null]],
            ["foo.zip/bar", ["foo.zip", "bar"]],
            ["foo.zip/bar/baz", ["foo.zip", "bar/baz"]],
            ["/a/b/c/foo.zip", ["/a/b/c/foo.zip", null]],
            ["./a/b/c/foo.zip", ["./a/b/c/foo.zip", null]],
            ["./a/b/c/.zip", null],
            ["./a/b/c/foo.zipp", null],
            ["./a/b/c/foo.zip/bar/baz/qux.zip", ["./a/b/c/foo.zip", "bar/baz/qux.zip"]],
            ["./a/b/c/foo.zip-bar.zip", ["./a/b/c/foo.zip-bar.zip", null]],
            ["./a/b/c/foo.zip-bar.zip/bar/baz/qux.zip", ["./a/b/c/foo.zip-bar.zip", "bar/baz/qux.zip"]],
            ["./a/b/c/foo.zip-bar/foo.zip-bar/foo.zip-bar.zip/d", ["./a/b/c/foo.zip-bar/foo.zip-bar/foo.zip-bar.zip", "d"]]
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
