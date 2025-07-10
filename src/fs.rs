use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    str::Utf8Error,
};

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

#[cfg(feature = "mmap")]
pub fn open_zip_via_mmap<P: AsRef<Path>>(p: P) -> Result<Zip<mmap_rs::Mmap>, std::io::Error> {
    let file = std::fs::File::open(p)?;

    let mmap_builder =
        mmap_rs::MmapOptions::new(file.metadata().unwrap().len().try_into().unwrap()).unwrap();

    let mmap = unsafe { mmap_builder.with_file(&file, 0).map().unwrap() };

    let zip = Zip::new(mmap).map_err(|_| std::io::Error::other("Failed to read the zip file"))?;

    Ok(zip)
}

#[cfg(feature = "mmap")]
pub fn open_zip_via_mmap_p(p: &Path) -> Result<Zip<mmap_rs::Mmap>, std::io::Error> {
    open_zip_via_mmap(p)
}

pub fn open_zip_via_read<P: AsRef<Path>>(p: P) -> Result<Zip<Vec<u8>>, std::io::Error> {
    let data = std::fs::read(p)?;

    let zip = Zip::new(data).map_err(|_| std::io::Error::other("Failed to read the zip file"))?;

    Ok(zip)
}

pub fn open_zip_via_read_p(p: &Path) -> Result<Zip<Vec<u8>>, std::io::Error> {
    open_zip_via_read(p)
}

pub trait ZipCache<Storage>
where
    Storage: AsRef<[u8]> + Send + Sync,
{
    fn act<T, P: AsRef<Path>, F: FnOnce(&Zip<Storage>) -> T>(
        &self,
        p: P,
        cb: F,
    ) -> Result<T, std::io::Error>;

    fn file_type<P: AsRef<Path>, S: AsRef<str>>(
        &self,
        zip_path: P,
        sub: S,
    ) -> Result<FileType, std::io::Error>;
    fn read<P: AsRef<Path>, S: AsRef<str>>(
        &self,
        zip_path: P,
        sub: S,
    ) -> Result<Vec<u8>, std::io::Error>;
    fn read_to_string<P: AsRef<Path>, S: AsRef<str>>(
        &self,
        zip_path: P,
        sub: S,
    ) -> Result<String, std::io::Error>;
}

#[derive(Debug)]
pub struct LruZipCache<Storage>
where
    Storage: AsRef<[u8]> + Send + Sync,
{
    lru: concurrent_lru::sharded::LruCache<PathBuf, Zip<Storage>>,
    open: fn(&Path) -> std::io::Result<Zip<Storage>>,
}

impl<Storage> LruZipCache<Storage>
where
    Storage: AsRef<[u8]> + Send + Sync,
{
    pub fn new(n: u64, open: fn(&Path) -> std::io::Result<Zip<Storage>>) -> LruZipCache<Storage> {
        LruZipCache { lru: concurrent_lru::sharded::LruCache::new(n), open }
    }
}

impl<Storage> ZipCache<Storage> for LruZipCache<Storage>
where
    Storage: AsRef<[u8]> + Send + Sync,
{
    fn act<T, P: AsRef<Path>, F: FnOnce(&Zip<Storage>) -> T>(
        &self,
        p: P,
        cb: F,
    ) -> Result<T, std::io::Error> {
        let zip = self.lru.get_or_try_init(p.as_ref().to_path_buf(), 1, |p| (self.open)(p))?;

        Ok(cb(zip.value()))
    }

    fn file_type<P: AsRef<Path>, S: AsRef<str>>(
        &self,
        zip_path: P,
        p: S,
    ) -> Result<FileType, std::io::Error> {
        self.act(zip_path, |zip| zip.file_type(p.as_ref()))?
    }

    fn read<P: AsRef<Path>, S: AsRef<str>>(
        &self,
        zip_path: P,
        p: S,
    ) -> Result<Vec<u8>, std::io::Error> {
        self.act(zip_path, |zip| zip.read(p.as_ref()))?
    }

    fn read_to_string<P: AsRef<Path>, S: AsRef<str>>(
        &self,
        zip_path: P,
        p: S,
    ) -> Result<String, std::io::Error> {
        self.act(zip_path, |zip| zip.read_to_string(p.as_ref()))?
    }
}

