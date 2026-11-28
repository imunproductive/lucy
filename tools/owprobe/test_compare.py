"""The pose comparison tool reads old and platform-aware server traces."""
import tempfile
import unittest
from pathlib import Path

from tools.owprobe.compare import server_poses


class ServerPosesTest(unittest.TestCase):
    def test_parent_fields_do_not_hide_owner_poses_or_state_changes(self):
        lines = [
            'DEBUG owner pose body=A0000101 frame=10 position=[1.0, 2.0, 3.0] '
            'velocity=[0.0, 0.0, 0.0] flags=0x20008 began=["1/0033/0#1"] ended=[]',
            'DEBUG owner pose body=A0000101 frame=11 position=[4.0, 5.0, 6.0] '
            'velocity=[0.0, 3.0, 0.0] flags=0x8 parent=None local=None local_velocity=None '
            'began=[] ended=["1/0033/0#1"] hook=[]',
            'DEBUG owner pose body=A0000101 frame=12 position=[7.0, 8.0, 9.0] '
            'velocity=[1.0, 0.0, 2.0] flags=0x0 parent=Some((256, 3)) '
            'local=Some([1.0, 2.0, 3.0]) local_velocity=Some([0.0, 0.0, 0.0]) '
            'began=["2/005A/1#2"] ended=[] hook=[]',
            'DEBUG owner pose body=A0000201 frame=12 position=[0.0, 0.0, 0.0] '
            'velocity=[0.0, 0.0, 0.0] flags=0x0 parent=None local=None local_velocity=None '
            'began=[] ended=[] hook=[]',
        ]
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "play.log"
            path.write_text("\n".join(lines), encoding="utf-8")
            body, poses = server_poses(path)
            self.assertEqual(body, "A0000101")
            self.assertEqual(poses, {
                10: {"position": [1.0, 2.0, 3.0], "velocity": [0.0, 0.0, 0.0],
                     "flags": 0x20008, "began": ["1/0033/0#1"], "ended": []},
                11: {"position": [4.0, 5.0, 6.0], "velocity": [0.0, 3.0, 0.0],
                     "flags": 0x8, "began": [], "ended": ["1/0033/0#1"]},
                12: {"position": [7.0, 8.0, 9.0], "velocity": [1.0, 0.0, 2.0],
                     "flags": 0x0, "began": ["2/005A/1#2"], "ended": []},
            })
            other, poses = server_poses(path, "A0000201")
            self.assertEqual(other, "A0000201")
            self.assertEqual(poses, {
                12: {"position": [0.0, 0.0, 0.0], "velocity": [0.0, 0.0, 0.0],
                     "flags": 0x0, "began": [], "ended": []},
            })


if __name__ == "__main__":
    unittest.main()
