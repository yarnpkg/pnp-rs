use serde::Deserialize;

use crate::{PnpResolutionConfig, Resolution, Manifest};

#[derive(Deserialize)]
struct Test {
    it: String,
    imported: String,
    importer: String,
    expected: String,
}

#[derive(Deserialize)]
struct TestSuite {
    manifest: Manifest,
    tests: Vec<Test>,
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crate::{init_pnp_manifest, resolve_to_unqualified, PnpResolutionHost};

    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn test_resolve_unqualified() {
        let expectations_path = std::env::current_dir()
            .expect("Assertion failed: Expected a valid current working directory")
            .join("testExpectations.json");

        let manifest_content = fs::read_to_string(&expectations_path)
            .expect("Assertion failed: Expected the expectations to be found");

        let mut test_suites: Vec<TestSuite> = serde_json::from_str(&manifest_content)
            .expect("Assertion failed: Expected the expectations to be loaded");

        for test_suite in test_suites.iter_mut() {
            let manifest = &mut test_suite.manifest;
            init_pnp_manifest(manifest, &PathBuf::from("/path/to/project/.pnp.cjs"));

            for test in test_suite.tests.iter() {
                let specifier = &test.imported;
                let parent = &PathBuf::from(&test.importer).join("fooo");

                let manifest_copy = manifest.clone();

                let host = PnpResolutionHost {
                    find_pnp_manifest: Box::new(move |_| Ok(Some(manifest_copy.clone()))),
                    ..Default::default()
                };

                let config = PnpResolutionConfig {
                    host,
                    ..Default::default()
                };

                let resolution = resolve_to_unqualified(specifier, parent, &config);

                match resolution {
                    Ok(Resolution::Path(path)) => {
                        assert_eq!(path.to_string_lossy(), test.expected, "{}", test.it);
                    },
                    Ok(Resolution::Specifier(specifier)) => {
                        assert_eq!(specifier, test.expected, "{}", test.it);
                    },
                    Err(err) => {
                        assert_eq!(test.expected, "error!", "{}: {}", test.it, err);
                    },
                }
                
            }
        }
    }
}
