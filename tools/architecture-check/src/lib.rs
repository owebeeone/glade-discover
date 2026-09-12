//! Fast source/dependency architecture lint. Rust compilation and behavioral
//! conformance tests remain necessary; this is not a semantic type checker.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use syn::{Attribute, Item, TraitItem, Type, Visibility};

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    dependencies: Vec<Dependency>,
    targets: Vec<Target>,
}

#[derive(Deserialize)]
struct Dependency {
    name: String,
    kind: Option<String>,
    rename: Option<String>,
}

#[derive(Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

impl Target {
    fn is_library(&self) -> bool {
        self.kind.iter().any(|k| {
            matches!(
                k.as_str(),
                "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
            )
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    version: u32,
    packages: BTreeMap<String, Rule>,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Role {
    Contract,
    Implementation,
    Pure,
    Protocol,
    Integration,
    Harness,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    role: Role,
    #[serde(default)]
    reason: String,
    dependencies: BTreeSet<String>,
    #[serde(default)]
    traits: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    implements: Vec<Implementation>,
    #[serde(default)]
    conformance_tests: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Implementation {
    package: String,
    r#trait: String,
    r#type: String,
}

#[derive(Default)]
struct SourceIndex {
    traits: BTreeMap<String, BTreeSet<String>>,
    public_types: BTreeSet<String>,
    implementations: BTreeSet<(String, String)>,
    tests: usize,
}

fn conditional(attrs: &[Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().is_ident("cfg") || a.path().is_ident("cfg_attr"))
}

fn path_name(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn qualified(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.into()
    } else {
        format!("{prefix}::{name}")
    }
}

struct Scanner<'a, F> {
    read: &'a F,
    visited: BTreeSet<PathBuf>,
    index: SourceIndex,
}

impl<F: Fn(&Path) -> Result<String, String>> Scanner<'_, F> {
    fn file(
        &mut self,
        path: &Path,
        module_dir: &Path,
        prefix: &str,
        public: bool,
    ) -> Result<(), String> {
        if self.visited.len() >= 512 || !self.visited.insert(path.to_path_buf()) {
            return Err(format!(
                "module cycle or source limit at {}",
                path.display()
            ));
        }
        let source = (self.read)(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file = syn::parse_file(&source).map_err(|e| format!("{}: {e}", path.display()))?;
        if !conditional(&file.attrs) {
            self.items(&file.items, module_dir, prefix, public)?;
        }
        Ok(())
    }

    fn items(
        &mut self,
        items: &[Item],
        module_dir: &Path,
        prefix: &str,
        public: bool,
    ) -> Result<(), String> {
        for item in items {
            match item {
                Item::Trait(t)
                    if public
                        && matches!(t.vis, Visibility::Public(_))
                        && !conditional(&t.attrs) =>
                {
                    let methods = t
                        .items
                        .iter()
                        .filter_map(|i| match i {
                            TraitItem::Fn(f) if f.default.is_none() && !conditional(&f.attrs) => {
                                Some(f.sig.ident.to_string())
                            }
                            _ => None,
                        })
                        .collect();
                    self.index
                        .traits
                        .insert(qualified(prefix, &t.ident.to_string()), methods);
                }
                Item::Struct(t)
                    if public
                        && matches!(t.vis, Visibility::Public(_))
                        && !conditional(&t.attrs) =>
                {
                    self.index
                        .public_types
                        .insert(qualified(prefix, &t.ident.to_string()));
                }
                Item::Enum(t)
                    if public
                        && matches!(t.vis, Visibility::Public(_))
                        && !conditional(&t.attrs) =>
                {
                    self.index
                        .public_types
                        .insert(qualified(prefix, &t.ident.to_string()));
                }
                Item::Impl(i) if !conditional(&i.attrs) => {
                    if let (Some((None, trait_path, _)), Type::Path(t)) =
                        (&i.trait_, i.self_ty.as_ref())
                    {
                        let name = path_name(&t.path);
                        let name = name
                            .strip_prefix("crate::")
                            .map(str::to_owned)
                            .unwrap_or_else(|| qualified(prefix, &name));
                        self.index
                            .implementations
                            .insert((path_name(trait_path), name));
                    }
                }
                Item::Fn(f) if !conditional(&f.attrs) => {
                    if f.attrs.iter().any(|a| a.path().is_ident("test"))
                        && !f.attrs.iter().any(|a| a.path().is_ident("ignore"))
                    {
                        self.index.tests += 1;
                    }
                }
                Item::Mod(m) if !conditional(&m.attrs) => {
                    if m.attrs.iter().any(|a| a.path().is_ident("path")) {
                        return Err("#[path] modules need explicit checker support; cannot silently skip them".into());
                    }
                    let name = m.ident.to_string();
                    let next_prefix = qualified(prefix, &name);
                    let next_dir = module_dir.join(&name);
                    let next_public = public && matches!(m.vis, Visibility::Public(_));
                    if let Some((_, items)) = &m.content {
                        self.items(items, &next_dir, &next_prefix, next_public)?;
                    } else {
                        let flat = module_dir.join(format!("{name}.rs"));
                        let nested = next_dir.join("mod.rs");
                        let path = if (self.read)(&flat).is_ok() {
                            flat
                        } else {
                            nested
                        };
                        self.file(&path, &next_dir, &next_prefix, next_public)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn scan<F: Fn(&Path) -> Result<String, String>>(
    path: &Path,
    read: &F,
) -> Result<SourceIndex, String> {
    let mut scanner = Scanner {
        read,
        visited: BTreeSet::new(),
        index: SourceIndex::default(),
    };
    scanner.file(path, path.parent().ok_or("source has no parent")?, "", true)?;
    Ok(scanner.index)
}

/// Check all library packages in Cargo's `--no-deps` metadata. This checks
/// declarations, including inactive optional/target/build/dev dependencies.
pub fn check<F: Fn(&Path) -> Result<String, String>>(
    metadata: Value,
    policy: Value,
    read: F,
) -> Result<Vec<String>, String> {
    let metadata: Metadata =
        serde_json::from_value(metadata).map_err(|e| format!("metadata: {e}"))?;
    let policy: Policy = serde_json::from_value(policy).map_err(|e| format!("policy: {e}"))?;
    if policy.version != 1 {
        return Err("unsupported architecture policy version".into());
    }
    let packages: BTreeMap<_, _> = metadata
        .packages
        .iter()
        .filter(|p| p.targets.iter().any(Target::is_library))
        .map(|p| (p.name.clone(), p))
        .collect();
    let mut errors = Vec::new();
    for name in policy.packages.keys() {
        if !packages.contains_key(name) {
            errors.push(format!("ARCH-001 stale classification: {name}"));
        }
    }
    for (name, package) in &packages {
        let Some(rule) = policy.packages.get(name) else {
            errors.push(format!("ARCH-001 unclassified library: {name}"));
            continue;
        };
        if !matches!(rule.role, Role::Contract | Role::Implementation)
            && rule.reason.trim().is_empty()
        {
            errors.push(format!(
                "ARCH-007 {name}: non-service classification requires a reason"
            ));
        }
        for dependency in &package.dependencies {
            let key = format!(
                "{}:{}",
                dependency.kind.as_deref().unwrap_or("normal"),
                dependency.name
            );
            if !rule.dependencies.contains(&key) {
                errors.push(format!("ARCH-002 {name}: undeclared dependency {key}"));
            }
            if matches!(
                rule.role,
                Role::Contract | Role::Pure | Role::Protocol | Role::Implementation
            ) {
                if let Some(target) = policy.packages.get(&dependency.name) {
                    if !matches!(target.role, Role::Contract | Role::Pure | Role::Protocol) {
                        errors.push(format!("ARCH-006 {name}: isolated library cannot depend on implementation/integration/harness {}", dependency.name));
                    }
                }
            }
        }
        let source = package
            .targets
            .iter()
            .find(|t| t.is_library())
            .expect("filtered library");
        let index = match scan(&source.src_path, &read) {
            Ok(index) => index,
            Err(e) => {
                errors.push(format!("ARCH-003 {name}: {e}"));
                continue;
            }
        };
        if rule.role == Role::Contract && rule.traits.is_empty() {
            errors.push(format!(
                "ARCH-003 {name}: contract must name a non-empty public trait"
            ));
        }
        for (trait_name, required) in &rule.traits {
            if required.is_empty()
                || !index
                    .traits
                    .get(trait_name)
                    .is_some_and(|found| required.is_subset(found))
            {
                errors.push(format!("ARCH-003 {name}: {trait_name} must expose required methods {required:?} unconditionally"));
            }
        }
        if rule.role == Role::Implementation && rule.implements.is_empty() {
            errors.push(format!("ARCH-004 {name}: implementation must name its contract and public implementation type"));
        }
        for implementation in &rule.implements {
            let contract = policy.packages.get(&implementation.package);
            let dependency = package
                .dependencies
                .iter()
                .find(|d| d.name == implementation.package && d.kind.is_none());
            let alias = dependency
                .and_then(|d| d.rename.as_deref())
                .unwrap_or(&implementation.package)
                .replace('-', "_");
            let trait_path = format!("{alias}::{}", implementation.r#trait);
            if !contract.is_some_and(|c| {
                c.role == Role::Contract && c.traits.contains_key(&implementation.r#trait)
            }) || dependency.is_none()
                || !index.public_types.contains(&implementation.r#type)
                || !index
                    .implementations
                    .contains(&(trait_path, implementation.r#type.clone()))
            {
                errors.push(format!(
                    "ARCH-004 {name}: missing explicit public implementation {} of {}::{}",
                    implementation.r#type, implementation.package, implementation.r#trait
                ));
            }
        }
        if matches!(rule.role, Role::Implementation | Role::Integration)
            && rule.conformance_tests.is_empty()
        {
            errors.push(format!("ARCH-005 {name}: conformance test target required"));
        }
        for test_name in &rule.conformance_tests {
            let target = package
                .targets
                .iter()
                .find(|t| t.name == *test_name && t.kind.iter().any(|k| k == "test"));
            if !target.is_some_and(|t| scan(&t.src_path, &read).is_ok_and(|s| s.tests > 0)) {
                errors.push(format!(
                    "ARCH-005 {name}: missing/non-executable conformance target {test_name}"
                ));
            }
        }
    }
    Ok(errors)
}
