import unittest

from .acceptance import terminal_json, event_data


class TerminalResponseTests(unittest.TestCase):
    def test_diagnostic_objects_do_not_replace_the_command_response(self):
        output = (
            b'Hive info auth.login phase="started"\r\n'
            b'Hive warn cleanup.secondary_failed error={"kind":"io"}\r\n'
            b'{\r\n  "state": "authenticated"\r\n}\r\n'
        )
        self.assertEqual(terminal_json(output), {"state": "authenticated"})

    def test_missing_incomplete_and_multiple_responses_are_not_accepted(self):
        for output in [b"Hive info auth.login\r\n", b'{"state":', b"{}\r\n{}\r\n"]:
            with self.subTest(output=output), self.assertRaises(ValueError):
                terminal_json(output)


class EventArtifactTests(unittest.TestCase):
    def test_inline_and_utf8_artifacts(self):
        self.assertEqual(event_data(None, {"text": "inline"}), {"text": "inline"})
        chunks = ['{"text":"é', '😀"}']
        offsets = [len(chunks[0].encode()), len("".join(chunks).encode())]
        calls = []

        def read(*args):
            index = len(calls)
            calls.append(args)
            return dict(text=chunks[index], next_offset=offsets[index], eof=index == 1)

        value = event_data(read, {"artifact": {"id": "out", "bytes": offsets[-1]}})
        self.assertEqual(value, {"text": "é😀"})
        self.assertEqual(
            calls,
            [
                ("output", "out", "--offset", "0", "--limit", "65536"),
                ("output", "out", "--offset", str(offsets[0]), "--limit", "65536"),
            ],
        )

    def test_invalid_size_progress_eof_and_json_are_refused(self):
        for size in [True, 0, -1, 1024 * 1024 + 1]:
            with self.subTest(size=size), self.assertRaises(RuntimeError):
                event_data(None, {"artifact": {"id": "out", "bytes": size}})
        for page in [
            dict(text="{}", next_offset=0, eof=False),
            dict(text="{}", next_offset=3, eof=True),
            dict(text="é", next_offset=1, eof=False),
            dict(text="{}", next_offset=2, eof=False),
            dict(text="{}", next_offset=True, eof=True),
            dict(text="{}", next_offset=2, eof=1),
            dict(text="[]", next_offset=2, eof=True),
        ]:
            with self.subTest(page=page), self.assertRaises(RuntimeError):
                event_data(lambda *args: page, {"artifact": {"id": "out", "bytes": 2}})
        with self.assertRaises(ValueError):
            event_data(
                lambda *args: dict(text="xx", next_offset=2, eof=True),
                {"artifact": {"id": "out", "bytes": 2}},
            )
