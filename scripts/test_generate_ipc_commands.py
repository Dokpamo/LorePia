import io
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path


SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from generate_ipc_commands import (
    BASE_TAURI_CAPABILITY,
    BASE_TAURI_CONFIG_RELATIVE,
    CAPABILITY_SELECTIONS,
    MANIFEST_RELATIVE,
    PERMISSIONS_RELATIVE,
    RUST_OUTPUT_RELATIVE,
    TYPESCRIPT_OUTPUT_RELATIVE,
    capability_selection_failures,
    load_commands,
    main,
    permission_artifact_failures,
    render_permission,
    render_rust,
    render_typescript,
)


class IpcCommandCodegenTests(unittest.TestCase):
    def write_manifest(self, root: Path, commands: list[object]) -> Path:
        path = root / MANIFEST_RELATIVE
        path.parent.mkdir(parents=True)
        path.write_text(
            json.dumps({"version": 1, "commands": commands}),
            encoding="utf-8",
        )
        return path

    def write_repository_gates(
        self,
        root: Path,
        commands: list[str],
        *,
        track_permissions: bool = True,
    ) -> None:
        base_config = root / BASE_TAURI_CONFIG_RELATIVE
        base_config.parent.mkdir(parents=True, exist_ok=True)
        base_config.write_text(
            json.dumps(
                {
                    "app": {
                        "security": {"capabilities": [BASE_TAURI_CAPABILITY]}
                    }
                }
            ),
            encoding="utf-8",
        )
        for _, config_relative, identifier, capability_relative in CAPABILITY_SELECTIONS:
            config_path = root / config_relative
            config_path.parent.mkdir(parents=True, exist_ok=True)
            config_path.write_text(
                json.dumps(
                    {"app": {"security": {"capabilities": [identifier]}}}
                ),
                encoding="utf-8",
            )
            capability_path = root / capability_relative
            capability_path.parent.mkdir(parents=True, exist_ok=True)
            capability_path.write_text(
                json.dumps(
                    {
                        "identifier": identifier,
                        "local": True,
                        "windows": ["main"],
                        "permissions": [
                            "core:window:allow-start-dragging",
                            *[
                                f"allow-{command.replace('_', '-')}"
                                for command in commands
                            ],
                        ],
                    }
                ),
                encoding="utf-8",
            )

        permission_root = root / PERMISSIONS_RELATIVE
        permission_root.mkdir(parents=True, exist_ok=True)
        for command in commands:
            (permission_root / f"{command}.toml").write_text(
                render_permission(command),
                encoding="utf-8",
            )

        subprocess.run(
            ["git", "init", "-q"],
            cwd=root,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        if track_permissions:
            subprocess.run(
                ["git", "add", "--", PERMISSIONS_RELATIVE.as_posix()],
                cwd=root,
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )

    def test_generates_matching_rust_and_typescript_names(self) -> None:
        commands = ("bootstrap", "get_portable_runtime_state", "put_portable_runtime_state")

        rust = render_rust(commands)
        typescript = render_typescript(commands)

        for command in commands:
            self.assertIn(f'"{command}"', rust)
            self.assertIn(f"'{command}'", typescript)
        self.assertIn("getPortableRuntimeState", typescript)
        self.assertIn("putPortableRuntimeState", typescript)

    def test_rejects_invalid_and_duplicate_names(self) -> None:
        invalid_values = [
            ["bootstrap", "bootstrap"],
            ["not-kebab-case"],
            ["Not_Snake_Case"],
            [7],
        ]
        for values in invalid_values:
            with self.subTest(values=values), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                manifest = self.write_manifest(root, values)
                with self.assertRaises(ValueError):
                    load_commands(manifest)

    def test_rejects_unreviewed_manifest_fields(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = self.write_manifest(root, ["bootstrap"])
            manifest.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "commands": ["bootstrap"],
                        "release_capabilities": ["bootstrap"],
                    }
                ),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(ValueError, "only version and commands"):
                load_commands(manifest)

    def test_rejects_snake_case_names_with_same_typescript_key(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = self.write_manifest(root, ["a1", "a_1"])

            with self.assertRaisesRegex(
                ValueError, "duplicate IPC TypeScript key: a1"
            ):
                load_commands(manifest)

    def test_permission_gate_rejects_untracked_and_modified_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            commands = ["bootstrap", "get_portable_runtime_state"]
            self.write_repository_gates(
                root,
                commands,
                track_permissions=False,
            )

            failures = permission_artifact_failures(root, commands)
            self.assertTrue(any("not tracked by git" in failure for failure in failures))

            subprocess.run(
                ["git", "add", "--", PERMISSIONS_RELATIVE.as_posix()],
                cwd=root,
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            permission = root / PERMISSIONS_RELATIVE / "bootstrap.toml"
            permission.write_text("stale\n", encoding="utf-8")

            failures = permission_artifact_failures(root, commands)
            self.assertTrue(
                any("non-canonical content" in failure for failure in failures)
            )

    def test_permission_gate_rejects_missing_and_unexpected_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            commands = ["bootstrap"]
            self.write_repository_gates(root, commands)
            (root / PERMISSIONS_RELATIVE / "bootstrap.toml").unlink()
            unexpected = root / PERMISSIONS_RELATIVE / "retired_command.toml"
            unexpected.write_text(
                render_permission("retired_command"),
                encoding="utf-8",
            )

            failures = permission_artifact_failures(root, commands)

            self.assertTrue(any("missing generated" in failure for failure in failures))
            self.assertTrue(any("unexpected generated" in failure for failure in failures))

    def test_capability_gate_rejects_crossed_mode_selection(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_repository_gates(root, ["bootstrap"])
            base_config = root / BASE_TAURI_CONFIG_RELATIVE
            base_config.write_text(
                json.dumps({"app": {"security": {"capabilities": []}}}),
                encoding="utf-8",
            )
            development_config = root / CAPABILITY_SELECTIONS[0][1]
            development_config.write_text(
                json.dumps(
                    {"app": {"security": {"capabilities": ["main-release"]}}}
                ),
                encoding="utf-8",
            )

            failures = capability_selection_failures(root, ["bootstrap"])

            self.assertEqual(
                failures,
                [
                    f"{BASE_TAURI_CONFIG_RELATIVE} must select only capability "
                    "main-development; an empty list enables every capability file",
                    f"{CAPABILITY_SELECTIONS[0][1]} must select only capability "
                    "main-development"
                ],
            )

    def test_capability_gate_rejects_mismatched_capability_identifier(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_repository_gates(root, ["bootstrap"])
            release_capability = root / CAPABILITY_SELECTIONS[1][3]
            release_capability.write_text(
                json.dumps(
                    {
                        "identifier": "main-development",
                        "local": True,
                        "windows": ["main"],
                        "permissions": ["allow-bootstrap"],
                    }
                ),
                encoding="utf-8",
            )

            failures = capability_selection_failures(root, ["bootstrap"])

            self.assertEqual(
                failures,
                [
                    f"{CAPABILITY_SELECTIONS[1][3]} identifier must be main-release"
                ],
            )

    def test_capability_gate_rejects_inexact_app_grants(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            commands = ["bootstrap", "get_portable_runtime_state"]
            self.write_repository_gates(root, commands)
            development_capability = root / CAPABILITY_SELECTIONS[0][3]
            development_capability.write_text(
                json.dumps(
                    {
                        "identifier": "main-development",
                        "local": True,
                        "windows": ["main"],
                        "permissions": [
                            "core:window:allow-start-dragging",
                            "allow-bootstrap",
                            "allow-bootstrap",
                            "allow-retired-command",
                        ],
                    }
                ),
                encoding="utf-8",
            )

            failures = capability_selection_failures(root, commands)

            self.assertTrue(any("duplicate app-command" in failure for failure in failures))
            self.assertTrue(any("exactly the manifest" in failure for failure in failures))

    def test_capability_gate_rejects_broadened_renderer_scope(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_repository_gates(root, ["bootstrap"])
            release_capability = root / CAPABILITY_SELECTIONS[1][3]
            release_capability.write_text(
                json.dumps(
                    {
                        "identifier": "main-release",
                        "local": False,
                        "windows": ["*"],
                        "remote": {"urls": ["https://example.invalid"]},
                        "permissions": ["allow-bootstrap"],
                    }
                ),
                encoding="utf-8",
            )

            failures = capability_selection_failures(root, ["bootstrap"])

            self.assertTrue(any("local-only" in failure for failure in failures))
            self.assertTrue(any("only the main window" in failure for failure in failures))
            self.assertTrue(any("remote origins" in failure for failure in failures))

    def test_check_mode_never_writes_and_detects_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            commands = ["bootstrap", "get_portable_runtime_state"]
            self.write_manifest(root, commands)
            self.write_repository_gates(root, commands)
            output = io.StringIO()

            with redirect_stdout(output):
                self.assertEqual(main(["--root", str(root), "--check"]), 1)
            self.assertFalse((root / RUST_OUTPUT_RELATIVE).exists())
            self.assertFalse((root / TYPESCRIPT_OUTPUT_RELATIVE).exists())

            with redirect_stdout(output):
                self.assertEqual(main(["--root", str(root)]), 0)
                self.assertEqual(main(["--root", str(root), "--check"]), 0)

            (root / TYPESCRIPT_OUTPUT_RELATIVE).write_text("stale\n", encoding="utf-8")
            with redirect_stdout(output):
                self.assertEqual(main(["--root", str(root), "--check"]), 1)
            self.assertEqual(
                (root / TYPESCRIPT_OUTPUT_RELATIVE).read_text(encoding="utf-8"),
                "stale\n",
            )


if __name__ == "__main__":
    unittest.main()
