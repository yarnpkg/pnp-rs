pub mod fs;

mod builtins;
mod error;
mod util;
mod zip;

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    path::{Path, PathBuf},
};

use fancy_regex::Regex;
use indexmap::IndexMap;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serde_with::{DefaultOnNull, serde_as};

use crate::util::RegexDef;

pub use crate::error::{
    BadSpecifier, Error, FailedManifestHydration, MissingDependency, MissingPeerDependency,
    UndeclaredDependency,
};

#[derive(Debug)]
pub enum Resolution {
    Resolved(PathBuf, Option<String>),
    Skipped,
}

pub struct ResolutionHost {
    #[allow(clippy::type_complexity)]
    pub find_pnp_manifest: Box<dyn Fn(&Path) -> Result<Option<Manifest>, Error>>,
}

impl Default for ResolutionHost {
    fn default() -> ResolutionHost {
        ResolutionHost { find_pnp_manifest: Box::new(find_pnp_manifest) }
    }
}

#[derive(Default)]
pub struct ResolutionConfig {
    pub host: ResolutionHost,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct PackageLocator {
    name: String,
    reference: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum PackageDependency {
    Reference(String),
    Alias(String, String),
}

#[serde_as]
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInformation {
    package_location: PathBuf,

    #[serde(default)]
    discard_from_lookup: bool,

    #[serde_as(as = "Vec<(_, Option<_>)>")]
    package_dependencies: HashMap<String, Option<PackageDependency>>,
}

#[serde_as]
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(skip_deserializing)]
    pub manifest_dir: PathBuf,

    #[serde(skip_deserializing)]
    pub manifest_path: PathBuf,

    #[serde(skip_deserializing)]
    location_trie: util::Trie<PackageLocator>,

    enable_top_level_fallback: bool,
    ignore_pattern_data: Option<RegexDef>,

    // dependencyTreeRoots: [{
    //   "name": "@app/monorepo",
    //   "workspace:."
    // }]
    dependency_tree_roots: HashSet<PackageLocator>,

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
    package_registry_data: HashMap<String, IndexMap<String, PackageInformation>>,
}

fn parse_scoped_package_name(specifier: &str) -> Option<(String, Option<String>)> {
    let mut segments = specifier.splitn(3, '/');

    let scope = segments.next()?;

    let name = segments.next()?;

    let package_name = specifier[..scope.len() + name.len() + 1].to_string();

    let subpath = segments.next().map(|v| v.to_string());

    Some((package_name, subpath))
}

fn parse_global_package_name(specifier: &str) -> Option<(String, Option<String>)> {
    let mut segments = specifier.splitn(2, '/');

    let name = segments.next()?;

    let package_name = name.to_string();

    let subpath = segments.next().map(|v| v.to_string());

    Some((package_name, subpath))
}

pub fn parse_bare_identifier(specifier: &str) -> Result<(String, Option<String>), Error> {
    let name = match specifier.starts_with("@") {
        true => parse_scoped_package_name(specifier),
        false => parse_global_package_name(specifier),
    };

    name.ok_or_else(|| {
        Error::BadSpecifier(Box::new(BadSpecifier {
            message: String::from("Invalid specifier"),
            specifier: specifier.to_string(),
        }))
    })
}

pub fn find_closest_pnp_manifest_path<P: AsRef<Path>>(p: P) -> Option<PathBuf> {
    let pnp_path = p.as_ref().join(".pnp.cjs");

    if pnp_path.exists() {
        Some(pnp_path)
    } else if let Some(directory_path) = p.as_ref().parent() {
        find_closest_pnp_manifest_path(directory_path)
    } else {
        None
    }
}

