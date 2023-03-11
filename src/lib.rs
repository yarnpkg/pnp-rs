pub mod fs;
mod util;

use fancy_regex::Regex;
use lazy_static::lazy_static;
use radix_trie::Trie;
use serde::Deserialize;
use serde_with::{serde_as, DefaultOnNull};
use std::{path::{Path, PathBuf}, collections::{HashSet, HashMap, hash_map::Entry}};
use util::RegexDef;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Bad specifier")]
    BadSpecifier,

    #[error("Bad specifier")]
    FailedResolution,

    #[error("Assertion failed: Regular expression failed to run")]
    Disconnect(#[from] fancy_regex::Error),

    #[error(transparent)]
    JsonError(#[from] serde_json::Error),

    #[error(transparent)]
    IOError(#[from] std::io::Error),
}

pub enum Resolution {
    Specifier(String),
    Package(PathBuf, Option<String>),
}

pub struct ResolutionHost {
    pub find_pnp_manifest: Box<dyn Fn(&Path) -> Result<Option<Manifest>, Error>>,
}

impl Default for ResolutionHost {
    fn default() -> ResolutionHost {
        ResolutionHost {
            find_pnp_manifest: Box::new(find_pnp_manifest),
        }
    }
}

#[derive(Default)]
pub struct ResolutionConfig {
    pub builtins: HashSet<String>,
    pub host: ResolutionHost,
}

#[derive(Clone)]
#[derive(Deserialize)]
pub struct PackageLocator {
    name: String,
    reference: String,
}

#[derive(Clone)]
#[derive(Deserialize)]
#[serde(untagged)]
enum PackageDependency {
    Reference(String),
    Alias(String, String),
}

#[serde_as]
#[derive(Clone)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInformation {
    package_location: String,

    #[serde(default)]
    discard_from_lookup: bool,

    #[serde_as(as = "Vec<(_, Option<_>)>")]
    package_dependencies: HashMap<String, Option<PackageDependency>>,
}

#[serde_as]
#[derive(Clone)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(skip_deserializing)]
    manifest_dir: PathBuf,

    #[serde(skip_deserializing)]
    location_trie: Trie<String, PackageLocator>,

    enable_top_level_fallback: bool,
    ignore_pattern_data: Option<RegexDef>,

    // fallbackPool: [[
    //   "@app/monorepo",
    //   "workspace:.",
    // ]]
    #[serde_as(as = "Vec<(_, _)>")]
    fallback_pool: HashMap<String, Option<PackageDependency>>,

    // fallbackExclusionList: [[
    //   "@app/server",
    //  ["workspace:sources/server"],
    // ]]
    #[serde_as(as = "Vec<(_, _)>")]
    fallback_exclusion_list: HashMap<String, HashSet<String>>,

    // packageRegistryData: [
    //   [null, [
    //     [null, {
    //       ...
    //     }]
    //   }]
    // ]
    #[serde_as(as = "Vec<(DefaultOnNull<_>, Vec<(DefaultOnNull<_>, _)>)>")]
    package_registry_data: HashMap<String, HashMap<String, PackageInformation>>,
}

pub fn parse_bare_identifier(specifier: &str) -> Result<(String, Option<String>), Error> {
    let mut segments = specifier.splitn(3, '/');
    let mut ident_option: Option<String> = None;

    if let Some(first) = segments.next() {
        if first.starts_with('@') {
            if let Some(second) = segments.next() {
                ident_option = Some(format!("{}/{}", first, second));
            }
        } else {
            ident_option = Some(first.to_string());
        }
    }

    if let Some(ident) = ident_option {
        Ok((ident, segments.next().map(|v| v.to_string())))
    } else {
        Err(Error::BadSpecifier)
    }
}

pub fn find_closest_pnp_manifest_path<P: AsRef<Path>>(p: P) -> Option<PathBuf> {
    let pnp_path = p.as_ref().join(".pnp.cjs");

    if pnp_path.exists() {
        Some(pnp_path)
    } else {
        if let Some(directory_path) = p.as_ref().parent() {
            find_closest_pnp_manifest_path(directory_path)
        } else {
            None
        }
    }
}

pub fn load_pnp_manifest<P: AsRef<Path>>(p: P) -> Result<Manifest, Error> {
    let manifest_content = std::fs::read_to_string(p.as_ref())?;

    lazy_static! {
        static ref RE: Regex = Regex::new("(const\\s+RAW_RUNTIME_STATE\\s*=\\s*|hydrateRuntimeState\\(JSON\\.parse\\()'").unwrap();
    }

    let manifest_match = RE.find(&manifest_content)?
        .expect("Should have been able to locate the runtime state payload offset");

    let iter = manifest_content.chars().skip(manifest_match.end());
    let mut json_string = String::default();
    let mut escaped = false;

    for c in iter {
        match c {
            '\'' if !escaped => {
                break;
            }
            '\\' if !escaped => {
                escaped = true;
            }
            _ => {
                escaped = false;
                json_string.push(c);
            }
        }
    }

    let mut manifest: Manifest = serde_json::from_str(&json_string.to_owned())?;
    init_pnp_manifest(&mut manifest, p.as_ref());

    Ok(manifest)
}

