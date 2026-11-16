import hashlib
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

import quadriflow_windows_archive_qa as qa


class FixedWindowsArchiveInputTests(unittest.TestCase):
    def test_pinned_bytes_are_required_and_never_replaced(self):
        payload = b"a small reviewed file\n"
        fingerprint = hashlib.sha256(payload).hexdigest()
        with TemporaryDirectory() as temp, patch.dict(
            qa.INPUT_PINS, {"boost": (len(payload), fingerprint)},
        ):
            path = Path(temp) / "public-notice.txt"
            path.write_bytes(payload)
            self.assertEqual(qa.read_input("boost", path), payload)
            path.write_bytes(payload.lower().replace(b"small", b"smell"))
            with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                qa.read_input("boost", path)
            path.write_bytes(payload + b"x")
            with self.assertRaisesRegex(ValueError, "invalid boost input"):
                qa.read_input("boost", path)

    def test_declared_windows_package_pin_matches_review_scope(self):
        self.assertEqual(qa.PLATFORM, "windows-x86_64")
        self.assertEqual(qa.EXPECTED_ARCHIVE_BYTES, 4_239_360)
        self.assertEqual(
            qa.EXPECTED_ARCHIVE_SHA256,
            "9037e53b8bdcc4192638bc5afdd5efa64a7b1c0ae0cc2e3aaa1731418bfee8f6",
        )


if __name__ == "__main__":
    unittest.main()