fn vpath(p: &Path) -> std::io::Result<VPath> {
    let Some(p_str) = p.as_os_str().to_str() else {
        return Ok(VPath::Native(p.to_path_buf()));
    };

    let normalized_path = crate::util::normalize_path(p_str);

    // We remove potential leading slashes to avoid __virtual__ accidentally removing them
    let normalized_relative_path = normalized_path.strip_prefix('/').unwrap_or(&normalized_path);

    let mut segment_it = normalized_relative_path.split('/');

    // `split` returns [""] if the path is empty; we need to remove it
    if normalized_relative_path.is_empty() {
        segment_it.next();
    }

    let mut base_items: Vec<&str> = Vec::with_capacity(10);

    let mut virtual_items: Option<Vec<&str>> = None;
    let mut internal_items: Option<Vec<&str>> = None;
    let mut zip_items: Option<Vec<&str>> = None;

    while let Some(segment) = segment_it.next() {
        if let Some(zip_segments) = &mut zip_items {
            zip_segments.push(segment);
            continue;
        }

        if segment == "__virtual__" && virtual_items.is_none() {
            let mut acc_segments = Vec::with_capacity(3);

            acc_segments.push(segment);

            // We just skip the arbitrary hash, it doesn't matter what it is
            if let Some(hash_segment) = segment_it.next() {
                acc_segments.push(hash_segment);
            }

            // We retrieve the depth
            if let Some(depth_segment) = segment_it.next() {
                let depth = depth_segment.parse::<usize>();

                acc_segments.push(depth_segment);

                // We extract the backward segments from the base ones
                if let Ok(depth) = depth {
                    let parent_segments =
                        base_items.split_off(base_items.len().saturating_sub(depth));

                    acc_segments.splice(0..0, parent_segments);
                }
            }

            virtual_items = Some(acc_segments);
            internal_items = Some(Vec::with_capacity(10));

            continue;
        }

        if segment.len() > 4 && segment.ends_with(".zip") {
            zip_items = Some(Vec::with_capacity(10));
        }

        if let Some(virtual_segments) = &mut virtual_items {
            virtual_segments.push(segment);
        }

        if let Some(internal_segments) = &mut internal_items {
            internal_segments.push(segment);
        } else {
            base_items.push(segment);
        }
    }

    let virtual_segments = match (virtual_items, internal_items) {
        (Some(virtual_segments), Some(internal_segments)) => {
            Some((virtual_segments.join("/"), internal_segments.join("/")))
        }

        _ => None,
    };

    if let Some(zip_segments) = zip_items {
        let mut base_path = base_items.join("/");

        // Don't forget to add back the leading slash we removed earlier
        if normalized_relative_path != normalized_path {
            base_path.insert(0, '/');
        }

        if !zip_segments.is_empty() {
            return Ok(VPath::Zip(ZipInfo {
                base_path,
                virtual_segments,
                zip_path: zip_segments.join("/"),
            }));
        }
    }

    if let Some(virtual_segments) = virtual_segments {
        let mut base_path = base_items.join("/");

        // Don't forget to add back the leading slash we removed earlier
        if normalized_relative_path != normalized_path {
            base_path.insert(0, '/');
        }

        return Ok(VPath::Virtual(VirtualInfo {
            base_path,
            virtual_segments,
        }));
    }

    Ok(VPath::Native(PathBuf::from(normalized_path)))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use std::path::PathBuf;

    use crate::util;

    use super::*;

    #[test]
    fn test_zip_type_api() {
        let zip = open_zip_via_read(PathBuf::from(
            "data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip",
        ))
        .unwrap();

        assert_eq!(zip.file_type("node_modules").unwrap(), FileType::Directory);
        assert_eq!(zip.file_type("node_modules/").unwrap(), FileType::Directory);
    }

    #[test]
    #[should_panic(expected = "Kind(NotFound)")]
    fn test_zip_type_api_not_exist_dir_with_slash() {
        let zip = open_zip_via_read(PathBuf::from(
            "data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip",
        ))
        .unwrap();

        zip.file_type("not_exists/").unwrap();
    }

    #[test]
    #[should_panic(expected = "Kind(NotFound)")]
    fn test_zip_type_api_not_exist_dir_without_slash() {
        let zip = open_zip_via_read(PathBuf::from(
            "data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip",
        ))
        .unwrap();

        zip.file_type("not_exists").unwrap();
    }

    #[test]
    fn test_zip_list() {
        let zip = open_zip_via_read(PathBuf::from(
            "data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip",
        ))
        .unwrap();

        let mut dirs: Vec<&String> = zip.dirs.iter().collect();
        let mut files: Vec<&String> = zip.files.keys().collect();

        dirs.sort();
        files.sort();

        assert_eq!(
            dirs,
            vec![
                "node_modules/",
                "node_modules/@babel/",
                "node_modules/@babel/plugin-syntax-dynamic-import/",
                "node_modules/@babel/plugin-syntax-dynamic-import/lib/",
            ]
        );

        assert_eq!(
            files,
            vec![
                "node_modules/@babel/plugin-syntax-dynamic-import/LICENSE",
                "node_modules/@babel/plugin-syntax-dynamic-import/README.md",
                "node_modules/@babel/plugin-syntax-dynamic-import/lib/index.js",
                "node_modules/@babel/plugin-syntax-dynamic-import/package.json",
            ]
        );
    }

    #[test]
    fn test_zip_read() {
        let zip = open_zip_via_read(PathBuf::from(
            "data/@babel-plugin-syntax-dynamic-import-npm-7.8.3-fb9ff5634a-8.zip",
        ))
        .unwrap();

        let res = zip
            .read_to_string("node_modules/@babel/plugin-syntax-dynamic-import/package.json")
            .unwrap();

        assert_eq!(
            res,
            "{\n  \"name\": \"@babel/plugin-syntax-dynamic-import\",\n  \"version\": \"7.8.3\",\n  \"description\": \"Allow parsing of import()\",\n  \"repository\": \"https://github.com/babel/babel/tree/master/packages/babel-plugin-syntax-dynamic-import\",\n  \"license\": \"MIT\",\n  \"publishConfig\": {\n    \"access\": \"public\"\n  },\n  \"main\": \"lib/index.js\",\n  \"keywords\": [\n    \"babel-plugin\"\n  ],\n  \"dependencies\": {\n    \"@babel/helper-plugin-utils\": \"^7.8.0\"\n  },\n  \"peerDependencies\": {\n    \"@babel/core\": \"^7.0.0-0\"\n  },\n  \"devDependencies\": {\n    \"@babel/core\": \"^7.8.0\"\n  }\n}\n"
        );
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
            None => VPath::Native(PathBuf::from(util::normalize_path(input))),
        };

        match vpath(&PathBuf::from(input)) {
            Ok(res) => {
                assert_eq!(res, expectation, "input='{input:?}'");
            }
            Err(err) => {
                panic!("{input:?}: {err}");
            }
        }
    }
}
