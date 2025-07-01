use std::{hash::BuildHasherDefault, path::PathBuf};

use indexmap::IndexMap;
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use serde::{Deserialize, de::Deserializer};

use crate::util::{RegexDef, Trie};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(skip_deserializing)]
    pub manifest_dir: PathBuf,

    #[serde(skip_deserializing)]
    pub manifest_path: PathBuf,

    #[serde(skip_deserializing)]
    pub location_trie: Trie<PackageLocator>,

    pub enable_top_level_fallback: bool,
    pub ignore_pattern_data: Option<RegexDef>,

    // dependencyTreeRoots: [{
    //   "name": "@app/monorepo",
    //   "workspace:."
    // }]
    pub dependency_tree_roots: FxHashSet<PackageLocator>,

    // fallbackPool: [[
    //   "@app/monorepo",
    //   "workspace:.",
    // ]]
    #[serde(deserialize_with = "deserialize_package_dependencies")]
    pub fallback_pool: FxHashMap<String, Option<PackageDependency>>,

    // fallbackExclusionList: [[
    //   "@app/server",
    //  ["workspace:sources/server"],
    // ]]
    #[serde(deserialize_with = "deserialize_fallback_exclusion_list")]
    pub fallback_exclusion_list: FxHashMap<String, FxHashSet<String>>,

    // packageRegistryData: [
    //   [null, [
    //     [null, {
    //       ...
    //     }]
    //   }]
    // ]
    #[serde(deserialize_with = "deserialize_package_registry_data")]
    pub package_registry_data:
        FxHashMap<String, IndexMap<String, PackageInformation, BuildHasherDefault<FxHasher>>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash, Deserialize)]
pub struct PackageLocator {
    pub name: String,
    pub reference: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInformation {
    pub package_location: PathBuf,

    #[serde(default)]
    pub discard_from_lookup: bool,

    #[serde(deserialize_with = "deserialize_package_dependencies")]
    pub package_dependencies: FxHashMap<String, Option<PackageDependency>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum PackageDependency {
    Reference(String),
    Alias(String, String),
}

fn deserialize_fallback_exclusion_list<'de, D>(
    deserializer: D,
) -> Result<FxHashMap<String, FxHashSet<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Debug, Deserialize)]
    struct Item(String, FxHashSet<String>);

    let mut map = FxHashMap::default();
    for item in Vec::<Item>::deserialize(deserializer)? {
        map.insert(item.0, item.1);
    }
    Ok(map)
}

fn deserialize_package_dependencies<'de, D>(
    deserializer: D,
) -> Result<FxHashMap<String, Option<PackageDependency>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Debug, Deserialize)]
    struct Item(String, Option<PackageDependency>);

    let mut map = FxHashMap::default();
    for item in Vec::<Item>::deserialize(deserializer)? {
        map.insert(item.0, item.1);
    }
    Ok(map)
}

#[expect(clippy::type_complexity)]
fn deserialize_package_registry_data<'de, D>(
    deserializer: D,
) -> Result<
    FxHashMap<String, IndexMap<String, PackageInformation, BuildHasherDefault<FxHasher>>>,
    D::Error,
>
where
    D: Deserializer<'de>,
{
    #[derive(Debug, Deserialize)]
    struct Item(Option<String>, Vec<(Option<String>, PackageInformation)>);

    let mut map = FxHashMap::default();
    for item in Vec::<Item>::deserialize(deserializer)? {
        let key = item.0.unwrap_or_else(|| "".to_string());
        let value = IndexMap::from_iter(
            item.1.into_iter().map(|(k, v)| (k.unwrap_or_else(|| "".to_string()), v)),
        );
        map.insert(key, value);
    }
    Ok(map)
}