pub fn load_pnp_manifest<P: AsRef<Path>>(p: P) -> Result<Manifest, Error> {
    let manifest_content = std::fs::read_to_string(p.as_ref()).map_err(|err| {
        Error::FailedManifestHydration(Box::new(FailedManifestHydration {
            message: format!(
                "We failed to read the content of the manifest.\n\nOriginal error: {err}"
            ),
            manifest_path: p.as_ref().to_path_buf(),
        }))
    })?;

    lazy_static! {
        static ref RE: Regex = Regex::new(
            "(const[ \\n]+RAW_RUNTIME_STATE[ \\n]*=[ \\n]*|hydrateRuntimeState\\(JSON\\.parse\\()'"
        )
        .unwrap();
    }

    let manifest_match = RE.find(&manifest_content)
        .unwrap_or_default()
        .ok_or_else(|| Error::FailedManifestHydration(Box::new(FailedManifestHydration {
            message: String::from("We failed to locate the PnP data payload inside its manifest file. Did you manually edit the file?"),
            manifest_path: p.as_ref().to_path_buf(),
        })))?;

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

    let mut manifest: Manifest = serde_json::from_str(&json_string.to_owned())
        .map_err(|err| Error::FailedManifestHydration(Box::new(FailedManifestHydration {
            message: format!("We failed to parse the PnP data payload as proper JSON; Did you manually edit the file?\n\nOriginal error: {err}"),
            manifest_path: p.as_ref().to_path_buf(),
        })))?;

    init_pnp_manifest(&mut manifest, p.as_ref());

    Ok(manifest)
}

pub fn init_pnp_manifest<P: AsRef<Path>>(manifest: &mut Manifest, p: P) {
    manifest.manifest_path = p.as_ref().to_path_buf();

    manifest.manifest_dir = p.as_ref().parent().expect("Should have a parent directory").to_owned();

    for (name, ranges) in manifest.package_registry_data.iter_mut() {
        for (reference, info) in ranges.iter_mut() {
            let package_location = manifest.manifest_dir.join(info.package_location.clone());

            let normalized_location = util::normalize_path(package_location.to_string_lossy());

            info.package_location = PathBuf::from(normalized_location);

            if !info.discard_from_lookup {
                manifest.location_trie.insert(
                    &info.package_location,
                    PackageLocator { name: name.clone(), reference: reference.clone() },
                );
            }
        }
    }

    let top_level_pkg = manifest
        .package_registry_data
        .get("")
        .expect("Assertion failed: Should have a top-level name key")
        .get("")
        .expect("Assertion failed: Should have a top-level range key");

    for (name, dependency) in &top_level_pkg.package_dependencies {
        if let Entry::Vacant(entry) = manifest.fallback_pool.entry(name.clone()) {
            entry.insert(dependency.clone());
        }
    }
}

pub fn find_pnp_manifest(parent: &Path) -> Result<Option<Manifest>, Error> {
    find_closest_pnp_manifest_path(parent).map_or(Ok(None), |p| Ok(Some(load_pnp_manifest(p)?)))
}

pub fn is_dependency_tree_root<'a>(manifest: &'a Manifest, locator: &'a PackageLocator) -> bool {
    manifest.dependency_tree_roots.contains(locator)
}

pub fn find_locator<'a, P: AsRef<Path>>(
    manifest: &'a Manifest,
    path: &P,
) -> Option<&'a PackageLocator> {
    let rel_path = pathdiff::diff_paths(path, &manifest.manifest_dir)
        .expect("Assertion failed: Provided path should be absolute");

    if let Some(regex) = &manifest.ignore_pattern_data {
        if regex.0.is_match(&util::normalize_path(rel_path.to_string_lossy())).unwrap() {
            return None;
        }
    }

    manifest.location_trie.get_ancestor_value(&path)
}

pub fn get_package<'a>(
    manifest: &'a Manifest,
    locator: &PackageLocator,
) -> Result<&'a PackageInformation, Error> {
    let references = manifest
        .package_registry_data
        .get(&locator.name)
        .expect("Should have an entry in the package registry");

    let info =
        references.get(&locator.reference).expect("Should have an entry in the package registry");

    Ok(info)
}

