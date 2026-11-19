import os
from pathlib import Path
import runpy
import stat
import sys
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch


SCRIPT = Path(__file__).resolve().parents[1] / "crates/core/src/huggingface_access.py"
REVISION = "ea8dc2863c51be0a264bab82070e3e8836b02d51"


class HuggingFaceAccessTests(unittest.TestCase):
    def setUp(self):
        directory = self.enterContext(TemporaryDirectory())
        self.token_path = Path(directory) / "isolated" / "token"
        self.enterContext(patch.dict(os.environ, HF_HOME=str(self.token_path.parent)))
        self.whoami = Mock()
        self.hub = SimpleNamespace(
            HfApi=Mock(return_value=SimpleNamespace(whoami=self.whoami)),
            get_hf_file_metadata=Mock(return_value=SimpleNamespace(commit_hash=REVISION, size=42)),
            get_token=Mock(return_value=None),
            hf_hub_url=Mock(return_value="checkpoint-url"),
        )
        self.enterContext(patch.dict(sys.modules, huggingface_hub=self.hub))
        module = runpy.run_path(str(SCRIPT))
        self.check_connection = module["check_connection"]
        self.check_model_access = module["check_model_access"]

    def test_missing_token_never_queries_the_hub(self):
        self.assertEqual(self.check_connection(""), "not_connected")
        self.assertEqual(self.check_model_access(REVISION), "not_connected")
        self.whoami.assert_not_called()
        self.hub.get_hf_file_metadata.assert_not_called()
        self.assertFalse(self.token_path.exists())

    def test_existing_token_checks_the_pinned_file_without_saving(self):
        self.hub.get_token.return_value = "hf_saved"
        self.assertEqual(self.check_model_access(REVISION), "verified")
        self.whoami.assert_not_called()
        self.hub.hf_hub_url.assert_called_once_with(
            "facebook/dinov3-vitl16-pretrain-lvd1689m", "model.safetensors",
            revision=REVISION, endpoint="https://huggingface.co",
        )
        self.hub.get_hf_file_metadata.assert_called_once_with(
            "checkpoint-url", token="hf_saved", timeout=10,
        )
        self.assertFalse(self.token_path.exists())

    def test_only_verified_replacement_is_saved_privately(self):
        self.token_path.parent.mkdir()
        self.token_path.write_text("hf_previous")
        denied = RuntimeError("fixture")
        denied.response = SimpleNamespace(status_code=403)
        self.whoami.side_effect = denied
        self.assertEqual(self.check_connection("hf_candidate"), "invalid_token")
        self.assertEqual(self.token_path.read_text(), "hf_previous")
        self.whoami.side_effect = TimeoutError()
        self.assertEqual(self.check_connection("hf_candidate"), "unavailable")
        self.assertEqual(self.token_path.read_text(), "hf_previous")

        self.whoami.side_effect = None
        self.assertEqual(self.check_connection("hf_candidate"), "connected")
        self.hub.get_hf_file_metadata.assert_not_called()
        self.assertEqual(self.token_path.read_text(), "hf_candidate")
        self.assertEqual(list(self.token_path.parent.iterdir()), [self.token_path])
        if os.name == "posix":
            self.assertEqual(stat.S_IMODE(self.token_path.stat().st_mode), 0o600)

    def test_model_denial_or_incomplete_metadata_does_not_change_credentials(self):
        self.token_path.parent.mkdir()
        self.token_path.write_text("hf_previous")
        self.hub.get_token.return_value = "hf_previous"
        denied = RuntimeError("fixture")
        denied.response = SimpleNamespace(status_code=403)
        self.hub.get_hf_file_metadata.side_effect = denied
        self.assertEqual(self.check_model_access(REVISION), "access_denied")
        self.hub.get_hf_file_metadata.side_effect = None
        self.hub.get_hf_file_metadata.return_value = SimpleNamespace(commit_hash=None, size=None)
        self.assertEqual(self.check_model_access(REVISION), "unavailable")
        self.assertEqual(self.token_path.read_text(), "hf_previous")
        self.whoami.assert_not_called()


if __name__ == "__main__":
    unittest.main()
