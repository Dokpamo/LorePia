import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from check_source_architecture import (
    SourceChange,
    SourceSize,
    aggregate_changes,
    aggregate_parent_child_groups,
    baseline_parent_key,
    classify_source,
    collect_core_storage_public_surface,
    collect_workspace_exported_symbol_surfaces,
    evaluate_baseline_changes,
    evaluate_character_runtime_transform_boundary,
    evaluate_core_storage_api_baseline_changes,
    evaluate_core_storage_public_surface,
    evaluate_core_storage_public_reexports,
    evaluate_dependency_architecture,
    evaluate_dependency_policy_changes,
    evaluate_source_sizes,
    evaluate_test_baseline_changes,
    evaluate_test_source_sizes,
    generated_sources,
    is_test_source,
    load_config,
    normalize_dependency_architecture,
    parent_child_group_deltas,
    parse_rust_use_tree,
    require_enf002_bootstrap_transition,
    require_v2_bootstrap_transition,
    rust_facade_public_anchors,
    rust_public_macro_invocation_anchors,
    rust_public_exported_macro_definition_anchors,
    rust_public_surface_anchor,
    rust_primary_type_public_anchors,
    rust_top_level_public_items,
    source_directory_key,
    strip_rust_comments_and_strings,
    test_sources,
    validate_dependency_architecture_config,
)


BOOTSTRAP_REF = "0" * 40
SOURCE_LANGUAGES = ("css", "kotlin", "lua", "rust", "svelte", "swift", "typescript")
TEST_LANGUAGES = ("kotlin", "rust", "svelte", "swift", "typescript")


def limits(languages: tuple[str, ...], *, bytes_limit: int = 100, lines: int = 5):
    return {
        language: {"bytes": bytes_limit, "lines": lines}
        for language in languages
    }


def source_config(
    *,
    baselines: dict[str, dict[str, int]],
    facade_paths: list[str] | None = None,
    parent_child_groups: dict[str, list[str]] | None = None,
) -> dict:
    return {
        "version": 2,
        "bootstrap_ref": BOOTSTRAP_REF,
        "facade_paths": facade_paths or [],
        "parent_child_groups": parent_child_groups or {},
        "limits": {
            "facade": limits(SOURCE_LANGUAGES),
            "generated": limits(SOURCE_LANGUAGES),
            "production": limits(SOURCE_LANGUAGES),
        },
        "baselines": baselines,
    }


def test_config(*, baselines: dict[str, dict[str, int]]) -> dict:
    return {
        "version": 2,
        "bootstrap_ref": BOOTSTRAP_REF,
        "limits": {"test": limits(TEST_LANGUAGES)},
        "baselines": baselines,
    }


def core_storage_api_config(
    *,
    core: tuple[str, ...] = (),
    storage: tuple[str, ...] = (),
    wildcards: tuple[str, ...] = (),
    stored: tuple[str, ...] = (),
    bootstrap_ref: str = BOOTSTRAP_REF,
) -> dict:
    return {
        "version": 2,
        "bootstrap_ref": bootstrap_ref,
        "public_surface": {
            "lorepia-core": sorted(core),
            "lorepia-storage": sorted(storage),
        },
        "legacy_wildcard_reexports": sorted(wildcards),
        "allowed_stored_reexports": sorted(stored),
    }


def dependency_metadata(root: Path) -> dict:
    def dependency(
        name: str,
        *,
        path: Path | None,
        kind: str | None = None,
        optional: bool = False,
        features: list[str] | None = None,
        rename: str | None = None,
    ) -> dict:
        return {
            "name": name,
            "source": None if path is not None else "registry+https://example.invalid/index",
            "req": "*" if path is not None else "^1",
            "kind": kind,
            "rename": rename,
            "optional": optional,
            "uses_default_features": True,
            "features": features or [],
            "target": None,
            "path": str(path) if path is not None else None,
        }

    package_specs = {
        "lorepia-domain": [],
        "lorepia-orchestration": [
            dependency("lorepia-domain", path=root / "crates/domain"),
            dependency("serde", path=None, features=["derive"]),
        ],
        "lorepia-storage": [
            dependency("lorepia-domain", path=root / "crates/domain")
        ],
    }
    packages = []
    for name, dependencies in package_specs.items():
        directory = name.removeprefix("lorepia-").replace("-", "_")
        package_root = root / "crates" / directory
        packages.append(
            {
                "id": f"path+file://{package_root}#{name}@0.1.0",
                "name": name,
                "manifest_path": str(package_root / "Cargo.toml"),
                "dependencies": dependencies,
                "features": {"default": []} if name == "lorepia-orchestration" else {},
            }
        )
    return {
        "packages": packages,
        "workspace_members": [package["id"] for package in packages],
        "workspace_root": str(root),
    }


def dependency_policy(metadata: dict, root: Path) -> dict:
    policy = normalize_dependency_architecture(metadata, root)
    return {"version": 1, "bootstrap_ref": BOOTSTRAP_REF, **policy}


def write_config(
    root: Path,
    *,
    baselines: dict[str, dict[str, int]],
    facade_paths: list[str] | None = None,
    parent_child_groups: dict[str, list[str]] | None = None,
) -> Path:
    config = root / "source-size-baseline.json"
    config.write_text(
        json.dumps(
            source_config(
                baselines=baselines,
                facade_paths=facade_paths,
                parent_child_groups=parent_child_groups,
            )
        ),
        encoding="utf-8",
    )
    return config


def write_test_config(root: Path, *, baselines: dict[str, dict[str, int]]) -> Path:
    config = root / "test-source-size-baseline.json"
    config.write_text(
        json.dumps(test_config(baselines=baselines)),
        encoding="utf-8",
    )
    return config