pub fn is_excluded_from_fallback(manifest: &Manifest, locator: &PackageLocator) -> bool {
    if let Some(references) = manifest.fallback_exclusion_list.get(&locator.name) {
        references.contains(&locator.reference)
    } else {
        false
    }
}

pub fn find_broken_peer_dependencies(
    _dependency: &str,
    _initial_package: &PackageLocator,
) -> Vec<PackageLocator> {
    [].to_vec()
}

pub fn resolve_to_unqualified_via_manifest<P: AsRef<Path>>(
    manifest: &Manifest,
    specifier: &str,
    parent: P,
) -> Result<Resolution, Error> {
    let (ident, module_path) = parse_bare_identifier(specifier)?;

    if let Some(parent_locator) = find_locator(manifest, &parent) {
        let parent_pkg = get_package(manifest, parent_locator)?;

        let mut reference_or_alias: Option<PackageDependency> = None;
        let mut is_set = false;

        if !is_set {
            if let Some(Some(binding)) = parent_pkg.package_dependencies.get(&ident) {
                reference_or_alias = Some(binding.clone());
                is_set = true;
            }
        }

        if !is_set
            && manifest.enable_top_level_fallback
            && !is_excluded_from_fallback(manifest, parent_locator)
        {
            if let Some(fallback_resolution) = manifest.fallback_pool.get(&ident) {
                reference_or_alias = fallback_resolution.clone();
                is_set = true;
            }
        }

        if !is_set {
            let message = if builtins::is_nodejs_builtin(specifier) {
                if is_dependency_tree_root(manifest, parent_locator) {
                    format!(
                        "Your application tried to access {dependency_name}. While this module is usually interpreted as a Node builtin, your resolver is running inside a non-Node resolution context where such builtins are ignored. Since {dependency_name} isn't otherwise declared in your dependencies, this makes the require call ambiguous and unsound.\n\nRequired package: {dependency_name}{via}\nRequired by: ${issuer_path}",
                        dependency_name = &ident,
                        via = if ident != specifier {
                            format!(" (via \"{}\")", &specifier)
                        } else {
                            String::from("")
                        },
                        issuer_path = parent.as_ref().to_string_lossy(),
                    )
                } else {
                    format!(
                        "${issuer_locator_name} tried to access {dependency_name}. While this module is usually interpreted as a Node builtin, your resolver is running inside a non-Node resolution context where such builtins are ignored. Since {dependency_name} isn't otherwise declared in ${issuer_locator_name}'s dependencies, this makes the require call ambiguous and unsound.\n\nRequired package: {dependency_name}{via}\nRequired by: ${issuer_path}",
                        issuer_locator_name = &parent_locator.name,
                        dependency_name = &ident,
                        via = if ident != specifier {
                            format!(" (via \"{}\")", &specifier)
                        } else {
                            String::from("")
                        },
                        issuer_path = parent.as_ref().to_string_lossy(),
                    )
                }
            } else if is_dependency_tree_root(manifest, parent_locator) {
                format!(
                    "Your application tried to access {dependency_name}, but it isn't declared in your dependencies; this makes the require call ambiguous and unsound.\n\nRequired package: {dependency_name}{via}\nRequired by: {issuer_path}",
                    dependency_name = &ident,
                    via = if ident != specifier {
                        format!(" (via \"{}\")", &specifier)
                    } else {
                        String::from("")
                    },
                    issuer_path = parent.as_ref().to_string_lossy(),
                )
            } else {
                format!(
                    "{issuer_locator_name} tried to access {dependency_name}, but it isn't declared in its dependencies; this makes the require call ambiguous and unsound.\n\nRequired package: {dependency_name}{via}\nRequired by: {issuer_locator_name}@{issuer_locator_reference} (via {issuer_path})",
                    issuer_locator_name = &parent_locator.name,
                    issuer_locator_reference = &parent_locator.reference,
                    dependency_name = &ident,
                    via = if ident != specifier {
                        format!(" (via \"{}\")", &specifier)
                    } else {
                        String::from("")
                    },
                    issuer_path = parent.as_ref().to_string_lossy(),
                )
            };

            return Err(Error::UndeclaredDependency(Box::new(UndeclaredDependency {
                message,
                request: specifier.to_string(),
                dependency_name: ident,
                issuer_locator: parent_locator.clone(),
                issuer_path: parent.as_ref().to_path_buf(),
            })));
        }

        if let Some(resolution) = reference_or_alias {
            let dependency_pkg = match resolution {
                PackageDependency::Reference(reference) => {
                    get_package(manifest, &PackageLocator { name: ident, reference })
                }
                PackageDependency::Alias(name, reference) => {
                    get_package(manifest, &PackageLocator { name, reference })
                }
            }?;

            Ok(Resolution::Resolved(dependency_pkg.package_location.clone(), module_path))
        } else {
            let broken_ancestors = find_broken_peer_dependencies(specifier, parent_locator);

            let message = if is_dependency_tree_root(manifest, parent_locator) {
                format!(
                    "Your application tried to access {dependency_name} (a peer dependency); this isn't allowed as there is no ancestor to satisfy the requirement. Use a devDependency if needed.\n\nRequired package: {dependency_name}{via}\nRequired by: {issuer_path}",
                    dependency_name = &ident,
                    via = if ident != specifier {
                        format!(" (via \"{}\")", &specifier)
                    } else {
                        String::from("")
                    },
                    issuer_path = parent.as_ref().to_string_lossy(),
                )
            } else if !broken_ancestors.is_empty()
                && broken_ancestors.iter().all(|locator| is_dependency_tree_root(manifest, locator))
            {
                format!(
                    "{issuer_locator_name} tried to access {dependency_name} (a peer dependency) but it isn't provided by your application; this makes the require call ambiguous and unsound.\n\nRequired package: {dependency_name}{via}\nRequired by: {issuer_locator_name}@{issuer_locator_reference} (via {issuer_path})",
                    issuer_locator_name = &parent_locator.name,
                    issuer_locator_reference = &parent_locator.reference,
                    dependency_name = &ident,
                    via = if ident != specifier {
                        format!(" (via \"{}\")", &specifier)
                    } else {
                        String::from("")
                    },
                    issuer_path = parent.as_ref().to_string_lossy(),
                )
            } else {
                format!(
                    "{issuer_locator_name} tried to access {dependency_name} (a peer dependency) but it isn't provided by its ancestors; this makes the require call ambiguous and unsound.\n\nRequired package: {dependency_name}{via}\nRequired by: {issuer_locator_name}@{issuer_locator_reference} (via {issuer_path})",
                    issuer_locator_name = &parent_locator.name,
                    issuer_locator_reference = &parent_locator.reference,
                    dependency_name = &ident,
                    via = if ident != specifier {
                        format!(" (via \"{}\")", &specifier)
                    } else {
                        String::from("")
                    },
                    issuer_path = parent.as_ref().to_string_lossy(),
                )
            };

            Err(Error::MissingPeerDependency(Box::new(MissingPeerDependency {
                message,
                request: specifier.to_string(),
                dependency_name: ident,
                issuer_locator: parent_locator.clone(),
                issuer_path: parent.as_ref().to_path_buf(),
                broken_ancestors: [].to_vec(),
            })))
        }
    } else {
        Ok(Resolution::Skipped)
    }
}

pub fn resolve_to_unqualified<P: AsRef<Path>>(
    specifier: &str,
    parent: P,
    config: &ResolutionConfig,
) -> Result<Resolution, Error> {
    if let Some(manifest) = (config.host.find_pnp_manifest)(parent.as_ref())? {
        resolve_to_unqualified_via_manifest(&manifest, specifier, &parent)
    } else {
        Ok(Resolution::Skipped)
    }
}

#[cfg(test)]
mod lib_tests;