pub fn init_pnp_manifest<P: AsRef<Path>>(manifest: &mut Manifest, p: P) {
    manifest.manifest_dir = p.as_ref().parent()
        .expect("Should have a parent directory")
        .to_owned();

    for (name, ranges) in manifest.package_registry_data.iter_mut() {
        for (reference, info) in ranges.iter_mut() {
            let p = manifest.manifest_dir
                .join(info.package_location.clone());

            info.package_location = util::normalize_path(
                p.to_string_lossy(),
            );

            if !info.discard_from_lookup {
                manifest.location_trie.insert(info.package_location.clone(), PackageLocator {
                    name: name.clone(),
                    reference: reference.clone(),
                });
            }
        }
    }

    let top_level_pkg = manifest.package_registry_data
        .get("").expect("Assertion failed: Should have a top-level name key")
        .get("").expect("Assertion failed: Should have a top-level range key");

    for (name, dependency) in &top_level_pkg.package_dependencies {
        if let Entry::Vacant(entry) = manifest.fallback_pool.entry(name.clone()) {
            entry.insert(dependency.clone());
        }
    }
}

pub fn find_pnp_manifest(parent: &Path) -> Result<Option<Manifest>, Error> {
    find_closest_pnp_manifest_path(parent).map_or(Ok(None), |p| Ok(Some(load_pnp_manifest(&p)?)))
}

pub fn find_locator<'a, P: AsRef<Path>>(manifest: &'a Manifest, path: &P) -> Option<&'a PackageLocator> {
    let rel_path = pathdiff::diff_paths(path, &manifest.manifest_dir)
        .expect("Assertion failed: Provided path should be absolute");

    if let Some(regex) = &manifest.ignore_pattern_data {
        if regex.0.is_match(&util::normalize_path(rel_path.to_string_lossy())).unwrap() {
            return None
        }
    }

    let trie_key = util::normalize_path(
        path.as_ref().to_string_lossy(),
    );

    manifest.location_trie.get_ancestor_value(&trie_key)
}

pub fn get_package<'a>(manifest: &'a Manifest, locator: &PackageLocator) -> Result<&'a PackageInformation, Error> {
    let references = manifest.package_registry_data.get(&locator.name)
        .expect("Should have an entry in the package registry");

    let info = references.get(&locator.reference)
        .expect("Should have an entry in the package registry");

    Ok(info)
}

pub fn is_excluded_from_fallback(manifest: &Manifest, locator: &PackageLocator) -> bool {
    if let Some(references) = manifest.fallback_exclusion_list.get(&locator.name) {
        references.contains(&locator.reference)
    } else {
        false
    }
}

pub fn resolve_to_unqualified_via_manifest<P: AsRef<Path>>(manifest: &Manifest, specifier: &str, parent: P) -> Result<Resolution, Error> {
    let (ident, module_path) = parse_bare_identifier(specifier)?;

    if let Some(parent_locator) = find_locator(&manifest, &parent) {
        let parent_pkg = get_package(&manifest, parent_locator)?;

        let mut reference_or_alias: Option<PackageDependency> = None;
        let mut is_set = false;
        
        if !is_set {
            if let Some(Some(binding)) = parent_pkg.package_dependencies.get(&ident) {
                reference_or_alias = Some(binding.clone());
                is_set = true;
            }
        }

        if !is_set && manifest.enable_top_level_fallback && !is_excluded_from_fallback(&manifest, parent_locator) {
            if let Some(fallback_resolution) = manifest.fallback_pool.get(&ident) {
                reference_or_alias = fallback_resolution.clone();
                is_set = true;
            }
        }

        if !is_set {
            return Err(Error::FailedResolution);
        }

        if let Some(resolution) = reference_or_alias {
            let dependency_pkg = match resolution {
                PackageDependency::Reference(reference) => get_package(&manifest, &PackageLocator { name: ident, reference }),
                PackageDependency::Alias(name, reference) => get_package(&manifest, &PackageLocator { name, reference }),
            }?;

            Ok(Resolution::Package(PathBuf::from(dependency_pkg.package_location.clone()), module_path.clone()))
        } else {
            return Err(Error::FailedResolution);
        }
    } else {
        Ok(Resolution::Specifier(specifier.to_string()))
    }
}

pub fn resolve_to_unqualified<P: AsRef<Path>>(specifier: &str, parent: P, config: &ResolutionConfig) -> Result<Resolution, Error> {
    if let Some(manifest) = (config.host.find_pnp_manifest)(parent.as_ref())? {
        resolve_to_unqualified_via_manifest(&manifest, &specifier, &parent)
    } else {
        Ok(Resolution::Specifier(specifier.to_string()))
    }
}

#[cfg(test)]
mod lib_tests;
