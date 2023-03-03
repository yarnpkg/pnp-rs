use std::{error::Error, path::{Path, PathBuf}, fs, collections::{HashSet, HashMap}, env};
use lazy_static::lazy_static;
use radix_trie::Trie;
use regex::Regex;
use serde::Deserialize;
use serde::Deserializer;
use serde_with::{serde_as, DeserializeAs, de::DeserializeAsWrap, OneOrMany};
use simple_error::{self, bail, SimpleError};

enum Resolution {
    Specifier(String),
    Path(PathBuf),
}

struct PnpResolutionHost {
//    find_pnp_manifest: Box<dyn FnMut()>,
}

struct PnpResolutionConfig {
    host: PnpResolutionHost,
}

#[derive(Deserialize)]
struct PackageLocator {
    name: String,
    reference: String,
}

#[serde_as]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageInformation {
    package_location: PathBuf,

    #[serde(default)]
    discard_from_lookup: bool,

    #[serde_as(as = "Vec<(_, Option<OneOrMany<_>>)>")]
    package_dependencies: HashMap<String, Option<Vec<String>>>,
}

fn deserialize_maybe_null_string<'de, D>(deserializer: D) -> Result<String, D::Error> where D: Deserializer<'de> {
    let buf = String::deserialize(deserializer)?;

    Ok(buf)
}

#[serde_as]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    enable_top_level_fallback: bool,

    #[serde(with = "serde_regex")]
    ignore_pattern_data: Option<Regex>,

    #[serde(skip_deserializing)]
    location_trie: Trie<PathBuf, PackageLocator>,

    // dependencyTreeRoots: [{
    //   name: "@app/monorepo",
    //   reference: "workspace:.",
    // }, {
    //   name: "@app/website",
    //   reference: "workspace:website",
    // }]
    //
    dependency_tree_roots: Vec<PackageLocator>,

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
    #[serde_as(as = "Vec<(_, Vec<(_, _)>)>")]
    package_registry_data: HashMap<String, HashMap<String, PackageInformation>>,
}

fn is_node_builtin(specifier: &String) -> bool {
    specifier.starts_with("node:")
}

fn is_path_specifier(specifier: &String) -> bool {
    lazy_static! {
        static ref RE: Regex = Regex::new("^\\.{0,2}/").unwrap();
    }

    RE.is_match(&specifier)
}

fn parse_bare_identifier(specifier: &String) -> Result<(String, Option<String>), SimpleError> {
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

fn find_closest_pnp_manifest_path(p: &Path) -> Option<PathBuf> {
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

fn load_pnp_manifest(p: &Path) -> Result<Manifest, Box<dyn Error>> {
    let manifest_content = fs::read_to_string(p)?;
    let manifest_dir = p.parent()
        .expect("Should have a parent directory");

    lazy_static! {
        static ref RE: Regex = Regex::new("const\\s+RAW_RUNTIME_STATE\\s*=\\s*'").unwrap();
    }

    let manifest_match = RE.find(&manifest_content)
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

    for (name, ranges) in manifest.package_registry_data.iter_mut() {
        for (reference, info) in ranges.iter_mut() {
            if info.discard_from_lookup {
                continue;
            }

            info.package_location = manifest_dir
                .join(info.package_location.clone());

            manifest.location_trie.insert(info.package_location.clone(), PackageLocator {
                name: name.clone(),
                reference: reference.clone(),
            });
        }
    }

    Ok(manifest)
}

fn find_pnp_manifest(parent: &Path) -> Result<Option<Manifest>, Box<dyn Error>> {
    find_closest_pnp_manifest_path(parent).map_or(Ok(None), |p| Ok(Some(load_pnp_manifest(&*p)?)))
}

fn find_locator<'a>(manifest: &'a Manifest, path: &Path) -> Option<&'a PackageLocator> {
    manifest.location_trie.get_ancestor_value(path)
}

