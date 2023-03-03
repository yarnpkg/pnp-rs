use std::{path::{Path, PathBuf, Component}, fs, collections::{HashSet, HashMap}, env, borrow::Cow};
use lazy_static::lazy_static;
use radix_trie::Trie;
use fancy_regex::Regex;
use serde::{de::Error, Deserialize, Deserializer};
use serde_with::{serde_as, DefaultOnNull};
use simple_error::{self, bail, SimpleError};

enum Resolution {
    Specifier(String),
    Path(PathBuf),
}

struct PnpResolutionHost {
    find_pnp_manifest: Box<dyn Fn(&Path) -> Result<Option<Manifest>, Box<dyn std::error::Error>>>,
}

impl Default for PnpResolutionHost {
    fn default() -> PnpResolutionHost {
        PnpResolutionHost {
            find_pnp_manifest: Box::new(find_pnp_manifest),
        }
    }
}

#[derive(Default)]
struct PnpResolutionConfig {
    builtins: HashSet<String>,
    host: PnpResolutionHost,
}



#[derive(Deserialize)]
struct PackageLocator {
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
struct PackageInformation {
    package_location: PathBuf,

    #[serde(default)]
    discard_from_lookup: bool,

    #[serde_as(as = "Vec<(_, Option<_>)>")]
    package_dependencies: HashMap<String, Option<PackageDependency>>,
}

fn strip_slash_escape(str: &str) -> String {
    let mut res = String::default();
    res.reserve_exact(str.len());

    let mut iter = str.chars().peekable();
    let mut escaped = false;

    while let Some(c) = iter.next() {
        if !escaped && c == '\\' {
            if iter.peek() == Some(&'/') {
                continue;
            }

            escaped = true;
        }

        res.push(c);
        escaped = false;
    }

    res
}

#[derive(Debug)]
struct RegexDef(Regex);

impl<'de> Deserialize<'de> for RegexDef {
    fn deserialize<D>(d: D) -> Result<RegexDef, D::Error>
    where D: Deserializer<'de>,
    {
        let s = <Cow<str>>::deserialize(d)?;

        match strip_slash_escape(s.as_ref()).parse() {
            Ok(regex) => Ok(RegexDef(regex)),
            Err(err) => Err(D::Error::custom(err)),
        }
    }
}

#[serde_as]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    enable_top_level_fallback: bool,

    ignore_pattern_data: Option<RegexDef>,

    #[serde(skip_deserializing)]
    fallback_dependencies: HashMap<String, Option<PackageDependency>>,

    #[serde(skip_deserializing)]
    location_trie: Trie<PathBuf, PackageLocator>,

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

fn is_builtin(specifier: &str, config: &PnpResolutionConfig) -> bool {
    config.builtins.contains(specifier)
}

fn is_path_specifier(specifier: &str) -> bool {
    lazy_static! {
        static ref RE: Regex = Regex::new("^\\.{0,2}/").unwrap();
    }

    RE.is_match(specifier).unwrap()
}

fn parse_bare_identifier(specifier: &str) -> Result<(String, Option<String>), SimpleError> {
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

fn load_pnp_manifest(p: &Path) -> Result<Manifest, Box<dyn std::error::Error>> {
    let manifest_content = fs::read_to_string(p)?;
    let manifest_dir = p.parent()
        .expect("Should have a parent directory");

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

    Ok(manifest)
}

fn find_pnp_manifest(parent: &Path) -> Result<Option<Manifest>, Box<dyn std::error::Error>> {
    find_closest_pnp_manifest_path(parent).map_or(Ok(None), |p| Ok(Some(load_pnp_manifest(&p)?)))
}

fn find_locator<'a>(manifest: &'a Manifest, path: &Path) -> Option<&'a PackageLocator> {
    manifest.location_trie.get_ancestor_value(path)
}

fn get_package<'a>(manifest: &'a Manifest, locator: &PackageLocator) -> Result<&'a PackageInformation, Box<dyn std::error::Error>> {
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

fn pnp_resolve(specifier: &str, parent: &Path, config: &PnpResolutionConfig) -> Result<Resolution, Box<dyn std::error::Error>> {
    if is_builtin(specifier, config) {
        return Ok(Resolution::Specifier(specifier.to_string()))
    }

    if is_path_specifier(specifier) {
        return Ok(Resolution::Specifier(specifier.to_string()))
    }

    resolve_to_unqualified(specifier, parent, config)
}

fn resolve_to_unqualified(specifier: &str, parent: &Path, config: &PnpResolutionConfig) -> Result<Resolution, Box<dyn std::error::Error>> {
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

fn main() {
    let mut args = env::args();

    // Skip the program name
    args.next();

    let specifier = args.next()
        .expect("A specifier must be provided");

    let parent = args.next()
        .map(PathBuf::from)
        .expect("A parent url must be provided");

    println!("specifier = {}", specifier);
    println!("parent    = {:?}", parent);

    let resolution = pnp_resolve(&specifier, &parent, &PnpResolutionConfig {
        ..Default::default()
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
