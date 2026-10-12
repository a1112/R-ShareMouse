import json
import plistlib
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class MacosLanMetadata(unittest.TestCase):
    def test_gui_and_actual_networking_helper_purposes(self):
        config = json.loads((ROOT / "apps/rshare-desktop/src-tauri/tauri.conf.json").read_text())
        info = plistlib.loads((ROOT / "apps/rshare-desktop/src-tauri" / config["bundle"]["macOS"]["infoPlist"]).read_bytes())
        helper = plistlib.loads((ROOT / "apps/rshare-daemon/Info.macos.plist").read_bytes())
        self.assertEqual(helper["CFBundleIdentifier"], "com.rsharemouse.daemon")
        self.assertNotEqual(helper["CFBundleIdentifier"], config["identifier"])
        self.assertEqual(info["NSLocalNetworkUsageDescription"], helper["NSLocalNetworkUsageDescription"])
        self.assertIn("NSAppleEventsUsageDescription", info)  # Existing input/helper flow retained.
        self.assertNotIn("NSBonjourServices", info)  # Raw UDP, not native Bonjour discovery.
        build = (ROOT / "apps/rshare-daemon/build.rs").read_text()
        self.assertIn("CARGO_CFG_TARGET_OS", build)
        self.assertIn("__TEXT,__info_plist", build)
        self.assertIn("Info.macos.plist", build)
        self.assertNotIn("NSCameraUsageDescription", info)

    def test_legacy_generator_and_signature_verification(self):
        script = (ROOT / "bin/macos/build.sh").read_text()
        plist = script[script.index('<?xml'):script.index('</plist>') + len('</plist>')]
        self.assertIn("NSLocalNetworkUsageDescription", plistlib.loads(plist.encode()))
        signing = (ROOT / "scripts/sign-macos-desktop.sh").read_text()
        self.assertIn("Identifier=com.rsharemouse.daemon", signing)
        self.assertIn('codesign --verify --strict --verbose=2 "$bin/rshare-daemon"', signing)


if __name__ == "__main__":
    unittest.main()
