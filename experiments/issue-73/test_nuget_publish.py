"""Finite, offline regression checks for the job's actual publication controller."""
import sys
sys.dont_write_bytecode = True
import importlib.util
from pathlib import Path
import tempfile
import unittest
import zipfile

MODULE = Path(__file__).resolve().parents[2] / "js/src/nuget-publish.py"
spec = importlib.util.spec_from_file_location("nuget_publish", MODULE)
controller = importlib.util.module_from_spec(spec)
spec.loader.exec_module(controller)


class PublicationTests(unittest.TestCase):
    def test_normalized_public_identity(self):
        self.assertEqual(controller.normalized_version("01.02.003.0+build.45"), "1.2.3")
        self.assertEqual(controller.normalized_version("1.0"), "1.0.0")
        self.assertEqual(controller.normalized_version("1.2.3.4-Preview.2"), "1.2.3.4-preview.2")
        for invalid in ["1.2.3-", "1.2.3-?", "1.2.3-a..b", "1.2.3+"]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                controller.normalized_version(invalid)

    def test_oidc_then_unchanged_fallback(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "library.nupkg"
            with zipfile.ZipFile(artifact, "w") as archive:
                archive.writestr("library.nuspec", "<package><metadata><id>Org.Library</id><version>1.2.3</version></metadata></package>")
            calls = []
            def push(file, key):
                calls.append((file.read_bytes(), key))
                return "accepted" if key == "fallback" else "authentication-failed"
            result = controller.publish(artifact, "oidc", "fallback", push, lambda *_: False)
            self.assertEqual(result["selected_method"], "token-fallback")
            self.assertFalse(result["trust_repaired"])
            self.assertEqual(calls[0][0], calls[1][0])
            result = controller.publish(artifact, "oidc", "fallback", lambda _, key: "authorization-failed" if key == "oidc" else "accepted", lambda *_: False)
            self.assertEqual(result["selected_method"], "token-fallback")
            calls.clear()
            result = controller.publish(artifact, "oidc", "invalid", lambda *_: "accepted", lambda *_: False)
            self.assertEqual(result["selected_method"], "trusted-publishing")
            for probe, status in [(lambda *_: None, "unknown"), (lambda *_: True, "already-present")]:
                result = controller.publish(artifact, "oidc", "fallback", push, probe)
                self.assertEqual(result["publication"], status)
                self.assertEqual(calls, [])
            result = controller.publish(artifact, "oidc", "fallback", lambda *_: "push-failed", lambda *_: False)
            self.assertEqual(len(result["attempts"]), 2)
            self.assertEqual(result["publication"], "unverified")


if __name__ == "__main__":
    unittest.main()
