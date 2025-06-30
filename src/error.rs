use std::path::PathBuf;

use thiserror::Error;

use crate::PackageLocator;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum Error {
    #[error(transparent)]
    BadSpecifier(Box<BadSpecifier>),

    #[error(transparent)]
    FailedManifestHydration(Box<FailedManifestHydration>),

    #[error(transparent)]
    MissingPeerDependency(Box<MissingPeerDependency>),

    #[error(transparent)]
    UndeclaredDependency(Box<UndeclaredDependency>),

    #[error(transparent)]
    MissingDependency(Box<MissingDependency>),
}

#[derive(Debug, Clone, PartialEq, Error)]
#[error("{message}")]
pub struct BadSpecifier {
    pub message: String,
    pub specifier: String,
}

#[derive(Debug, Clone, PartialEq, Error)]
#[error("{message}")]
pub struct FailedManifestHydration {
    pub message: String,
    pub manifest_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Error)]
#[error("{message}")]
pub struct MissingPeerDependency {
    pub message: String,
    pub request: String,

    pub dependency_name: String,

    pub issuer_locator: PackageLocator,
    pub issuer_path: PathBuf,

    pub broken_ancestors: Vec<PackageLocator>,
}

#[derive(Debug, Clone, PartialEq, Error)]
#[error("{message}")]
pub struct UndeclaredDependency {
    pub message: String,
    pub request: String,

    pub dependency_name: String,

    pub issuer_locator: PackageLocator,
    pub issuer_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Error)]
#[error("{message}")]
pub struct MissingDependency {
    pub message: String,
    pub request: String,

    pub dependency_locator: PackageLocator,
    pub dependency_name: String,

    pub issuer_locator: PackageLocator,
    pub issuer_path: PathBuf,
}
