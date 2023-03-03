mod util;

use std::{path::{Path, PathBuf, Component}, fs, collections::{HashSet, HashMap}};
use crate::util::RegexDef;
use lazy_static::lazy_static;
use radix_trie::Trie;
use fancy_regex::Regex;
use serde::Deserialize;
use serde_with::{serde_as, DefaultOnNull};
use simple_error::{self, bail, SimpleError};

pub enum Resolution {
    Specifier(String),
    Path(PathBuf),
}

pub struct PnpResolutionHost {
    pub find_pnp_manifest: Box<dyn Fn(&Path) -> Result<Option<Manifest>, Box<dyn std::error::Error>>>,
}

impl Default for PnpResolutionHost {
    fn default() -> PnpResolutionHost {
        PnpResolutionHost {
            find_pnp_manifest: Box::new(find_pnp_manifest),
        }
    }
}

#[derive(Default)]
pub struct PnpResolutionConfig {
    pub builtins: HashSet<String>,
    pub host: PnpResolutionHost,
}

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
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInformation {
    package_location: PathBuf,

    #[serde(default)]
    discard_from_lookup: bool,

    #[serde_as(as = "Vec<(_, Option<_>)>")]
    package_dependencies: HashMap<String, Option<PackageDependency>>,
}

#[serde_as]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(skip_deserializing)]
    manifest_path: PathBuf,

    #[serde(skip_deserializing)]
    fallback_dependencies: HashMap<String, Option<PackageDependency>>,

    #[serde(skip_deserializing)]
    location_trie: Trie<PathBuf, PackageLocator>,

    enable_top_level_fallback: bool,
    ignore_pattern_data: Option<RegexDef>,

    // fallbackPool: [[
    //   "@app/monorepo",
    //   "workspace:.",
    // ]]
    fallback_pool: Vec<PackageLocator>,

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

fn normalize_path(path: &Path) -> PathBuf {
    let mut components = path.components().peekable();
    let mut ret = if let Some(c @ Component::Prefix(..)) = components.peek().cloned() {
        components.next();
        PathBuf::from(c.as_os_str())
    } else {
        PathBuf::new()
    };

    for component in components {
        match component {
            Component::Prefix(..) => unreachable!(),
            Component::RootDir => {
                ret.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                ret.pop();
            }
            Component::Normal(c) => {
                ret.push(c);
            }
        }
    }
    ret
}

pub fn is_builtin(specifier: &str, config: &PnpResolutionConfig) -> bool {
    config.builtins.contains(specifier)
}

pub fn is_path_specifier(specifier: &str) -> bool {
    lazy_static! {
        static ref RE: Regex = Regex::new("^\\.{0,2}/").unwrap();
    }

    RE.is_match(specifier).unwrap()
}

pub fn parse_bare_identifier(specifier: &str) -> Result<(String, Option<String>), SimpleError> {
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
        bail!("Invalid specifier")
    }
}

pub fn find_closest_pnp_manifest_path(p: &Path) -> Option<PathBuf> {
    if let Some(directory_path) = p.parent() {
        let pnp_path = directory_path.join(".pnp.cjs");

        if pnp_path.exists() {
            Some(pnp_path)
        } else {
            find_closest_pnp_manifest_path(directory_path)
        }
    } else {
        None
    }
}

