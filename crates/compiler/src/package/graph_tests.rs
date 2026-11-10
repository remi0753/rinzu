use super::*;
use crate::{ArchivePin, load_manifest};

struct SelectedRegistry<'a> {
    project: &'a Path,
    version: u8,
}

impl PackageProvider for SelectedRegistry<'_> {
    fn dependencies(
        &mut self,
        _id: PackageId,
        owner: &ResolvedPackage,
    ) -> Result<Vec<PackageLocation>, Vec<Diagnostic>> {
        Ok(owner
            .manifest
            .dependencies
            .iter()
            .map(|dependency| {
                let name = &dependency.name;
                if name == "common" {
                    PackageLocation {
                        root: self.project.join(format!("common-{}", self.version)),
                        source: PackageSource::Archive(ArchivePin {
                            locator: format!(
                                "https://registry.example/common/1.{}.tar.gz",
                                self.version
                            ),
                            sha256: "a".repeat(64),
                            tree_sha256: "b".repeat(64),
                        }),
                    }
                } else {
                    PackageLocation::local(self.project.join(name))
                }
            })
            .collect())
    }

    fn load(&mut self, location: &PackageLocation) -> Result<Manifest, Vec<Diagnostic>> {
        load_manifest(&location.root.join("rinzu.toml"))
            .map_err(|error| vec![error.into_diagnostic()])
    }
}

#[test]
fn selected_registry_candidate_uses_the_same_graph_lock_and_frontend() {
    let project = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("rinzu-registry-graph-{}", std::process::id()));
    std::fs::create_dir(&project).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    let _cleanup = Cleanup(project.clone());
    for (directory, name, kind, entry, dependencies, source) in [
        (
            ".",
            "app",
            "application",
            "main",
            "left = { path = \"left\" }\nright = { path = \"right\" }",
            "import left; import right; fn main() { if left.value() != right.value() { panic(\"different value\"); } }",
        ),
        (
            "left",
            "left",
            "sdk",
            "left",
            "common = { archive = \"https://registry.example/common/1.4.tar.gz\" }",
            "import common; export common.value;",
        ),
        (
            "right",
            "right",
            "sdk",
            "right",
            "common = { archive = \"https://registry.example/common/1.4.tar.gz\" }",
            "import common; export common.value;",
        ),
        (
            "common-4",
            "common",
            "sdk",
            "common",
            "",
            "fn value() -> Int { return 42; } export value;",
        ),
    ] {
        let root = project.join(directory);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("rinzu.toml"), format!(
            "[package]\nname = \"{name}\"\nkind = \"{kind}\"\nlanguage = \"0.5\"\n\n[dependencies]\n{dependencies}\n",
        )).unwrap();
        std::fs::write(root.join(format!("src/{entry}.rinzu")), source).unwrap();
    }
    // The library's development lock plays no role in the consumer's selection.
    std::fs::write(
        project.join("left/rinzu.modules.lock"),
        "library selected common 1.2",
    )
    .unwrap();
    let left = 2..10;
    let right = 4..10;
    let version = [2, 4, 6]
        .into_iter()
        .find(|version| left.contains(version) && right.contains(version))
        .unwrap();
    assert_eq!(version, 4);
    let root = load_manifest(&project.join("rinzu.toml")).unwrap();
    let mut provider = SelectedRegistry {
        project: &project,
        version,
    };
    let graph = resolve_package_graph(root.clone(), &mut provider).unwrap();
    let children = &graph.package(ResolvedGraph::ROOT).dependencies;
    let common = graph.package(children["left"]).dependencies["common"];
    assert_eq!(
        graph.package(children["right"]).dependencies["common"],
        common
    );
    let lock = ModulesLock::from_graph(&graph, &root);
    assert_eq!(
        lock,
        ModulesLock::parse(&serde_json::to_vec(&lock).unwrap()).unwrap()
    );
    let mut cache = SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    let (_, artifact, _) = check_resolved_graph(&graph, "", &mut sources, None).unwrap();
    assert_eq!(artifact.sdk_packages.len(), 3);
}