class SourceArchitectureTests(unittest.TestCase):
    def test_core_cannot_implicitly_apply_character_runtime_native_transforms(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "core" / "src" / "orchestration.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "fn unsafe_projection(content: CharacterContent) {\n"
                "    let _ = content.runtime.transform_set_id;\n"
                "}\n",
                encoding="utf-8",
            )

            failures = evaluate_character_runtime_transform_boundary(root)

            self.assertEqual(len(failures), 1)
            self.assertIn("revision-bound grant", failures[0])

            source.write_text(
                "// content.runtime.transform_set_id stays on the frontend grant path.\n"
                "fn safe_prompt_transforms() {}\n",
                encoding="utf-8",
            )
            self.assertEqual(evaluate_character_runtime_transform_boundary(root), [])

    def test_core_transform_boundary_scans_moved_alias_and_destructuring_access(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "core" / "src" / "app.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "fn unsafe_alias(content: CharacterContent) {\n"
                "    let runtime = content.runtime;\n"
                "    let _ = runtime.transform_set_id;\n"
                "}\n",
                encoding="utf-8",
            )
            failures = evaluate_character_runtime_transform_boundary(root)
            self.assertEqual(len(failures), 1)
            self.assertIn("crates/core/src/app.rs", failures[0])

            source.write_text(
                "fn unsafe_destructure(content: CharacterContent) {\n"
                "    let CharacterRuntime { transform_set_id, .. } = content.runtime;\n"
                "    drop(transform_set_id);\n"
                "}\n",
                encoding="utf-8",
            )
            failures = evaluate_character_runtime_transform_boundary(root)
            self.assertEqual(len(failures), 1)
            self.assertIn("crates/core/src/app.rs", failures[0])

    def test_core_cannot_add_storage_persistence_row_reexports(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "core" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "pub use lorepia_storage::{DatabaseStats, "
                "StoredNewPersistenceRow as CoreAlias};\n"
                "pub use {::lorepia_storage::StoredGroupedRow as ExistingName};\n",
                encoding="utf-8",
            )

            failures = evaluate_core_storage_public_reexports(root, set())

            self.assertEqual(len(failures), 2)
            self.assertTrue(any("StoredNewPersistenceRow" in failure for failure in failures))
            self.assertTrue(any("StoredGroupedRow" in failure for failure in failures))

    def test_core_cannot_wildcard_reexport_storage(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "core" / "src" / "facade.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub use lorepia_storage::*;\n", encoding="utf-8")

            failures = evaluate_core_storage_public_reexports(root, set())

            self.assertEqual(len(failures), 1)
            self.assertIn("wildcard-reexport", failures[0])

    def test_core_storage_reexport_baseline_must_shrink_with_source(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "core" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub use lorepia_storage::DatabaseStats;\n", encoding="utf-8")

            failures = evaluate_core_storage_public_reexports(
                root, {"StoredRemovedPersistenceRow"}
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("StoredRemovedPersistenceRow", failures[0])

    def test_empty_core_storage_reexport_baseline_cannot_regrow(self) -> None:
        base = {"version": 1, "allowed_stored_reexports": []}
        current = {
            "version": 1,
            "allowed_stored_reexports": ["StoredNewRow"],
        }

        failures = evaluate_core_storage_api_baseline_changes(current, base)

        self.assertEqual(len(failures), 1)
        self.assertIn("StoredNewRow", failures[0])

    def test_rust_comments_and_strings_do_not_create_reexports(self) -> None:
        content = (
            '// pub use lorepia_storage::StoredComment;\n'
            'const EXAMPLE: &str = r#"pub use lorepia_storage::StoredString;"#;\n'
            'pub use lorepia_storage::StoredVisible;\n'
        )

        stripped = strip_rust_comments_and_strings(content)

        self.assertNotIn("StoredComment", stripped)
        self.assertNotIn("StoredString", stripped)
        self.assertIn("StoredVisible", stripped)

    def test_public_use_tree_expands_nested_aliases_and_wildcards(self) -> None:
        leaves = parse_rust_use_tree(
            "lorepia_domain::{discovery::{self as discovery_api, Candidate}, "
            "orchestration::*}"
        )

        self.assertEqual(
            leaves,
            [
                ("discovery_api", "lorepia_domain::discovery", False),
                ("Candidate", "lorepia_domain::discovery::Candidate", False),
                ("*", "lorepia_domain::orchestration::*", True),
            ],
        )
        with self.assertRaisesRegex(ValueError, "unterminated public use group"):
            parse_rust_use_tree("crate::{Core")

    def test_public_surface_collects_facades_and_primary_type_members(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            core = root / "crates/core/src"
            storage = root / "crates/storage/src"
            domain = root / "crates/domain/src"
            core.mkdir(parents=True)
            storage.mkdir(parents=True)
            domain.mkdir(parents=True)
            (core / "lib.rs").write_text(
                "mod app;\nmod extension;\nmod hidden;\n"
                "pub use app::{Core, View as PublicView};\n"
                "pub use lorepia_domain::{discovery::{self as discovery_api, Candidate}, "
                "orchestration::*};\n"
                "pub use lorepia_domain::make_hidden;\n"
                "pub(crate) use app::Internal;\n"
                "pub const CORE_API_VERSION: u32 = 1;\n"
                "pub fn core_version() -> &'static str { \"1\" }\n"
                "pub fn hidden_value() -> hidden::Hidden { "
                "hidden::Hidden { value: 1 } }\n",
                encoding="utf-8",
            )
            (core / "app.rs").write_text(
                "pub struct Core;\n"
                "pub struct View { pub label: String, hidden: bool }\n"
                "pub(crate) struct Internal;\n"
                "const LEFT: char = '{';\n"
                "impl Core {\n"
                "    pub fn open() {}\n"
                "    pub async fn send() {}\n"
                "    pub(crate) fn helper() {}\n"
                "}\n"
                "impl View { pub fn inspect() {} }\n"
                "impl Internal { pub fn leaked() {} }\n"
                "macro_rules! fake { () => { impl Core { pub fn fake() {} } } }\n"
                "#[cfg(test)] mod tests { impl Core { pub fn test_only() {} } }\n",
                encoding="utf-8",
            )
            (core / "extension.rs").write_text(
                "impl crate::app::Core { pub fn qualified() {} }\n",
                encoding="utf-8",
            )
            (core / "hidden.rs").write_text(
                "pub struct Hidden { pub value: u8 }\n"
                "impl Hidden { pub fn visible_method(&self) {} }\n",
                encoding="utf-8",
            )
            (storage / "lib.rs").write_text(
                "mod database;\npub use database::{Row as PublicRow, Storage};\n",
                encoding="utf-8",
            )
            (storage / "database.rs").write_text(
                "pub struct Row;\npub struct Storage;\n"
                "impl Row { pub fn id() {} }\nimpl Storage {\n"
                "    pub const fn open() {}\n"
                "    pub(super) fn helper() {}\n"
                "}\n",
                encoding="utf-8",
            )
            (domain / "orchestration.rs").write_text(
                "pub struct WildcardType;\n"
                "impl WildcardType { pub fn validate() {} }\n",
                encoding="utf-8",
            )
            (domain / "extension.rs").write_text(
                "impl crate::orchestration::WildcardType { "
                "pub fn cross_file(&self) {} }\n",
                encoding="utf-8",
            )
            (domain / "lib.rs").write_text(
                "pub mod discovery;\nmod extension;\nmod hidden;\n"
                "pub mod orchestration;\n"
                "pub fn make_hidden() -> hidden::Hidden { hidden::Hidden { value: 1 } }\n",
                encoding="utf-8",
            )
            (domain / "hidden.rs").write_text(
                "pub struct Hidden { pub value: u8 }\n"
                "impl Hidden { pub fn from_dependency(&self) {} }\n",
                encoding="utf-8",
            )
            (domain / "discovery.rs").write_text(
                "pub struct Candidate { pub id: u64 }\n",
                encoding="utf-8",
            )

            surface = collect_core_storage_public_surface(root)
            core_surface = surface["lorepia-core"]
            storage_surface = surface["lorepia-storage"]

            for prefix in (
                "export:Candidate<-lorepia_domain::discovery::Candidate:sha256:",
                "export:Core<-local::Core:sha256:",
                "export:PublicView<-local::View:sha256:",
                "export:discovery_api<-lorepia_domain::discovery:sha256:",
                "export-surface:Candidate<-lorepia_domain::discovery::Candidate:",
                "definition:struct:Core:sha256:",
                "definition:struct:View:sha256:",
                "item:const:CORE_API_VERSION:sha256:",
                "item:fn:core_version:sha256:",
                "member:Core:fn:open:sha256:",
                "member:Core:fn:qualified:sha256:",
                "member:Core:fn:send:sha256:",
                "member:View:fn:inspect:sha256:",
                "definition:struct:Hidden:sha256:",
                "member:Hidden:fn:visible_method:sha256:",
                "workspace-public-owner:lorepia_domain:Hidden:"
                "definition:struct:Hidden:sha256:",
                "workspace-public-owner:lorepia_domain:Hidden:"
                "member:Hidden:fn:from_dependency:sha256:",
                "wildcard-declaration:lorepia_domain::orchestration:sha256:",
                "wildcard-surface:lorepia_domain::orchestration:"
                "item:struct:WildcardType:sha256:",
                "wildcard-surface:lorepia_domain::orchestration:"
                "member:WildcardType:fn:validate:sha256:",
                "wildcard-surface:lorepia_domain::orchestration:"
                "workspace-public-owner:WildcardType:"
                "member:WildcardType:fn:cross_file:sha256:",
            ):
                self.assertTrue(
                    any(anchor.startswith(prefix) for anchor in core_surface),
                    prefix,
                )
            self.assertIn("wildcard:lorepia_domain::orchestration", core_surface)
            for prefix in (
                "export:PublicRow<-local::Row:sha256:",
                "export:Storage<-local::Storage:sha256:",
                "definition:struct:Row:sha256:",
                "definition:struct:Storage:sha256:",
                "member:Row:fn:id:sha256:",
                "member:Storage:fn:open:sha256:",
            ):
                self.assertTrue(
                    any(anchor.startswith(prefix) for anchor in storage_surface),
                    prefix,
                )
            self.assertFalse(any("Internal" in anchor for anchor in core_surface))
            self.assertFalse(
                any(":fake:" in anchor for anchor in core_surface),
                [anchor for anchor in core_surface if ":fake:" in anchor],
            )
            self.assertFalse(any(":test_only:" in anchor for anchor in core_surface))

    def test_public_surface_fingerprints_contract_shapes_without_bodies(self) -> None:
        def item_anchor(source: str, name: str) -> str:
            matches = [
                rust_public_surface_anchor("definition", kind, item_name, surface)
                for kind, item_name, surface in rust_top_level_public_items(source)
                if item_name == name
            ]
            self.assertEqual(len(matches), 1, name)
            return matches[0]

        function = "pub fn render<T>(value: T) -> u8 where T: Copy { 1 }\n"
        self.assertEqual(
            item_anchor(function, "render"),
            item_anchor(function.replace("{ 1 }", "{ 2 }"), "render"),
        )
        self.assertNotEqual(
            item_anchor(function, "render"),
            item_anchor(function.replace("-> u8", "-> u16"), "render"),
        )
        array_function = "pub fn built_in() -> [u8; 2] { [0, 0] }\n"
        self.assertEqual(
            item_anchor(array_function, "built_in"),
            item_anchor(array_function.replace("[0, 0]", "[1, 1]"), "built_in"),
        )
        self.assertNotEqual(
            item_anchor(array_function, "built_in"),
            item_anchor("pub fn built_in() -> [u8; 3] { [0, 0, 0] }\n", "built_in"),
        )
        self.assertNotEqual(
            item_anchor("pub fn shaped() -> Shape<{ 1 }> { Shape }\n", "shaped"),
            item_anchor("pub fn shaped() -> Shape<{ 2 }> { Shape }\n", "shaped"),
        )
        self.assertNotEqual(
            item_anchor(
                "pub fn macro_shaped() -> type_of! { u8 } { todo!() }\n",
                "macro_shaped",
            ),
            item_anchor(
                "pub fn macro_shaped() -> type_of! { u16 } { todo!() }\n",
                "macro_shaped",
            ),
        )
        diverging = "pub fn diverge() -> ! { loop {} }\n"
        self.assertEqual(
            item_anchor(diverging, "diverge"),
            item_anchor(diverging.replace("loop {}", "panic!()"), "diverge"),
        )
        grouped_macro_type = (
            "macro_rules! ty { (<) => { u8 } }\n"
            "pub fn grouped_macro(_: ty!(<)) {}\n"
        )
        self.assertNotEqual(
            item_anchor(grouped_macro_type, "grouped_macro"),
            item_anchor(
                grouped_macro_type.replace("ty!(<)", "ty!(>)"),
                "grouped_macro",
            ),
        )
        local_signature_macro = (
            "macro_rules! inner_ty { () => { u8 } }\n"
            "macro_rules! public_ty { () => { inner_ty!() } }\n"
            "pub fn local_macro() -> public_ty!() { 0 }\n"
        )
        self.assertNotEqual(
            rust_facade_public_anchors(local_signature_macro)[0],
            rust_facade_public_anchors(
                local_signature_macro.replace("{ u8 }", "{ u16 }")
            )[0],
        )
        self.assertNotEqual(
            item_anchor(
                "pub const BRANCH: usize = if true { 1 } else { 2 };\n",
                "BRANCH",
            ),
            item_anchor(
                "pub const BRANCH: usize = if true { 1 } else { 3 };\n",
                "BRANCH",
            ),
        )
        self.assertNotEqual(
            item_anchor("pub const CALLBACK: fn() -> u8 = callback;\n", "CALLBACK"),
            item_anchor("pub const CALLBACK: fn() -> u16 = callback;\n", "CALLBACK"),
        )
        self.assertNotEqual(
            item_anchor("pub const LT: bool = 1 < 2;\n", "LT"),
            item_anchor("pub const LT: bool = 1 < 3;\n", "LT"),
        )
        self.assertNotEqual(
            item_anchor("pub const SHIFT: u8 = 1 << 2;\n", "SHIFT"),
            item_anchor("pub const SHIFT: u8 = 1 << 3;\n", "SHIFT"),
        )
        self.assertNotEqual(
            item_anchor("pub static mut VALUE: u8 = 1;\n", "VALUE"),
            item_anchor("pub static mut VALUE: u16 = 1;\n", "VALUE"),
        )
        self.assertNotEqual(
            item_anchor("#[cfg(feature = \"a\")]\n" + function, "render"),
            item_anchor("#[cfg(feature = \"b\")]\n" + function, "render"),
        )

        structure = "pub struct View { pub label: String, hidden: u8 }\n"
        self.assertEqual(
            item_anchor(structure, "View"),
            item_anchor(structure.replace("hidden: u8", "hidden: u16"), "View"),
        )
        self.assertNotEqual(
            item_anchor(structure, "View"),
            item_anchor("pub struct View { pub label: String }\n", "View"),
        )
        self.assertNotEqual(
            item_anchor(structure, "View"),
            item_anchor(structure.replace("pub label: String", "pub label: u64"), "View"),
        )
        prefixed_structure = (
            "fn private() -> bool { 1 < 2 }\n"
            "pub struct TailFields { pub shown: u8, hidden: u8 }\n"
        )
        self.assertEqual(
            item_anchor(prefixed_structure, "TailFields"),
            item_anchor(
                prefixed_structure.replace("hidden: u8", "hidden: u16"),
                "TailFields",
            ),
        )
        self.assertEqual(
            item_anchor("pub struct Tuple(pub String, u8);\n", "Tuple"),
            item_anchor("pub struct Tuple(pub String, u16);\n", "Tuple"),
        )
        self.assertNotEqual(
            item_anchor("pub struct Tuple(pub String, u8);\n", "Tuple"),
            item_anchor("pub struct Tuple(pub String, pub u8);\n", "Tuple"),
        )
        self.assertNotEqual(
            item_anchor("pub struct Callback<T: Fn(u8)>(pub T);\n", "Callback"),
            item_anchor("pub struct Callback<T: Fn(u16)>(pub T);\n", "Callback"),
        )
        self.assertNotEqual(
            item_anchor("pub struct ConstShape<const N: usize = { 1 }>;\n", "ConstShape"),
            item_anchor("pub struct ConstShape<const N: usize = { 2 }>;\n", "ConstShape"),
        )
        self.assertNotEqual(
            item_anchor(
                "pub struct Callbacks { pub callback: Wrapper<fn() -> u8, u16>, pub x: u8 }\n",
                "Callbacks",
            ),
            item_anchor(
                "pub struct Callbacks { pub callback: Wrapper<fn() -> u8, u32>, pub x: u8 }\n",
                "Callbacks",
            ),
        )
        self.assertNotEqual(
            item_anchor("pub enum Mode { A, B(u8) }\n", "Mode"),
            item_anchor("pub enum Mode { A, B(u16), C }\n", "Mode"),
        )
        self.assertNotEqual(
            item_anchor('#[doc = "a b"]\npub struct Literal;\n', "Literal"),
            item_anchor('#[doc = "a  b"]\npub struct Literal;\n', "Literal"),
        )
        self.assertNotEqual(
            item_anchor('pub const C_TEXT: &CStr = cr#"a b"#;\n', "C_TEXT"),
            item_anchor('pub const C_TEXT: &CStr = cr#"a  b"#;\n', "C_TEXT"),
        )
        self.assertNotEqual(
            rust_facade_public_anchors(
                '#[cfg(feature = "a")]\npub use app::Core;\n'
            )[0],
            rust_facade_public_anchors(
                '#[cfg(feature = "b")]\npub use app::Core;\n'
            )[0],
        )
        original_group = rust_facade_public_anchors(
            "pub use app::{Core, View};\n"
        )[0]
        extended_group = rust_facade_public_anchors(
            "pub use app::{Core, View, Window};\n"
        )[0]
        self.assertTrue(
            {anchor for anchor in original_group if "export:Core" in anchor}
            <= extended_group
        )

        trait = "pub trait Broker { fn lease(&self) -> u8 { 1 } }\n"
        self.assertEqual(
            item_anchor(trait, "Broker"),
            item_anchor(trait.replace("{ 1 }", "{ 2 }"), "Broker"),
        )
        self.assertNotEqual(
            item_anchor(trait, "Broker"),
            item_anchor(trait.replace("-> u8", "-> u16"), "Broker"),
        )
        prefixed_trait = (
            "fn private() -> bool { 1 < 2 }\n"
            "pub trait TailExpression { fn value(&self) -> u8 { 1 } }\n"
        )
        self.assertEqual(
            item_anchor(prefixed_trait, "TailExpression"),
            item_anchor(
                prefixed_trait.replace("-> u8 { 1 }", "-> u8 { 2 }"),
                "TailExpression",
            ),
        )
        trait_const = "pub trait Sized { fn width() {} const N: usize = { 1 }; }\n"
        self.assertNotEqual(
            item_anchor(trait_const, "Sized"),
            item_anchor(trait_const.replace("{ 1 }", "{ 2 }"), "Sized"),
        )
        trait_macro_type = (
            "pub trait MacroType { fn value(&self) -> type_of! { u8 } {} }\n"
        )
        self.assertNotEqual(
            item_anchor(trait_macro_type, "MacroType"),
            item_anchor(trait_macro_type.replace("u8", "u16"), "MacroType"),
        )
        const_generic_default = (
            "pub trait ConstGenericDefault { "
            "fn value() -> Shape<{ 1 }> { let value = 1; } }\n"
        )
        self.assertEqual(
            item_anchor(const_generic_default, "ConstGenericDefault"),
            item_anchor(
                const_generic_default.replace("let value = 1", "let value = 2"),
                "ConstGenericDefault",
            ),
        )

        implementation = (
            "pub struct Core;\n"
            "struct Hidden;\n"
            "impl crate::app::Core { pub async fn run(&self) -> u8 { 1 } }\n"
            "impl Hidden { pub fn leaked() {} }\n"
        )
        member_surface = rust_primary_type_public_anchors(implementation, {"Core"})
        changed_body = rust_primary_type_public_anchors(
            implementation.replace("{ 1 }", "{ 2 }"), {"Core"}
        )
        changed_signature = rust_primary_type_public_anchors(
            implementation.replace("-> u8", "-> u16"), {"Core"}
        )
        self.assertEqual(member_surface, changed_body)
        self.assertNotEqual(member_surface, changed_signature)
        self.assertEqual(len(member_surface), 1)
        self.assertFalse(any("Hidden" in anchor for anchor in member_surface))
        macro_member = (
            "macro_rules! ty { () => { u8 } }\n"
            "pub struct MacroOwner;\n"
            "impl MacroOwner { pub fn value(&self) -> ty!() { 0 } }\n"
        )
        self.assertNotEqual(
            rust_primary_type_public_anchors(macro_member, {"MacroOwner"}),
            rust_primary_type_public_anchors(
                macro_member.replace("{ u8 }", "{ u16 }"), {"MacroOwner"}
            ),
        )
        direct_member_macro = (
            "macro_rules! add_api { () => { "
            "pub fn generated(&self) -> u8 { 1 } } }\n"
            "pub struct MacroOwner;\n"
            "impl MacroOwner { add_api!(); }\n"
        )
        direct_member_surface = rust_primary_type_public_anchors(
            direct_member_macro, {"MacroOwner"}
        )
        self.assertTrue(
            any(
                anchor.startswith("direct-macro:MacroOwner:member:add_api:")
                for anchor in direct_member_surface
            )
        )
        self.assertNotEqual(
            direct_member_surface,
            rust_primary_type_public_anchors(
                direct_member_macro.replace("-> u8", "-> u16"), {"MacroOwner"}
            ),
        )
        opaque_return = (
            "pub struct Foo;\n"
            "fn make() -> impl Fn() -> Foo { pub fn hidden() {} || Foo }\n"
        )
        self.assertEqual(
            rust_primary_type_public_anchors(opaque_return, {"Foo"}),
            set(),
        )
        self.assertEqual(
            member_surface,
            rust_primary_type_public_anchors(
                implementation.replace("impl crate::app::Core", "impl Core"),
                {"Core"},
            ),
        )
        nested_impl = (
            "pub struct Core;\n"
            "mod private_extra { "
            "impl super::Core { pub async fn run(&self) -> u8 { 1 } } }\n"
        )
        self.assertEqual(
            member_surface,
            rust_primary_type_public_anchors(nested_impl, {"Core"}),
        )
        const_block_impl = (
            "pub struct Core;\n"
            "const _: () = { impl Core { "
            "pub async fn run(&self) -> u8 { 1 } } };\n"
        )
        self.assertEqual(
            member_surface,
            rust_primary_type_public_anchors(const_block_impl, {"Core"}),
        )
        separated_macros = (
            "macro_rules! shape { () => { u8 } }\n"
            "pub struct Core;\ntype A = shape!{};\n"
            "impl Core { pub async fn run(&self) -> u8 { 1 } }\n"
            "type B = shape!{};\n"
        )
        self.assertEqual(
            member_surface,
            rust_primary_type_public_anchors(separated_macros, {"Core"}),
        )
        self.assertNotEqual(
            rust_primary_type_public_anchors(
                implementation.replace(
                    "impl crate::app::Core",
                    '#[cfg(feature = "a")]\nimpl crate::app::Core',
                ),
                {"Core"},
            ),
            rust_primary_type_public_anchors(
                implementation.replace(
                    "impl crate::app::Core",
                    '#[cfg(feature = "b")]\nimpl crate::app::Core',
                ),
                {"Core"},
            ),
        )
        trait_object = (
            "pub trait PublicTrait {}\n"
            "impl dyn crate::PublicTrait { pub fn exposed(&self) {} }\n"
        )
        self.assertEqual(
            rust_primary_type_public_anchors(trait_object, {"PublicTrait"}),
            rust_primary_type_public_anchors(
                trait_object.replace("dyn crate::PublicTrait", "dyn PublicTrait"),
                {"PublicTrait"},
            ),
        )
        self.assertTrue(
            any(
                anchor.startswith("member:PublicTrait:fn:exposed:sha256:")
                for anchor in rust_primary_type_public_anchors(
                    trait_object, {"PublicTrait"}
                )
            )
        )
        self.assertTrue(
            any(
                anchor.startswith("member:PublicTrait:fn:exposed:sha256:")
                for anchor in rust_primary_type_public_anchors(
                    trait_object.replace(
                        "dyn crate::PublicTrait", "dyn crate::PublicTrait + Send"
                    ),
                    {"PublicTrait"},
                )
            )
        )
        generic_impl = (
            "pub struct Generic<T>(T);\n"
            "impl<T: Fn() -> u8> Generic<T> { pub fn value(&self) -> &T { &self.0 } }\n"
        )
        self.assertNotEqual(
            rust_primary_type_public_anchors(generic_impl, {"Generic"}),
            rust_primary_type_public_anchors(
                generic_impl.replace("-> u8", "-> u16"), {"Generic"}
            ),
        )
        const_generic_impl = (
            "pub struct Generic<F>(F);\n"
            "impl<F: Example<{ 1 << 2 }>> Generic<F> { pub fn value(&self) {} }\n"
        )
        self.assertNotEqual(
            rust_primary_type_public_anchors(const_generic_impl, {"Generic"}),
            rust_primary_type_public_anchors(
                const_generic_impl.replace("1 << 2", "1 << 3"), {"Generic"}
            ),
        )

        trait_impl = (
            "pub struct Core;\nstruct Hidden;\n"
            "impl Example for Core { type Error = u8; const LIMIT: u8 = 1; "
            "fn clone(&self) -> Self { Core } }\n"
            "impl Example for Hidden { type Error = u8; "
            "fn clone(&self) -> Self { Hidden } }\n"
        )
        trait_impl_surface = rust_primary_type_public_anchors(trait_impl, {"Core"})
        self.assertEqual(len(trait_impl_surface), 3)
        self.assertTrue(
            all(anchor.startswith("trait-impl:Core:") for anchor in trait_impl_surface)
        )
        self.assertEqual(
            trait_impl_surface,
            rust_primary_type_public_anchors(
                trait_impl.replace("impl Example for Core", "impl Example for crate::app::Core"),
                {"Core"},
            ),
        )
        self.assertEqual(
            trait_impl_surface,
            rust_primary_type_public_anchors(
                trait_impl.replace("{ Core }", "{ panic!() }"), {"Core"}
            ),
        )
        self.assertNotEqual(
            trait_impl_surface,
            rust_primary_type_public_anchors(
                trait_impl.replace("impl Example for Core", "impl Other for Core"),
                {"Core"},
            ),
        )
        self.assertNotEqual(
            rust_primary_type_public_anchors(
                trait_impl.replace(
                    "impl Example for Core",
                    '#[cfg(feature = "a")] unsafe impl Example for Core',
                ),
                {"Core"},
            ),
            rust_primary_type_public_anchors(
                trait_impl.replace(
                    "impl Example for Core",
                    '#[cfg(feature = "b")] unsafe impl Example for Core',
                ),
                {"Core"},
            ),
        )
        self.assertNotEqual(
            rust_primary_type_public_anchors(
                trait_impl.replace("impl Example for Core", "impl Example<[u8; 2]> for Core"),
                {"Core"},
            ),
            rust_primary_type_public_anchors(
                trait_impl.replace("impl Example for Core", "impl Example<[u8; 3]> for Core"),
                {"Core"},
            ),
        )
        wrapped_trait_impl = (
            "pub trait Ext {}\npub struct Foo;\n"
            "impl Ext for Box<Foo> { type Error = u8; }\n"
        )
        wrapped_surface = rust_primary_type_public_anchors(
            wrapped_trait_impl, {"Ext", "Foo"}
        )
        self.assertTrue(
            any(anchor.startswith("trait-impl:Ext:") for anchor in wrapped_surface)
        )
        self.assertTrue(
            any(anchor.startswith("trait-impl:Foo:") for anchor in wrapped_surface)
        )
        self.assertEqual(
            wrapped_surface,
            rust_primary_type_public_anchors(
                wrapped_trait_impl.replace(
                    "impl Ext for Box<Foo>",
                    "impl crate::Ext for Box<crate::Foo>",
                ),
                {"Ext", "Foo"},
            ),
        )
        self.assertTrue(
            any(
                anchor.startswith("trait-impl:Ext:")
                for anchor in rust_primary_type_public_anchors(
                    "pub trait Ext {}\nimpl<T> Ext for T {}\n", {"Ext"}
                )
            )
        )
        self.assertNotEqual(
            trait_impl_surface,
            rust_primary_type_public_anchors(
                trait_impl.replace("type Error = u8", "type Error = u16", 1),
                {"Core"},
            ),
        )
        const_trait_impl = (
            "pub struct Core;\n"
            "impl Example<{ 1 << 2 }> for Core { type Error = u8; }\n"
        )
        self.assertNotEqual(
            rust_primary_type_public_anchors(const_trait_impl, {"Core"}),
            rust_primary_type_public_anchors(
                const_trait_impl.replace("1 << 2", "1 << 3"), {"Core"}
            ),
        )
        hrtb_trait_impl = (
            "pub struct Core;\n"
            "impl<T> Example for crate::app::Core where T: for<'a> Fn(&'a str) "
            "{ fn clone(&self) -> Self { Core } }\n"
        )
        self.assertEqual(
            len(rust_primary_type_public_anchors(hrtb_trait_impl, {"Core"})),
            1,
        )

        macro = (
            "macro_rules! public_id { ($name:ident) => {\n"
            "pub struct $name(String);\n"
            "impl $name { pub fn as_str(&self) -> &str { &self.0 } }\n"
            "}; }\n"
            "public_id!(Alpha);\n"
        )
        macro_surface, macro_owners = rust_public_macro_invocation_anchors(macro)
        expanded_surface, expanded_owners = rust_public_macro_invocation_anchors(
            macro.replace(
                "pub fn as_str(&self) -> &str",
                "pub fn as_str(&self) -> &str; pub fn len(&self) -> usize",
            )
        )
        self.assertEqual(macro_owners, {"Alpha"})
        self.assertEqual(expanded_owners, {"Alpha"})
        self.assertNotEqual(macro_surface, expanded_surface)
        self.assertNotEqual(
            rust_public_macro_invocation_anchors(
                '#[cfg(feature = "a")]\n' + macro
            )[0],
            rust_public_macro_invocation_anchors(
                '#[cfg(feature = "b")]\n' + macro
            )[0],
        )
        self.assertNotEqual(
            rust_public_macro_invocation_anchors(
                macro.replace(
                    "public_id!(Alpha)",
                    '#[cfg(feature = "a")] crate::public_id!(Alpha)',
                )
            )[0],
            rust_public_macro_invocation_anchors(
                macro.replace(
                    "public_id!(Alpha)",
                    '#[cfg(feature = "b")] other::public_id!(Alpha)',
                )
            )[0],
        )
        sibling_macro = macro.replace("public_id!(Alpha)", "public_id!(Alpha, Beta)")
        sibling_surface = rust_public_macro_invocation_anchors(sibling_macro)[0]
        reduced_surface = rust_public_macro_invocation_anchors(macro)[0]
        self.assertEqual(
            {anchor for anchor in sibling_surface if ":Alpha:" in anchor},
            {anchor for anchor in reduced_surface if ":Alpha:" in anchor},
        )
        fixed_macro_surface, fixed_macro_owners = rust_public_macro_invocation_anchors(
            "macro_rules! make { () => { pub struct Generated; } }\nmake!();\n"
        )
        self.assertEqual(fixed_macro_owners, {"Generated"})
        self.assertTrue(
            any(
                anchor.startswith("macro-export:make:Generated:sha256:")
                for anchor in fixed_macro_surface
            )
        )
        _, fixed_trait_owners = rust_public_macro_invocation_anchors(
            "macro_rules! make { () => { pub trait GeneratedTrait {} } }\n"
            "make!();\n"
        )
        self.assertEqual(fixed_trait_owners, {"GeneratedTrait"})
        self.assertNotEqual(
            rust_public_exported_macro_definition_anchors(
                "#[macro_export]\nmacro_rules! public_api { () => { u8 } }\n"
            ),
            rust_public_exported_macro_definition_anchors(
                "#[macro_export]\nmacro_rules! public_api { () => { u16 } }\n"
            ),
        )
        self.assertTrue(
            any(
                anchor.startswith("macro-export-definition:r#try:")
                for anchor in rust_public_exported_macro_definition_anchors(
                    "#[macro_export]\nmacro_rules! r#try { () => { 1 } }\n"
                )
            )
        )
        with self.assertRaisesRegex(ValueError, "non-ASCII Rust identifier"):
            rust_facade_public_anchors(
                "#[macro_export]\nmacro_rules! café { () => { 1 } }\n"
            )
        nested_item_macro = (
            "macro_rules! api { ($target:ty) => { impl $target { "
            "pub fn generated(&self) {} } } }\n"
            "pub struct MacroTarget;\n"
            "mod private { use super::MacroTarget; api!(MacroTarget); }\n"
        )
        nested_macro_surface = rust_public_macro_invocation_anchors(
            nested_item_macro
        )[0]
        self.assertTrue(
            any(
                anchor.startswith("macro-top-level-invocation:api:")
                for anchor in nested_macro_surface
            )
        )
        self.assertNotEqual(
            nested_macro_surface,
            rust_public_macro_invocation_anchors(
                nested_item_macro.replace("pub fn generated", "pub fn replacement")
            )[0],
        )

        visibility_macro = (
            "macro_rules! make { ($v:vis $name:ident) => { $v struct $name; }; }\n"
            "make!{pub New}\n"
        )
        visibility_surface, visibility_owners = rust_public_macro_invocation_anchors(
            visibility_macro
        )
        self.assertEqual(visibility_owners, {"New"})
        self.assertTrue(
            any(
                anchor.startswith("macro-public-invocation:make:New:sha256:")
                for anchor in visibility_surface
            )
        )
        bracket_surface, bracket_owners = rust_public_macro_invocation_anchors(
            "bitflags! [ pub struct Flags: u8 { const A = 1; } ]\n"
        )
        self.assertEqual(bracket_owners, {"Flags"})
        self.assertTrue(
            any(
                anchor.startswith("macro-public-invocation:bitflags:Flags:sha256:")
                for anchor in bracket_surface
            )
        )
        self.assertNotEqual(
            rust_public_macro_invocation_anchors(
                '#[cfg(feature = "a")]\na::bitflags! { pub struct Flags: u8 {} }\n'
            )[0],
            rust_public_macro_invocation_anchors(
                '#[cfg(feature = "b")]\nb::bitflags! { pub struct Flags: u8 {} }\n'
            )[0],
        )

    def test_workspace_public_surface_resolution_fails_closed_on_ambiguity(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "crates/sample/src"
            source_root.mkdir(parents=True)
            (source_root / "one.rs").write_text(
                "pub struct Duplicate;\n", encoding="utf-8"
            )
            (source_root / "two.rs").write_text(
                "pub struct Duplicate;\n", encoding="utf-8"
            )

            with self.assertRaisesRegex(ValueError, "exactly one definition surface"):
                collect_workspace_exported_symbol_surfaces(
                    root, Path("crates/sample/src"), {"Duplicate"}
                )

            (source_root / "one.rs").write_text(
                "pub struct Public;\n", encoding="utf-8"
            )
            (source_root / "two.rs").write_text(
                "struct Public;\n", encoding="utf-8"
            )
            with self.assertRaisesRegex(ValueError, "ambiguous private same-name"):
                collect_workspace_exported_symbol_surfaces(
                    root, Path("crates/sample/src"), {"Public"}
                )

            (source_root / "two.rs").write_text(
                "struct Hidden;\n"
                "type Public = Hidden;\n"
                "impl Public { pub fn leaked() {} }\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "ambiguous private same-name"):
                collect_workspace_exported_symbol_surfaces(
                    root, Path("crates/sample/src"), {"Public"}
                )

            (source_root / "one.rs").write_text(
                'unsafe extern "C" { pub fn foreign_api(value: u8) -> u16; }\n',
                encoding="utf-8",
            )
            (source_root / "two.rs").write_text(
                "pub struct Public;\n", encoding="utf-8"
            )
            with self.assertRaisesRegex(ValueError, "public foreign items"):
                collect_workspace_exported_symbol_surfaces(
                    root, Path("crates/sample/src"), {"Public"}
                )

            (source_root / "one.rs").write_text(
                "pub struct Public;\n", encoding="utf-8"
            )
            (source_root / "two.rs").write_text("", encoding="utf-8")
            (source_root / "lib.rs").write_text(
                "mod tests;\n", encoding="utf-8"
            )
            (source_root / "tests.rs").write_text(
                "impl crate::Public { pub fn leaked_from_test_name(&self) {} }\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "must be included only"):
                collect_workspace_exported_symbol_surfaces(
                    root, Path("crates/sample/src"), {"Public"}
                )

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "crates/sample/src"
            module_root = source_root / "parent"
            module_root.mkdir(parents=True)
            (source_root / "parent.rs").write_text(
                '#[path = "parent/support.rs"]\npub mod support;\n',
                encoding="utf-8",
            )
            (module_root / "support.rs").write_text(
                "pub struct Fixture;\n"
                "impl Fixture { pub fn intended() {} }\n",
                encoding="utf-8",
            )
            (source_root / "unrelated.rs").write_text(
                "pub struct Fixture;\n"
                "impl Fixture { pub fn unrelated() {} }\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "exactly one definition surface"):
                collect_workspace_exported_symbol_surfaces(
                    root, Path("crates/sample/src"), {"support"}
                )

    def test_workspace_public_surface_expands_aliases_and_external_modules(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "crates/sample/src"
            support_root = source_root / "parent"
            support_root.mkdir(parents=True)
            (source_root / "contract.rs").write_text(
                "pub struct Resolved { pub value: u8 }\n"
                "pub type Alias = Resolved;\n"
                "type InternalAlias = Alias;\n"
                "use crate::contract::Resolved as ImportedAlias;\n"
                "impl Resolved { pub fn verify(&self) -> bool { true } }\n"
                "impl Alias { pub fn exposed(&self) {} }\n"
                "impl InternalAlias { pub fn internally_exposed(&self) {} }\n"
                "impl ImportedAlias { pub fn imported_exposed(&self) {} }\n"
                "mod nested { use super::Resolved as NestedAlias; "
                "impl NestedAlias { pub fn nested_exposed(&self) {} } }\n",
                encoding="utf-8",
            )
            (source_root / "parent.rs").write_text(
                '#[path = "parent/support.rs"]\npub mod support;\n',
                encoding="utf-8",
            )
            support = support_root / "support.rs"
            support.write_text(
                "pub struct Fixture { pub id: u8 }\n"
                "pub use crate::contract::Resolved as PublicResolved;\n"
                "pub fn seed() -> Fixture { Fixture { id: 1 } }\n",
                encoding="utf-8",
            )

            observed = collect_workspace_exported_symbol_surfaces(
                root, Path("crates/sample/src"), {"Alias", "support"}
            )
            for prefix in (
                "alias-target:Alias:definition:struct:Resolved:sha256:",
                "alias-target:Alias:member:Resolved:fn:verify:sha256:",
                "alias-target:Alias:member:Resolved:fn:exposed:sha256:",
            ):
                self.assertTrue(
                    any(anchor.startswith(prefix) for anchor in observed["Alias"]),
                    prefix,
                )
            resolved = collect_workspace_exported_symbol_surfaces(
                root, Path("crates/sample/src"), {"Resolved"}
            )["Resolved"]
            for prefix in (
                "member:Resolved:fn:exposed:sha256:",
                "member:Resolved:fn:internally_exposed:sha256:",
                "member:Resolved:fn:imported_exposed:sha256:",
                "member:Resolved:fn:nested_exposed:sha256:",
            ):
                self.assertTrue(
                    any(anchor.startswith(prefix) for anchor in resolved),
                    prefix,
                )
            alias_impl = collect_workspace_exported_symbol_surfaces(
                root, Path("crates/sample/src"), {"Resolved"}
            )["Resolved"]
            contract = source_root / "contract.rs"
            contract.write_text(
                contract.read_text(encoding="utf-8").replace(
                    "impl Alias { pub fn exposed(&self) {} }",
                    "impl Resolved { pub fn exposed(&self) {} }",
                ),
                encoding="utf-8",
            )
            direct_impl = collect_workspace_exported_symbol_surfaces(
                root, Path("crates/sample/src"), {"Resolved"}
            )["Resolved"]
            self.assertEqual(alias_impl, direct_impl)
            for prefix in (
                "module-surface:support:item:struct:Fixture:sha256:",
                "module-surface:support:item:fn:seed:sha256:",
                "module-surface:support:export:PublicResolved<-local::Resolved:sha256:",
                "module-surface:support:export-surface:PublicResolved<-crate::contract::Resolved:definition:struct:Resolved:sha256:",
                "module-surface:support:export-surface:PublicResolved<-crate::contract::Resolved:member:Resolved:fn:verify:sha256:",
            ):
                self.assertTrue(
                    any(anchor.startswith(prefix) for anchor in observed["support"]),
                    prefix,
                )

            original_module_surface = observed["support"]
            support.write_text(
                "pub struct Fixture { pub id: u8 }\n"
                "pub use crate::contract::Resolved as PublicResolved;\n"
                "pub fn seed() -> Fixture { Fixture { id: 2 } }\n",
                encoding="utf-8",
            )
            body_only = collect_workspace_exported_symbol_surfaces(
                root, Path("crates/sample/src"), {"support"}
            )["support"]
            self.assertEqual(original_module_surface, body_only)

            support.write_text(
                "pub struct Fixture { pub id: u8 }\n"
                "pub use crate::contract::Resolved as PublicResolved;\n"
                "pub fn seed() -> Fixture { Fixture { id: 1 } }\n"
                "pub fn reset() {}\n",
                encoding="utf-8",
            )
            changed = collect_workspace_exported_symbol_surfaces(
                root, Path("crates/sample/src"), {"support"}
            )["support"]
            self.assertNotEqual(original_module_surface, changed)
            self.assertTrue(any(":item:fn:reset:" in anchor for anchor in changed))

    def test_public_inventory_tracks_free_items_and_literal_includes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            core = root / "crates/core/src"
            storage = root / "crates/storage/src"
            domain = root / "crates/domain/src"
            generated = core / "generated"
            for directory in (core, storage, domain, generated):
                directory.mkdir(parents=True, exist_ok=True)
            (core / "lib.rs").write_text(
                "mod hidden;\npub mod exposed;\npub mod api;\n"
                'include!{"generated/root.rs"}\n'
                "pub use lorepia_domain::Thing;\n"
                "macro_rules! make_type { () => {} }\n"
                "lorepia_domain::make_type!(Foo);\nimpl Foo {}\n"
                "use lorepia_domain::make_type as mt;\n"
                "mt!(Bar);\nimpl Bar {}\n"
                "use lorepia_domain as ld;\n"
                "ld::make_type!(Baz);\nimpl Baz {}\n",
                encoding="utf-8",
            )
            api = core / "api.rs"
            api.write_text("pub fn old() {}\n", encoding="utf-8")
            hidden = core / "hidden.rs"
            hidden.write_text("pub fn newly_public() {}\n", encoding="utf-8")
            exposed = core / "exposed.rs"
            exposed.write_text("", encoding="utf-8")
            generated_api = generated / "root.rs"
            generated_api.write_text("pub const ROOT: u8 = 1;\n", encoding="utf-8")
            (storage / "lib.rs").write_text("", encoding="utf-8")
            domain_api = domain / "lib.rs"
            domain_api.write_text(
                "#[macro_export]\n"
                "macro_rules! make_type { ($name:ident) => { pub struct $name; } }\n"
                "pub struct Thing;\npub fn external_old() {}\n",
                encoding="utf-8",
            )

            baseline = collect_core_storage_public_surface(root)["lorepia-core"]
            api.write_text(
                "pub fn old() {}\npub fn added() {}\n", encoding="utf-8"
            )
            with_module_growth = collect_core_storage_public_surface(root)[
                "lorepia-core"
            ]
            self.assertNotEqual(baseline, with_module_growth)
            self.assertTrue(any(":fn:added:" in anchor for anchor in with_module_growth))

            api.write_text("pub fn old() {}\n", encoding="utf-8")
            generated_api.write_text(
                "pub const ROOT: u8 = 1;\npub static ADDED: u8 = 2;\n",
                encoding="utf-8",
            )
            with_generated_growth = collect_core_storage_public_surface(root)[
                "lorepia-core"
            ]
            self.assertNotEqual(baseline, with_generated_growth)
            self.assertTrue(
                any(":static:ADDED:" in anchor for anchor in with_generated_growth)
            )

            generated_api.write_text("pub const ROOT: u8 = 1;\n", encoding="utf-8")
            domain_api.write_text(
                "#[macro_export]\n"
                "macro_rules! make_type { ($name:ident) => { pub struct $name; } }\n"
                "pub struct Thing;\npub fn external_old() {}\n"
                "pub fn external_added() {}\n",
                encoding="utf-8",
            )
            with_dependency_growth = collect_core_storage_public_surface(root)[
                "lorepia-core"
            ]
            self.assertNotEqual(baseline, with_dependency_growth)
            self.assertTrue(
                any("external_added" in anchor for anchor in with_dependency_growth)
            )

            domain_api.write_text(
                "#[macro_export]\n"
                "macro_rules! make_type { ($name:ident) => { pub struct $name; } }\n"
                "pub struct Thing;\npub fn external_old() {}\n",
                encoding="utf-8",
            )
            exposed.write_text(
                "pub fn newly_public() {}\n", encoding="utf-8"
            )
            hidden.write_text("", encoding="utf-8")
            with_public_module_move = collect_core_storage_public_surface(root)[
                "lorepia-core"
            ]
            self.assertNotEqual(baseline, with_public_module_move)
            self.assertTrue(
                any(
                    "module-surface:exposed:item:fn:newly_public" in anchor
                    for anchor in with_public_module_move
                )
            )

            exposed.write_text("", encoding="utf-8")
            generated_api.write_text(
                "pub const ROOT: u8 = 1;\npub fn newly_public() {}\n",
                encoding="utf-8",
            )
            with_root_include_move = collect_core_storage_public_surface(root)[
                "lorepia-core"
            ]
            self.assertNotEqual(baseline, with_root_include_move)
            self.assertTrue(
                any(
                    "crate-root-include:definition:fn:newly_public" in anchor
                    for anchor in with_root_include_move
                )
            )

            generated_api.write_text("pub const ROOT: u8 = 1;\n", encoding="utf-8")
            hidden.write_text("pub fn newly_public() {}\n", encoding="utf-8")
            core_lib = core / "lib.rs"
            core_lib.write_text(
                core_lib.read_text(encoding="utf-8").replace(
                    "impl Foo {}", "impl Foo { pub fn added(&self) {} }"
                ),
                encoding="utf-8",
            )
            with_workspace_macro_impl = collect_core_storage_public_surface(root)[
                "lorepia-core"
            ]
            self.assertNotEqual(baseline, with_workspace_macro_impl)
            self.assertTrue(
                any(":Foo:fn:added:" in anchor for anchor in with_workspace_macro_impl)
            )

            core_lib.write_text(
                core_lib.read_text(encoding="utf-8")
                .replace("impl Foo { pub fn added(&self) {} }", "impl Foo {}")
                .replace("impl Bar {}", "impl Bar { pub fn added(&self) {} }"),
                encoding="utf-8",
            )
            with_workspace_macro_alias_impl = collect_core_storage_public_surface(
                root
            )["lorepia-core"]
            self.assertNotEqual(baseline, with_workspace_macro_alias_impl)
            self.assertTrue(
                any(
                    ":Bar:fn:added:" in anchor
                    for anchor in with_workspace_macro_alias_impl
                )
            )

            core_lib.write_text(
                core_lib.read_text(encoding="utf-8")
                .replace("impl Bar { pub fn added(&self) {} }", "impl Bar {}")
                .replace("impl Baz {}", "impl Baz { pub fn added(&self) {} }"),
                encoding="utf-8",
            )
            with_workspace_crate_alias_impl = collect_core_storage_public_surface(
                root
            )["lorepia-core"]
            self.assertNotEqual(baseline, with_workspace_crate_alias_impl)
            self.assertTrue(
                any(
                    ":Baz:fn:added:" in anchor
                    for anchor in with_workspace_crate_alias_impl
                )
            )

    def test_public_surface_tracks_transitive_and_cross_file_macros(self) -> None:
        direct_base = (
            "macro_rules! helper { ($n:ident) => { pub struct $n; } }\n"
            "macro_rules! make { ($n:ident) => { helper!($n); } }\n"
            "make!(Foo);\n"
        )
        direct_changed = direct_base.replace(
            "pub struct $n;", "pub struct $n { pub added: u8 }"
        )
        base_anchors, base_owners = rust_public_macro_invocation_anchors(direct_base)
        changed_anchors, changed_owners = rust_public_macro_invocation_anchors(
            direct_changed
        )
        self.assertEqual(base_owners, {"Foo"})
        self.assertEqual(changed_owners, {"Foo"})
        self.assertNotEqual(base_anchors, changed_anchors)

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "crates/sample/src"
            source_root.mkdir(parents=True)
            (source_root / "lib.rs").write_text(
                "#[macro_use] mod macros;\nmod item;\npub use item::Foo;\n",
                encoding="utf-8",
            )
            macros = source_root / "macros.rs"
            macros.write_text(
                "macro_rules! api_ty { () => { u8 } }\n"
                "macro_rules! helper { () => { pub fn first(&self) {} } }\n"
                "macro_rules! add { () => { helper!(); } }\n",
                encoding="utf-8",
            )
            (source_root / "item.rs").write_text(
                "pub struct Foo { pub value: api_ty!() }\n"
                "impl Foo { add!(); }\n",
                encoding="utf-8",
            )
            baseline = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                {"Foo"},
                include_public_inventory=True,
            )
            macros.write_text(
                macros.read_text(encoding="utf-8").replace(
                    "api_ty { () => { u8 } }", "api_ty { () => { u16 } }"
                ),
                encoding="utf-8",
            )
            type_changed = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                {"Foo"},
                include_public_inventory=True,
            )
            self.assertNotEqual(baseline, type_changed)

            macros.write_text(
                macros.read_text(encoding="utf-8")
                .replace("api_ty { () => { u16 } }", "api_ty { () => { u8 } }")
                .replace(
                    "pub fn first(&self) {}",
                    "pub fn first(&self) {} pub fn added(&self) {}",
                ),
                encoding="utf-8",
            )
            helper_changed = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                {"Foo"},
                include_public_inventory=True,
            )
            self.assertNotEqual(baseline, helper_changed)

            (source_root / "hidden.rs").write_text(
                "make!();\n", encoding="utf-8"
            )
            macros.write_text(
                "macro_rules! make { () => { pub fn generated() {} } }\n",
                encoding="utf-8",
            )
            (source_root / "item.rs").write_text("", encoding="utf-8")
            (source_root / "lib.rs").write_text(
                "#[macro_use] mod macros;\nmod hidden;\n",
                encoding="utf-8",
            )
            private_macro = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                set(),
                include_public_inventory=True,
            )
            (source_root / "hidden.rs").write_text("", encoding="utf-8")
            (source_root / "lib.rs").write_text(
                "#[macro_use] mod macros;\nmod hidden;\nmake!();\n",
                encoding="utf-8",
            )
            root_macro = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                set(),
                include_public_inventory=True,
            )
            self.assertNotEqual(private_macro, root_macro)

            macros.write_text(
                "macro_rules! export_mod { () => { pub mod exposed; } }\n",
                encoding="utf-8",
            )
            (source_root / "lib.rs").write_text(
                "#[macro_use] mod macros;\nmod hidden;\nexport_mod!();\n",
                encoding="utf-8",
            )
            (source_root / "hidden.rs").write_text(
                "pub fn newly_public() {}\n", encoding="utf-8"
            )
            (source_root / "exposed.rs").write_text("", encoding="utf-8")
            private_module_item = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                {"exposed"},
                include_public_inventory=True,
            )
            (source_root / "hidden.rs").write_text("", encoding="utf-8")
            (source_root / "exposed.rs").write_text(
                "pub fn newly_public() {}\n", encoding="utf-8"
            )
            public_module_item = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                {"exposed"},
                include_public_inventory=True,
            )
            self.assertNotEqual(private_module_item, public_module_item)
            self.assertTrue(
                any(
                    "macro-module-surface:exposed:item:fn:newly_public" in anchor
                    for anchor in public_module_item["exposed"]
                )
            )

    def test_public_surface_treats_cfg_attr_and_unguarded_include_as_production(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "crates/sample/src"
            tests_root = source_root / "tests"
            tests_root.mkdir(parents=True)
            source = source_root / "lib.rs"
            source.write_text(
                "pub struct Foo;\n"
                '#[cfg_attr(feature = "only_when_enabled", cfg(test))]\n'
                "mod extension { impl super::Foo { pub fn visible(&self) {} } }\n",
                encoding="utf-8",
            )
            observed = collect_workspace_exported_symbol_surfaces(
                root, Path("crates/sample/src"), {"Foo"}
            )["Foo"]
            self.assertTrue(any(":visible:" in anchor for anchor in observed))

            (tests_root / "api.rs").write_text(
                "pub struct Leaked;\n", encoding="utf-8"
            )
            source.write_text(
                'include!("tests/api.rs");\n', encoding="utf-8"
            )
            included = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                set(),
                include_public_inventory=True,
            )
            self.assertTrue(
                any(
                    ":struct:Leaked:" in anchor
                    for anchor in included[
                        "__lorepia_crate_public_inventory__"
                    ]
                )
            )

            (tests_root / "api.rs").write_text(
                "#[cfg(test)] mod only_tests { pub struct NotProduction; }\n",
                encoding="utf-8",
            )
            test_only_include = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                set(),
                include_public_inventory=True,
            )
            self.assertFalse(
                any(
                    "NotProduction" in anchor
                    for anchor in test_only_include[
                        "__lorepia_crate_public_inventory__"
                    ]
                )
            )

            source.write_text(
                'include!(concat!("tests/", "api.rs"));\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "plain relative"):
                collect_workspace_exported_symbol_surfaces(
                    root,
                    Path("crates/sample/src"),
                    set(),
                    include_public_inventory=True,
                )
            source.write_text('include!("missing.rs");\n', encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "target is missing"):
                collect_workspace_exported_symbol_surfaces(
                    root,
                    Path("crates/sample/src"),
                    set(),
                    include_public_inventory=True,
                )
            outside = root / "outside.rs"
            outside.write_text("pub struct Outside;\n", encoding="utf-8")
            source.write_text(
                'include!("../../../outside.rs");\n', encoding="utf-8"
            )
            with self.assertRaisesRegex(ValueError, "escapes its source root"):
                collect_workspace_exported_symbol_surfaces(
                    root,
                    Path("crates/sample/src"),
                    set(),
                    include_public_inventory=True,
                )

    def test_public_surface_fails_closed_for_unresolved_item_macro_bindings(self) -> None:
        variants = (
            "extern crate lorepia_domain as ld;\nld::make_type!(Foo);\n",
            "#[macro_use] extern crate lorepia_domain;\nmake_type!(Foo);\n",
            "mod macros { pub(crate) use lorepia_domain::make_type; }\n"
            "macros::make_type!(Foo);\n",
            "use lorepia_domain::make_type;\nself::make_type!(Foo);\n",
        )
        for source in variants:
            with self.subTest(source=source):
                with tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    source_root = root / "crates/sample/src"
                    source_root.mkdir(parents=True)
                    (source_root / "lib.rs").write_text(
                        source, encoding="utf-8"
                    )
                    with self.assertRaisesRegex(
                        ValueError, "unresolved production item macro"
                    ):
                        collect_workspace_exported_symbol_surfaces(
                            root,
                            Path("crates/sample/src"),
                            set(),
                            include_public_inventory=True,
                        )

    def test_root_include_resolves_public_modules_and_reexports(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "crates/sample/src"
            source_root.mkdir(parents=True)
            (source_root / "lib.rs").write_text(
                'include!("api.rs");\n', encoding="utf-8"
            )
            api = source_root / "api.rs"
            api.write_text(
                "pub mod exposed;\nmod hidden;\n", encoding="utf-8"
            )
            hidden = source_root / "hidden.rs"
            hidden.write_text(
                "pub struct Foo;\npub fn moved() -> u8 { 0 }\n",
                encoding="utf-8",
            )
            exposed = source_root / "exposed.rs"
            exposed.write_text("", encoding="utf-8")
            baseline = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                set(),
                include_public_inventory=True,
            )

            hidden.write_text("pub struct Foo;\n", encoding="utf-8")
            exposed.write_text(
                "pub fn moved() -> u8 { 0 }\n", encoding="utf-8"
            )
            public_module_move = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                set(),
                include_public_inventory=True,
            )
            self.assertNotEqual(baseline, public_module_move)
            self.assertTrue(
                any(
                    "crate-root-include-surface:module-surface:exposed:"
                    "item:fn:moved" in anchor
                    for anchor in public_module_move[
                        "__lorepia_crate_public_inventory__"
                    ]
                )
            )

            hidden.write_text(
                "pub struct Foo;\npub fn moved() -> u8 { 0 }\n",
                encoding="utf-8",
            )
            exposed.write_text("", encoding="utf-8")
            api.write_text(
                "pub mod exposed;\nmod hidden;\npub use hidden::Foo;\n",
                encoding="utf-8",
            )
            public_reexport = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                set(),
                include_public_inventory=True,
            )
            self.assertNotEqual(baseline, public_reexport)
            self.assertTrue(
                any(
                    "crate-root-include-surface:export:Foo" in anchor
                    for anchor in public_reexport[
                        "__lorepia_crate_public_inventory__"
                    ]
                )
            )

            module_root = source_root / "api"
            module_root.mkdir()
            (source_root / "lib.rs").write_text(
                "pub mod api;\n", encoding="utf-8"
            )
            api.write_text(
                'include!("api/facade.rs");\nmod hidden;\n',
                encoding="utf-8",
            )
            facade = module_root / "facade.rs"
            facade.write_text("", encoding="utf-8")
            (module_root / "hidden.rs").write_text(
                "pub struct NestedFoo;\n", encoding="utf-8"
            )
            nested_baseline = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                {"api"},
                include_public_inventory=True,
            )
            facade.write_text(
                "pub use hidden::NestedFoo;\n", encoding="utf-8"
            )
            nested_reexport = collect_workspace_exported_symbol_surfaces(
                root,
                Path("crates/sample/src"),
                {"api"},
                include_public_inventory=True,
            )
            self.assertNotEqual(nested_baseline, nested_reexport)
            self.assertTrue(
                any(
                    "module-surface:api:include-surface:export:NestedFoo"
                    in anchor
                    for anchor in nested_reexport["api"]
                )
            )

    def test_workspace_public_surface_rejects_alias_and_module_cycles(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "crates/sample/src"
            source_root.mkdir(parents=True)
            (source_root / "aliases.rs").write_text(
                "pub type A = B;\npub type B = A;\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                ValueError, "cyclic public type alias surface: A -> B -> A"
            ):
                collect_workspace_exported_symbol_surfaces(
                    root, Path("crates/sample/src"), {"A"}
                )

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "crates/sample/src"
            module_root = source_root / "parent"
            module_root.mkdir(parents=True)
            (source_root / "parent.rs").write_text(
                '#[path = "parent/support.rs"]\npub mod support;\n',
                encoding="utf-8",
            )
            (module_root / "support.rs").write_text(
                '#[path = "support.rs"]\npub mod again;\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "cyclic public module surface"):
                collect_workspace_exported_symbol_surfaces(
                    root, Path("crates/sample/src"), {"support"}
                )

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "crates/sample/src"
            (source_root / "foo").mkdir(parents=True)
            (source_root / "lib.rs").write_text("pub mod foo;\n", encoding="utf-8")
            (source_root / "foo.rs").write_text("pub fn one() {}\n", encoding="utf-8")
            (source_root / "foo/mod.rs").write_text(
                "pub fn two() {}\n", encoding="utf-8"
            )
            with self.assertRaisesRegex(ValueError, "found 2"):
                collect_workspace_exported_symbol_surfaces(
                    root, Path("crates/sample/src"), {"foo"}
                )

    def test_public_surface_rejects_growth_staleness_and_same_count_swap(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for crate in ("core", "storage"):
                facade = root / f"crates/{crate}/src/lib.rs"
                facade.parent.mkdir(parents=True)
                facade.write_text(
                    "pub fn alpha() {}\n" if crate == "core" else "pub fn stable() {}\n",
                    encoding="utf-8",
                )
            observed = collect_core_storage_public_surface(root)
            config = core_storage_api_config(
                core=tuple(observed["lorepia-core"]),
                storage=tuple(observed["lorepia-storage"]),
            )
            self.assertEqual(evaluate_core_storage_public_surface(root, config), [])

            (root / "crates/core/src/lib.rs").write_text(
                "pub fn beta() {}\n", encoding="utf-8"
            )
            failures = evaluate_core_storage_public_surface(root, config)
            self.assertGreaterEqual(len(failures), 2)
            self.assertTrue(any("growth" in failure and "beta" in failure for failure in failures))
            self.assertTrue(any("stale" in failure and "alpha" in failure for failure in failures))

    def test_core_storage_public_api_baseline_only_shrinks_after_v2(self) -> None:
        base = core_storage_api_config(
            core=("export:A", "export:B"), storage=("export:Storage",)
        )
        smaller = core_storage_api_config(
            core=("export:A",), storage=("export:Storage",)
        )
        swapped = core_storage_api_config(
            core=("export:A", "export:C"), storage=("export:Storage",)
        )

        self.assertEqual(evaluate_core_storage_api_baseline_changes(smaller, base), [])
        failures = evaluate_core_storage_api_baseline_changes(swapped, base)
        self.assertEqual(len(failures), 1)
        self.assertIn("export:C", failures[0])

        legacy = {"version": 1, "allowed_stored_reexports": []}
        self.assertEqual(evaluate_core_storage_api_baseline_changes(base, legacy), [])
        regression = evaluate_core_storage_api_baseline_changes(legacy, base)
        self.assertTrue(any("regressed" in failure for failure in regression))

    def test_dependency_policy_matches_exact_declaration_profiles(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata = dependency_metadata(root)
            policy = dependency_policy(metadata, root)

            self.assertEqual(
                evaluate_dependency_architecture(metadata, policy, root), []
            )
            metadata["packages"].reverse()
            for package in metadata["packages"]:
                package["dependencies"].reverse()
            self.assertEqual(
                evaluate_dependency_architecture(metadata, policy, root), []
            )

    def test_dependency_policy_rejects_edges_profiles_and_path_spoofing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata = dependency_metadata(root)
            policy = dependency_policy(metadata, root)
            orchestration = next(
                package
                for package in metadata["packages"]
                if package["name"] == "lorepia-orchestration"
            )
            domain_dependency = orchestration["dependencies"][0]
            domain_dependency["optional"] = True
            failures = evaluate_dependency_architecture(metadata, policy, root)
            self.assertTrue(any("unapproved direct dependency" in failure for failure in failures))
            self.assertTrue(any("stale dependency policy" in failure for failure in failures))

            domain_dependency["optional"] = False
            domain_dependency["req"] = "^999"
            failures = evaluate_dependency_architecture(metadata, policy, root)
            self.assertTrue(any("req=^999" in failure for failure in failures))
            self.assertTrue(any("req=*" in failure for failure in failures))

            domain_dependency["req"] = "*"
            domain_dependency["path"] = str(root / "vendor/domain-spoof")
            domain_dependency["source"] = None
            failures = evaluate_dependency_architecture(metadata, policy, root)
            self.assertTrue(any("lorepia-domain" in failure for failure in failures))

            domain_dependency["path"] = str(root / "crates/storage")
            failures = evaluate_dependency_architecture(metadata, policy, root)
            self.assertTrue(
                any("orchestration may only depend" in failure for failure in failures)
            )

    def test_dependency_config_and_base_policy_are_monotonic(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            base = dependency_policy(dependency_metadata(root), root)
            smaller = json.loads(json.dumps(base))
            smaller["direct_external_dependencies"] = []
            self.assertEqual(evaluate_dependency_policy_changes(smaller, base), [])

            expanded = json.loads(json.dumps(base))
            expanded["package_features"]["lorepia-orchestration"]["new"] = []
            failures = evaluate_dependency_policy_changes(expanded, base)
            self.assertEqual(len(failures), 1)
            self.assertIn("new package feature", failures[0])

            malformed = json.loads(json.dumps(base))
            malformed["workspace_packages"].reverse()
            with self.assertRaisesRegex(ValueError, "unique and sorted"):
                validate_dependency_architecture_config(malformed)

    def test_existing_giant_may_shrink_but_must_not_grow(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "sample" / "src" / "giant.rs"
            source.parent.mkdir(parents=True)
            original = "fn item() {}\n" * 6
            source.write_text(original, encoding="utf-8")
            relative = source.relative_to(root).as_posix()
            config = write_config(
                root,
                baselines={
                    relative: {
                        "bytes": len(original.encode("utf-8")),
                        "lines": 6,
                    }
                },
            )

            self.assertEqual(evaluate_source_sizes(root, config)[0], [])

            source.write_text(original + "fn grew() {}\n", encoding="utf-8")
            self.assertIn("grew beyond its baseline", evaluate_source_sizes(root, config)[0][0])

            source.write_text("fn smaller() {}\n", encoding="utf-8")
            self.assertEqual(evaluate_source_sizes(root, config)[0], [])

    def test_new_oversized_production_source_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "apps" / "lorepia" / "src" / "new-module.ts"
            source.parent.mkdir(parents=True)
            source.write_text("export const value = 1;\n" * 6, encoding="utf-8")
            config = write_config(root, baselines={})

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), 1)
            self.assertIn("production:typescript source exceeds", failures[0])

    def test_production_file_cannot_hide_under_migration_named_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "sample" / "src" / "migrations" / "hidden.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub fn value() {}\n" * 6, encoding="utf-8")
            config = write_config(root, baselines={})

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), 1)
            self.assertIn("production:rust source exceeds", failures[0])

    def test_test_sources_do_not_count_as_production_but_native_main_sources_do(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sources = [
                root / "crates" / "sample" / "src" / "tests" / "reachable.rs",
                root
                / "plugins"
                / "sample"
                / "android"
                / "src"
                / "main"
                / "java"
                / "Reachable.kt",
                root / "plugins" / "sample" / "ios" / "Sources" / "Reachable.swift",
            ]
            for source in sources:
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_text("public value\n" * 6, encoding="utf-8")
            config = write_config(root, baselines={})

            failures, production = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), 2)
            self.assertNotIn(sources[0].relative_to(root).as_posix(), {item.path for item in production})

            test_config = write_test_config(root, baselines={})
            test_failures, _ = evaluate_test_source_sizes(root, test_config)
            self.assertEqual(len(test_failures), 1)
            self.assertIn(sources[0].relative_to(root).as_posix(), test_failures[0])

    def test_frontend_rust_android_and_ios_tests_are_classified(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = [
                root / "apps/lorepia/src/feature.test.ts",
                root / "apps/lorepia/src/tests/support.ts",
                root / "apps/lorepia/src-tauri/tests/contract.rs",
                root / "crates/sample/tests/integration.rs",
                root / "crates/sample/src/feature/tests.rs",
                root / "crates/sample/src/feature/child_tests.rs",
                root / "plugins/sample/android/src/test/java/PluginTest.kt",
                root / "plugins/sample/android/src/androidTest/java/PluginDeviceTest.kt",
                root / "plugins/sample/ios/Tests/PluginTests.swift",
            ]
            for path in paths:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("test\n", encoding="utf-8")

            observed = test_sources(root)

            self.assertEqual(observed, sorted(path.relative_to(root) for path in paths))
            self.assertTrue(all(is_test_source(path.relative_to(root)) for path in paths))

    def test_existing_test_may_shrink_but_must_not_grow(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates/sample/tests/giant.rs"
            source.parent.mkdir(parents=True)
            original = "fn item() {}\n" * 6
            source.write_text(original, encoding="utf-8")
            relative = source.relative_to(root).as_posix()
            config = write_test_config(
                root,
                baselines={
                    relative: {
                        "bytes": len(original.encode("utf-8")),
                        "lines": 6,
                    }
                },
            )

            self.assertEqual(evaluate_test_source_sizes(root, config)[0], [])
            source.write_text(original + "fn grew() {}\n", encoding="utf-8")
            self.assertIn(
                "grew beyond its test baseline",
                evaluate_test_source_sizes(root, config)[0][0],
            )

    def test_test_baseline_caps_and_exceptions_cannot_grow(self) -> None:
        base = {
            "version": 1,
            "new_test_file_limits": {"bytes": 100, "lines": 5},
            "baselines": {"crates/sample/tests/a.rs": {"bytes": 200, "lines": 10}},
        }
        current = {
            "version": 1,
            "new_test_file_limits": {"bytes": 101, "lines": 5},
            "baselines": {
                "crates/sample/tests/a.rs": {"bytes": 201, "lines": 10},
                "crates/sample/tests/b.rs": {"bytes": 300, "lines": 20},
            },
        }

        failures = evaluate_test_baseline_changes(current, base)

        self.assertEqual(len(failures), 3)

    def test_frontend_production_cannot_import_excluded_test_sources(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "apps" / "lorepia" / "src"
            (source_root / "tests").mkdir(parents=True)
            (source_root / "tests" / "reachable.ts").write_text(
                "export const hidden = 1;\n", encoding="utf-8"
            )
            (source_root / "reachable.test.ts").write_text(
                "export const hidden = 2;\n", encoding="utf-8"
            )
            entry = source_root / "entry.ts"
            entry_text = (
                "import './tests/reachable';\n"
                "import './reachable.test';\n"
                "import/* split token */('.\\\\/tests/reachable');\n"
                "const hidden = import.meta/* split token */.glob(\n"
                "    '.\\u002ftests/*.ts', { eager: true }\n"
                ");\n"
                "const dynamic = import(`./tests/reachable.ts`);\n"
                "const folder = 'tests';\n"
                "const hiddenFolder = import(`./${folder}/reachable.ts`);\n"
                "const kind = 'test';\n"
                "const hiddenSuffix = import(`./reachable.${kind}.ts`);\n"
            )
            entry.write_text(entry_text, encoding="utf-8")
            config = write_config(
                root,
                baselines={
                    entry.relative_to(root).as_posix(): {
                        "bytes": len(entry_text.encode("utf-8")),
                        "lines": len(entry_text.splitlines()),
                    }
                },
            )

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), 7)
            self.assertTrue(all("excluded test source" in failure for failure in failures))

    def test_portable_regex_evaluator_is_worker_only_in_production(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "apps" / "lorepia" / "src" / "features" / "chat"
            source_root.mkdir(parents=True)
            operation = source_root / "portable-regex-operation.ts"
            operation.write_text("export function evaluate() {}\n", encoding="utf-8")
            worker = source_root / "portable-regex.worker.ts"
            worker.write_text(
                "import { evaluate } from './portable-regex-operation';\n",
                encoding="utf-8",
            )
            renderer = source_root / "portable-display.ts"
            renderer.write_text(
                "import { evaluate } from './portable-regex-operation';\n",
                encoding="utf-8",
            )
            config = write_config(root, baselines={})

            failures, _ = evaluate_source_sizes(root, config)

            boundary_failures = [
                failure for failure in failures if "Worker-only portable regex" in failure
            ]
            self.assertEqual(
                boundary_failures,
                [
                    "apps/lorepia/src/features/chat/portable-display.ts imports the "
                    "Worker-only portable regex evaluator"
                ],
            )

    def test_base_revision_prevents_cap_increases_and_new_exceptions(self) -> None:
        base = {
            "version": 1,
            "new_file_limits": {"bytes": 100, "lines": 5},
            "baselines": {"crates/sample/src/giant.rs": {"bytes": 200, "lines": 10}},
        }
        current = {
            "version": 1,
            "new_file_limits": {"bytes": 101, "lines": 5},
            "baselines": {
                "crates/sample/src/giant.rs": {"bytes": 201, "lines": 10},
                "crates/sample/src/new-giant.rs": {"bytes": 300, "lines": 20},
            },
        }

        failures = evaluate_baseline_changes(current, base)

        self.assertEqual(len(failures), 3)

    def test_v2_classifies_generated_test_facade_and_language_before_production(self) -> None:
        orchestration_facade = (
            "apps/lorepia/src/features/orchestration/orchestration-controller.ts"
        )
        facade_paths = {"crates/sample/src/stable.rs", orchestration_facade}

        self.assertEqual(
            classify_source(
                Path("apps/lorepia/src/lib/ipc/commands.generated.ts"), facade_paths
            ),
            ("generated", "typescript"),
        )
        self.assertEqual(
            classify_source(Path("apps/lorepia/src/index.test.ts"), facade_paths),
            ("test", "typescript"),
        )
        self.assertEqual(
            classify_source(Path("crates/sample/src/stable.rs"), facade_paths),
            ("facade", "rust"),
        )
        self.assertEqual(
            classify_source(Path("crates/sample/src/lib.rs"), set()),
            ("facade", "rust"),
        )
        self.assertEqual(
            classify_source(Path(orchestration_facade), facade_paths),
            ("facade", "typescript"),
        )
        self.assertEqual(
            classify_source(Path("crates/sample/src/feature.rs"), facade_paths),
            ("production", "rust"),
        )

    def test_generated_registries_are_scanned_with_generated_limits(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = [
                root / "apps/lorepia/src/lib/ipc/commands.generated.ts",
                root / "apps/lorepia/src-tauri/generated/app_commands.rs",
            ]
            for path in paths:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("generated\n" * 6, encoding="utf-8")
            config = write_config(root, baselines={})

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(
                generated_sources(root), sorted(path.relative_to(root) for path in paths)
            )
            self.assertEqual(len(failures), 2)
            self.assertTrue(all("generated:" in failure for failure in failures))

    def test_explicit_facade_uses_stricter_kind_limit(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates/sample/src/stable.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub fn stable() {}\n" * 6, encoding="utf-8")
            data = source_config(
                baselines={}, facade_paths=[source.relative_to(root).as_posix()]
            )
            data["limits"]["production"]["rust"]["lines"] = 10
            config = root / "source-size-baseline.json"
            config.write_text(json.dumps(data), encoding="utf-8")

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), 1)
            self.assertIn("facade:rust source exceeds", failures[0])

    def test_v2_limit_baseline_bootstrap_and_facade_ratchets_cannot_weaken(self) -> None:
        base = source_config(
            baselines={"crates/sample/src/legacy.rs": {"bytes": 200, "lines": 10}},
            facade_paths=["crates/sample/src/stable.rs"],
        )
        current = json.loads(json.dumps(base))
        current["bootstrap_ref"] = "1" * 40
        current["facade_paths"] = []
        current["limits"]["production"]["rust"]["bytes"] = 101
        current["baselines"]["crates/sample/src/new.rs"] = {
            "bytes": 300,
            "lines": 20,
        }

        failures = evaluate_baseline_changes(current, base)

        self.assertEqual(len(failures), 4)
        self.assertTrue(any("bootstrap_ref" in failure for failure in failures))
        self.assertTrue(any("facade classification" in failure for failure in failures))
        self.assertTrue(any("limit increased" in failure for failure in failures))
        self.assertTrue(any("new baseline exception" in failure for failure in failures))

    def test_v2_parent_child_groups_cannot_shrink(self) -> None:
        parent = "crates/sample/src/stable.rs"
        base = source_config(
            baselines={},
            parent_child_groups={
                parent: [
                    "crates/sample/src/stable-child.rs",
                    "crates/sample/src/stable/",
                ]
            },
        )
        current = json.loads(json.dumps(base))
        current["parent_child_groups"][parent] = [
            "crates/sample/src/stable/"
        ]

        entry_failures = evaluate_baseline_changes(current, base)
        del current["parent_child_groups"][parent]
        group_failures = evaluate_baseline_changes(current, base)

        self.assertEqual(len(entry_failures), 1)
        self.assertIn("aggregate entry cannot be removed", entry_failures[0])
        self.assertEqual(len(group_failures), 1)
        self.assertIn("aggregate group cannot be removed", group_failures[0])

    def test_v1_to_v2_bootstrap_may_capture_existing_files_without_raising_caps(self) -> None:
        base = {
            "version": 1,
            "new_file_limits": {"bytes": 100, "lines": 5},
            "baselines": {},
        }
        current = source_config(
            baselines={"crates/sample/src/existing.rs": {"bytes": 200, "lines": 10}}
        )
        bootstrap = {
            "version": 1,
            "new_file_limits": {"bytes": 100, "lines": 5},
            "baselines": {
                "crates/sample/src/existing.rs": {"bytes": 200, "lines": 10}
            },
        }

        self.assertEqual(
            evaluate_baseline_changes(current, base, bootstrap=bootstrap), []
        )
        self.assertIn(
            "new baseline exception",
            evaluate_baseline_changes(current, base)[0],
        )

    def test_v2_bootstrap_transition_allows_only_enforcement_paths(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            checker = root / "scripts/check_source_architecture.py"
            production = root / "crates/sample/src/lib.rs"
            api_policy = root / "config/core-storage-public-api-baseline.json"
            checker.parent.mkdir(parents=True)
            production.parent.mkdir(parents=True)
            api_policy.parent.mkdir(parents=True)
            checker.write_text("before\n", encoding="utf-8")
            production.write_text("before\n", encoding="utf-8")
            api_policy.write_text(
                '{"version":1,"allowed_stored_reexports":[]}\n',
                encoding="utf-8",
            )
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(
                ["git", "config", "user.email", "test@example.invalid"],
                cwd=root,
                check=True,
            )
            subprocess.run(
                ["git", "config", "user.name", "Test"], cwd=root, check=True
            )
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            subprocess.run(
                ["git", "commit", "-qm", "bootstrap"], cwd=root, check=True
            )
            bootstrap_ref = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=root,
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

            checker.write_text("after\n", encoding="utf-8")
            later_exact_paths = {
                ".github/workflows/ci.yml",
                "config/ai-context-map.json",
                "scripts/check_ai_context_map.py",
                "scripts/check_github_workflow_security.py",
                "scripts/report_refactoring_baseline.py",
                "scripts/test_check_ai_context_map.py",
                "scripts/test_check_github_workflow_security.py",
                "scripts/test_report_refactoring_baseline.py",
            }
            for relative_path in later_exact_paths:
                candidate = root / relative_path
                candidate.parent.mkdir(parents=True, exist_ok=True)
                candidate.write_text("later enforcement\n", encoding="utf-8")
            for relative_path in (
                "config/refactoring/completion.json",
                "docs/refactoring/completion.md",
            ):
                candidate = root / relative_path
                candidate.parent.mkdir(parents=True, exist_ok=True)
                candidate.write_text("later enforcement\n", encoding="utf-8")
            require_v2_bootstrap_transition(root, bootstrap_ref)
            require_enf002_bootstrap_transition(root, bootstrap_ref)

            production.write_text("after\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "unexpected changed path"):
                require_v2_bootstrap_transition(root, bootstrap_ref)
            with self.assertRaisesRegex(ValueError, "unexpected changed path"):
                require_enf002_bootstrap_transition(root, bootstrap_ref)

            production.write_text("before\n", encoding="utf-8")
            untracked = root / "crates/sample/src/new.rs"
            untracked.write_text("pub fn new_surface() {}\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "new.rs"):
                require_v2_bootstrap_transition(root, bootstrap_ref)
            with self.assertRaisesRegex(ValueError, "new.rs"):
                require_enf002_bootstrap_transition(root, bootstrap_ref)

    def test_v2_config_requires_every_language_for_every_kind(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = source_config(baselines={})
            del data["limits"]["generated"]["swift"]
            config = root / "source-size-baseline.json"
            config.write_text(json.dumps(data), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "must define exactly"):
                load_config(config)

    def test_parent_child_config_requires_sorted_entries(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = source_config(
                baselines={},
                parent_child_groups={
                    "crates/sample/src/stable.rs": [
                        "crates/sample/src/z/",
                        "crates/sample/src/a/",
                    ]
                },
            )
            config = root / "source-size-baseline.json"
            config.write_text(json.dumps(data), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "unique, and sorted"):
                load_config(config)

    def test_parent_child_and_directory_aggregate_deltas_are_sorted(self) -> None:
        changes = [
            SourceChange(
                before_path=Path("crates/core/src/app.rs"),
                before_size=SourceSize(bytes=100, lines=10),
                after_path=Path("crates/core/src/app.rs"),
                after_size=SourceSize(bytes=20, lines=2),
            ),
            SourceChange(
                before_path=None,
                before_size=None,
                after_path=Path("crates/core/src/app/generation.rs"),
                after_size=SourceSize(bytes=80, lines=8),
            ),
            SourceChange(
                before_path=Path("apps/lorepia/src/old.ts"),
                before_size=SourceSize(bytes=10, lines=1),
                after_path=None,
                after_size=None,
            ),
        ]

        directories = aggregate_changes(changes, key_for_path=source_directory_key)
        parents = aggregate_changes(
            changes,
            key_for_path=lambda path: baseline_parent_key(
                path, {"crates/core/src/app.rs"}
            ),
        )
        groups = aggregate_parent_child_groups(
            {
                Path("crates/core/src/app.rs"): SourceSize(bytes=100, lines=10),
                Path("crates/core/src/app/existing.rs"): SourceSize(
                    bytes=40, lines=4
                ),
                Path("crates/core/src/app-support.rs"): SourceSize(
                    bytes=30, lines=3
                ),
                Path("crates/core/src/unrelated.rs"): SourceSize(
                    bytes=900, lines=90
                ),
            },
            {
                Path("crates/core/src/app.rs"): SourceSize(bytes=20, lines=2),
                Path("crates/core/src/app/existing.rs"): SourceSize(
                    bytes=50, lines=5
                ),
                Path("crates/core/src/app/generation.rs"): SourceSize(
                    bytes=80, lines=8
                ),
                Path("crates/core/src/app-support.rs"): SourceSize(
                    bytes=35, lines=4
                ),
                Path("crates/core/src/unrelated.rs"): SourceSize(
                    bytes=900, lines=90
                ),
            },
            {
                "crates/core/src/app.rs": [
                    "crates/core/src/app-support.rs",
                    "crates/core/src/app/",
                ]
            },
        )

        self.assertEqual([item.path for item in directories], ["apps/lorepia/src", "crates/core/src"])
        self.assertEqual(
            (parents[0].before_files, parents[0].after_files),
            (1, 1),
        )
        self.assertEqual(
            (parents[0].before_bytes, parents[0].after_bytes),
            (100, 20),
        )
        self.assertEqual(
            (groups[0].before_files, groups[0].after_files),
            (3, 4),
        )
        self.assertEqual(
            (groups[0].before_bytes, groups[0].after_bytes),
            (170, 185),
        )

    def test_git_parent_child_aggregate_covers_full_trees_and_stale_entries(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            parent = Path("crates/sample/src/facade.rs")
            prefix = "crates/sample/src/facade/"
            existing = Path(f"{prefix}existing.rs")
            deleted = Path(f"{prefix}deleted.rs")
            outgoing = Path(f"{prefix}outgoing.rs")
            incoming = Path("crates/sample/src/incoming.rs")
            base_contents = {
                parent: "pub fn facade() {}\n",
                existing: "before\n",
                deleted: "deleted\n",
                outgoing: "outgoing\n",
                incoming: "incoming\n",
            }
            for path, contents in base_contents.items():
                absolute = root / path
                absolute.parent.mkdir(parents=True, exist_ok=True)
                absolute.write_text(contents, encoding="utf-8")
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(
                ["git", "config", "user.email", "test@example.invalid"],
                cwd=root,
                check=True,
            )
            subprocess.run(
                ["git", "config", "user.name", "Test"], cwd=root, check=True
            )
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            subprocess.run(["git", "commit", "-qm", "base"], cwd=root, check=True)
            base_ref = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=root,
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

            existing_after = "after expanded\n"
            generated_contents = "generated\n"
            test_contents = "#[test]\nfn check() {}\n"
            (root / existing).write_text(existing_after, encoding="utf-8")
            (root / deleted).unlink()
            (root / outgoing).rename(root / "crates/sample/src/outgoing.rs")
            incoming_child = root / f"{prefix}incoming.rs"
            (root / incoming).rename(incoming_child)
            (root / f"{prefix}registry.generated.rs").write_text(
                generated_contents, encoding="utf-8"
            )
            (root / f"{prefix}tests.rs").write_text(test_contents, encoding="utf-8")

            aggregates = parent_child_group_deltas(
                root,
                base_ref,
                facade_paths={parent.as_posix()},
                groups={parent.as_posix(): [prefix]},
            )

            before_group = [
                base_contents[parent],
                base_contents[existing],
                base_contents[deleted],
                base_contents[outgoing],
            ]
            after_group = [
                base_contents[parent],
                existing_after,
                base_contents[incoming],
                generated_contents,
                test_contents,
            ]
            self.assertEqual(
                (aggregates[0].before_files, aggregates[0].after_files), (4, 5)
            )
            self.assertEqual(
                (aggregates[0].before_bytes, aggregates[0].after_bytes),
                (
                    sum(len(contents.encode()) for contents in before_group),
                    sum(len(contents.encode()) for contents in after_group),
                ),
            )
            self.assertEqual(
                (aggregates[0].before_lines, aggregates[0].after_lines), (4, 6)
            )

            data = source_config(
                baselines={},
                facade_paths=[parent.as_posix()],
                parent_child_groups={
                    parent.as_posix(): [
                        f"{prefix}missing.rs",
                        "crates/sample/src/missing/",
                    ]
                },
            )
            data["bootstrap_ref"] = base_ref
            config = root / "source-size-baseline.json"
            config.write_text(json.dumps(data), encoding="utf-8")

            failures, _ = evaluate_source_sizes(root, config)
            stale_failures = [
                failure
                for failure in failures
                if failure.startswith("stale parent-child source entry:")
            ]
            self.assertEqual(len(stale_failures), 2)


if __name__ == "__main__":
    unittest.main()