pub fn load_pnp_manifest(p: &Path) -> Result<Manifest, Box<dyn std::error::Error>> {
    let manifest_content = fs::read_to_string(p)?;

    lazy_static! {
        static ref RE: Regex = Regex::new("const\\s+RAW_RUNTIME_STATE\\s*=\\s*'").unwrap();
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
    init_pnp_manifest(&mut manifest, p);

    Ok(manifest)
}

pub fn init_pnp_manifest(manifest: &mut Manifest, p: &Path) {
    let manifest_dir = p.parent()
        .expect("Should have a parent directory");

    manifest.manifest_path = p.to_owned();

    for locator in manifest.fallback_pool.iter() {
        let info = manifest.package_registry_data
            .get(&locator.name)
                .expect("Assertion failed: The locator should be registered")
            .get(&locator.reference)
                .expect("Assertion failed: The locator should be registered");

        for (name, dependency) in info.package_dependencies.iter() {
            manifest.fallback_dependencies.insert(name.clone(), dependency.clone());
        }
    }

    for (name, ranges) in manifest.package_registry_data.iter_mut() {
        for (reference, info) in ranges.iter_mut() {
            if info.discard_from_lookup {
                continue;
            }

            info.package_location = normalize_path(manifest_dir
                .join(info.package_location.clone())
                .as_path());

            manifest.location_trie.insert(info.package_location.clone(), PackageLocator {
                name: name.clone(),
                reference: reference.clone(),
            });
        }
    }
}

pub fn find_pnp_manifest(parent: &Path) -> Result<Option<Manifest>, Box<dyn std::error::Error>> {
    find_closest_pnp_manifest_path(parent).map_or(Ok(None), |p| Ok(Some(load_pnp_manifest(&p)?)))
}

pub fn find_locator<'a>(manifest: &'a Manifest, path: &Path) -> Option<&'a PackageLocator> {
    let relative_path = pathdiff::diff_paths(path, &manifest.manifest_path)
        .map_or(String::from("."), |p| p.to_string_lossy().to_string());

    if let Some(regex) = &manifest.ignore_pattern_data {
        if regex.0.is_match(&relative_path).unwrap() {
            return None
        }
    }

    manifest.location_trie.get_ancestor_value(path)
}

pub fn get_package<'a>(manifest: &'a Manifest, locator: &PackageLocator) -> Result<&'a PackageInformation, Box<dyn std::error::Error>> {
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

pub fn pnp_resolve(specifier: &str, parent: &Path, config: &PnpResolutionConfig) -> Result<Resolution, Box<dyn std::error::Error>> {
    if is_builtin(specifier, config) {
        return Ok(Resolution::Specifier(specifier.to_string()))
    }

    if is_path_specifier(specifier) {
        return Ok(Resolution::Specifier(specifier.to_string()))
    }

    resolve_to_unqualified(specifier, parent, config)
}

pub fn resolve_to_unqualified(specifier: &str, parent: &Path, config: &PnpResolutionConfig) -> Result<Resolution, Box<dyn std::error::Error>> {
    let (ident, module_path) = parse_bare_identifier(specifier)?;

    if let Some(manifest) = (config.host.find_pnp_manifest)(parent)? {
        if let Some(parent_locator) = find_locator(&manifest, parent) {
            let parent_pkg = get_package(&manifest, parent_locator)?;

            let mut reference_or_alias: Option<PackageDependency> = None;
            let mut is_set = false;
            
            if !is_set {
                if let Some(binding) = parent_pkg.package_dependencies.get(&ident) {
                    reference_or_alias = binding.clone();
                    is_set = true;
                }
            }

            if !is_set && manifest.enable_top_level_fallback && !is_excluded_from_fallback(&manifest, parent_locator) {
                if let Some(fallback_resolution) = manifest.fallback_dependencies.get(&ident) {
                    reference_or_alias = fallback_resolution.clone();
                    is_set = true;
                }
            }

            if !is_set {
                bail!("Resolution failed");
            }

            if let Some(resolution) = reference_or_alias {
                let dependency_pkg = match resolution {
                    PackageDependency::Reference(reference) => get_package(&manifest, &PackageLocator { name: ident, reference }),
                    PackageDependency::Alias(name, reference) => get_package(&manifest, &PackageLocator { name, reference }),
                }?;

                let final_path = dependency_pkg.package_location
                    .join(module_path.unwrap_or_default());

                Ok(Resolution::Path(final_path))
            } else {
                bail!("Resolution failed: Unsatisfied peer dependency");
            }
        } else {
            Ok(Resolution::Specifier(specifier.to_string()))
        }
    } else {
        Ok(Resolution::Specifier(specifier.to_string()))
    }
}