fn get_package<'a>(manifest: &'a Manifest, locator: &PackageLocator) -> Result<&'a PackageInformation, Box<dyn Error>> {
    let references = manifest.package_registry_data.get(&locator.name)
        .expect("Should have an entry in the package registry");

    let info = references.get(&locator.reference)
        .expect("Should have an entry in the package registry");

    Ok(info)
}

fn is_excluded_from_fallback(manifest: &Manifest, locator: &PackageLocator) -> bool {
    if let Some(references) = manifest.fallback_exclusion_list.get(&locator.name) {
        references.contains(&locator.reference)
    } else {
        false
    }
}

fn pnp_resolve(specifier: &String, parent: &Path, config: &PnpResolutionConfig) -> Result<Resolution, Box<dyn Error>> {
    if is_node_builtin(&specifier) {
        return Ok(Resolution::Specifier(specifier.clone()))
    }

    if is_path_specifier(&specifier) {
        return Ok(Resolution::Specifier(specifier.clone()))
    }

    resolve_to_unqualified(&specifier, &parent, config)
}

fn get_dependency_from_fallback(manifest: &Manifest, ident: &String) -> Option<Vec<String>> {
    None
}

fn resolve_to_unqualified(specifier: &String, parent: &Path, config: &PnpResolutionConfig) -> Result<Resolution, Box<dyn Error>> {
    let (ident, module_path) = parse_bare_identifier(specifier)?;

    if let Some(manifest) = find_pnp_manifest(parent)? {
        if let Some(parent_locator) = find_locator(&manifest, parent) {
            let parent_pkg = get_package(&manifest, &parent_locator)?;

            let mut reference_or_alias: Option<Vec<String>> = None;
            let mut is_set = false;
            
            if !is_set {
                if let Some(binding) = parent_pkg.package_dependencies.get(&ident) {
                    reference_or_alias = binding.clone();
                    is_set = true;
                }
            }

            if !is_set {
                if manifest.enable_top_level_fallback {
                    if !is_excluded_from_fallback(&manifest, &parent_locator) {
                        if let Some(fallback_resolution) = get_dependency_from_fallback(&manifest, &ident) {
                            reference_or_alias = Some(fallback_resolution);
                            is_set = true;
                        }
                    }
                }
            }

            if !is_set {
                bail!("Resolution failed");
            }

            if let Some(resolution) = reference_or_alias {
                let dependency_pkg = match resolution.as_slice() {
                    [reference] => get_package(&manifest, &PackageLocator { name: ident, reference: reference.clone() }),
                    [name, reference] => get_package(&manifest, &PackageLocator { name: name.clone(), reference: reference.clone() }),
                    _ => bail!("Invalid amount of elements"),
                }?;

                let final_path = dependency_pkg.package_location
                    .join(module_path.unwrap_or_default());

                Ok(Resolution::Path(final_path))
            } else {
                bail!("Resolution failed: Unsatisfied peer dependency");
            }
        } else {
            Ok(Resolution::Specifier(specifier.clone()))
        }
    } else {
        Ok(Resolution::Specifier(specifier.clone()))
    }
}

fn main() {
    let mut args = env::args();

    // Skip the program name
    args.next();

    let specifier = args.next()
        .expect("A specifier must be provided");

    let parent = args.next()
        .map(|p| PathBuf::from(p))
        .expect("A parent url must be provided");

    println!("specifier = {}", specifier);
    println!("parent    = {:?}", parent);

    let resolution = pnp_resolve(&specifier, &parent, &PnpResolutionConfig {
        host: PnpResolutionHost {
        },
    });

    match resolution {
        Ok(res) => {
            match res {
                Resolution::Path(p) => {
                    println!("result    = Path ({:?})", p);
                }
                Resolution::Specifier(specifier) => {
                    println!("result    = Specifier ({})", specifier);
                }
            }
        }
        Err(err) => {
            println!("error     = {}", err);
        }
    }
}
