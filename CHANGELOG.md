# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.12.2](https://github.com/yarnpkg/pnp-rs/compare/v0.12.1...v0.12.2) - 2025-08-25

### Other

- *(deps)* lock file maintenance rust crates ([#45](https://github.com/yarnpkg/pnp-rs/pull/45))
- add recived path into panic info ([#46](https://github.com/yarnpkg/pnp-rs/pull/46))
- *(deps)* update dependency rust to v1.89.0 ([#43](https://github.com/yarnpkg/pnp-rs/pull/43))

## [0.12.1](https://github.com/yarnpkg/pnp-rs/compare/v0.12.0...v0.12.1) - 2025-07-10

### Other

- add renovate bot ([#39](https://github.com/yarnpkg/pnp-rs/pull/39))
- Improves performances ([#42](https://github.com/yarnpkg/pnp-rs/pull/42))
- Adds a benchmark workflow ([#40](https://github.com/yarnpkg/pnp-rs/pull/40))

## [0.12.0](https://github.com/yarnpkg/pnp-rs/compare/v0.11.0...v0.12.0) - 2025-07-10

### Other

- remove `AsRef<Path>` from functions ([#38](https://github.com/yarnpkg/pnp-rs/pull/38))
- change `find_closest_pnp_manifest_path` from recursion to a loop ([#35](https://github.com/yarnpkg/pnp-rs/pull/35))

## [0.11.0](https://github.com/yarnpkg/pnp-rs/compare/v0.10.0...v0.11.0) - 2025-07-01

### Fixed

- fix windows failure ([#22](https://github.com/yarnpkg/pnp-rs/pull/22))

### Other

- add release-plz.yml ([#24](https://github.com/yarnpkg/pnp-rs/pull/24))
- remove indexmap
- remove `serde_with` ([#32](https://github.com/yarnpkg/pnp-rs/pull/32))
- remove the unused `Serialize` on `PackageLocator` ([#31](https://github.com/yarnpkg/pnp-rs/pull/31))
- bump deps ([#30](https://github.com/yarnpkg/pnp-rs/pull/30))
- use fxhash in zip data structures ([#28](https://github.com/yarnpkg/pnp-rs/pull/28))
- remove the `lazy_static` crate ([#27](https://github.com/yarnpkg/pnp-rs/pull/27))
- improve `NODEJS_BUILTINS` ([#26](https://github.com/yarnpkg/pnp-rs/pull/26))
- remove unnecessary derive `Serialize` on `Error` ([#25](https://github.com/yarnpkg/pnp-rs/pull/25))
- use fxhash ([#23](https://github.com/yarnpkg/pnp-rs/pull/23))
- `clippy::result_large_err` for the `Error` type ([#21](https://github.com/yarnpkg/pnp-rs/pull/21))
- run `cargo clippy --fix` + manual fixes ([#20](https://github.com/yarnpkg/pnp-rs/pull/20))
- run `cargo fmt` ([#19](https://github.com/yarnpkg/pnp-rs/pull/19))
- add `cargo check` and `cargo test --all-features` ([#18](https://github.com/yarnpkg/pnp-rs/pull/18))
- add rust-toolchain.toml ([#17](https://github.com/yarnpkg/pnp-rs/pull/17))
- disable more
- enable most tests on windows CI
