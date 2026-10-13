"""Kernel alert regressions from actual pre-launch Wi-Fi diagnostics."""

import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import npu_compile_lab as lab


class KernelAlertTests(unittest.TestCase):
    def compare(self, line):
        manifest = SimpleNamespace(serial="wave", policy=SimpleNamespace(poc_id="18"))
        before = {"serials": ["wave"], "dmesg_tail": ["marker"], "processes": []}
        after = {**before, "dmesg_tail": ["marker", line]}
        return lab.compare_health(manifest, before, after)

    def test_wifi_changed_messages_are_not_hangs(self):
        for line in (
            "[1043.704594] [atbm_log]:atbm_bss_info_changed:chatype(3),channelNumber(36)",
            "[1043.757360] [atbm_log]:[wlan0]:dtim changed [0]->[2]",
        ):
            with self.subTest(line=line):
                self.assertEqual(self.compare(line), ([], []))

    def test_actual_wifi_warning_and_timeout_still_stop_the_gate(self):
        for line in (
            "[1032.161067] WARNING: CPU: 0 PID: 119 at drivers/net/wireless/atmb_wifi_40M/wsm.c:2581",
            "[1032.160968] [atbm_log]:wsm_vif_flush_tx:bh_error=1,timeout 2500",
            "[1021.946477] [atbm_log]:<WARNING>wlan0:AP lost",
        ):
            with self.subTest(line=line):
                differences, alerts = self.compare(line)
                self.assertTrue(differences)
                self.assertEqual(alerts, [line])

    def test_npu_hang_variants_still_stop_the_gate(self):
        for line in (
            "[1043] galcore: GPU hang detected",
            "[1043] galcore: hanging command buffer",
            "[1043] galcore: gpu_hang_detected",
            "[1043] galcore: HANGCHECK failed",
        ):
            with self.subTest(line=line):
                differences, alerts = self.compare(line)
                self.assertTrue(differences)
                self.assertEqual(alerts, [line])


if __name__ == "__main__":
    unittest.main()
