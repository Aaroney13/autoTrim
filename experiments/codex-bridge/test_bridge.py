"""Protocol/lifecycle tests. Real backend tests use only synthetic scratch tasks."""
import datetime
import json
import os
from pathlib import Path
import queue
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock
import uuid

from bridge import bridge_arguments
from wire import Client, MAX_MESSAGE, Wire
import desktop_trial

HERE = Path(__file__).parent.resolve()


class ArgumentsTest(unittest.TestCase):
    def test_desktop_overrides_preserved(self):
        args = ['-c', 'features.code_mode_host=true', 'app-server',
                '--analytics-default-enabled', '-c', 'plugins.codex-app-tools@openai-bundled.mcp_servers.codex_app.enabled=true']
        self.assertEqual(bridge_arguments(args), args)
        self.assertEqual(bridge_arguments(args + ['--listen', 'stdio://']), args)

    def test_other_invocations_are_not_intercepted(self):
        for args in [['--version'], ['exec', 'app-server'], ['app-server', '--help'],
                     ['app-server', 'daemon', 'version'], ['app-server', 'generate-ts'],
                     ['app-server', '--listen', 'unix:///tmp/other'],
                     ['-c', 'name=app-server', 'resume', 'task'], ['code-mode-host'],
                     ['app-server', '--config']]:
            with self.subTest(args=args):
                self.assertIsNone(bridge_arguments(args))

    def test_passthrough_output_and_exit_code(self):
        with tempfile.TemporaryDirectory() as root:
            cli = Path(root) / 'fake-codex'
            cli.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\nexit 17\n')
            cli.chmod(0o700)
            result = subprocess.run([sys.executable, str(HERE / 'bridge.py'), 'exec', 'two words'],
                                    env={**os.environ, 'AUTOTRIM_CODEX_REAL_CLI': str(cli)},
                                    capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 17)
            self.assertEqual(result.stdout, b'exec\ntwo words\n')


class WireTest(unittest.TestCase):
    def setUp(self):
        self.left, self.right = socket.socketpair()
        self.left.settimeout(2)
        self.right.settimeout(2)
        self.wire = Wire(self.left)

    def tearDown(self):
        self.wire.close()
        self.right.close()

    def test_fragmented_utf8_and_ping(self):
        self.right.sendall(b'\x01\x02"\xe2' + b'\x89\x01p' + b'\x80\x03\x82\xac"')
        self.assertEqual(self.wire.receive(), '"€"'.encode())
        pong = self.right.recv(7)
        self.assertEqual(pong[0], 0x8a)
        self.assertEqual(bytes([pong[6] ^ pong[2]]), b'p')

    def test_oversized_frame_rejected_before_allocation(self):
        self.right.sendall(b'\x81\x7f' + struct.pack('!Q', MAX_MESSAGE + 1))
        with self.assertRaises(ValueError):
            self.wire.receive()

    def test_server_requests_keep_their_ids(self):
        request = b'{"id":"approval:4","method":"item/commandExecution/requestApproval","params":{}}'
        self.right.sendall(bytes([0x81, len(request)]) + request)
        self.assertEqual(self.wire.receive(), request)

    def test_client_mask_for_large_message(self):
        payload = b'abcde' * 20000
        sender = threading.Thread(target=lambda: self.wire.send_frame(payload))
        sender.start()
        receiver = Wire(self.right)
        first, second = receiver.exact(2)
        self.assertEqual((first, second), (0x81, 0xff))
        size = struct.unpack('!Q', receiver.exact(8))[0]
        mask, body = receiver.exact(4), receiver.exact(size)
        self.assertEqual(bytes(v ^ mask[i % 4] for i, v in enumerate(body)), payload)
        sender.join(2)
        self.assertFalse(sender.is_alive())


class TrialTest(unittest.TestCase):
    def test_success_requires_desktop_initialization(self):
        with tempfile.TemporaryDirectory() as root, mock.patch.dict(os.environ, {'AUTOTRIM_CODEX_BRIDGE_DIR': root}), \
                mock.patch.object(sys, 'argv', ['trial']), mock.patch.object(desktop_trial, 'running', return_value=False), \
                mock.patch.object(desktop_trial, 'launch') as launch, \
                mock.patch.object(desktop_trial.time, 'time', return_value=100), \
                mock.patch.object(desktop_trial, 'Client') as client:
            Path(root, '1.json').write_text(json.dumps({'created_at': 100, 'desktop_initialized_at': 100,
                                                       'backend_pid': 2, 'bridge_pid': 1, 'socket': '/test'}))
            client.return_value.request.return_value = {'data': ['a', 'b']}
            desktop_trial.main()
            launch.assert_called_once_with(True)
            result = json.loads(Path(root, 'last-trial-result.txt').read_text())
            self.assertTrue(result['desktop_connected'])
            self.assertFalse(result['cleanup_enabled'])

    def test_failed_verification_restores_normal_launch(self):
        with tempfile.TemporaryDirectory() as root, mock.patch.dict(os.environ, {'AUTOTRIM_CODEX_BRIDGE_DIR': root}), \
                mock.patch.object(sys, 'argv', ['trial']), mock.patch.object(desktop_trial, 'running', return_value=False), \
                mock.patch.object(desktop_trial, 'launch') as launch, \
                mock.patch.object(desktop_trial.time, 'monotonic', side_effect=[0, 61]):
            with self.assertRaises(RuntimeError):
                desktop_trial.main()
            self.assertEqual(launch.call_args_list, [mock.call(True), mock.call(False)])
            result = json.loads(Path(root, 'last-trial-result.txt').read_text())
            self.assertTrue(result['normal_launch_restored'])
            self.assertFalse(result['desktop_connected'])

    def test_running_desktop_is_not_quit_without_restart_flag(self):
        with tempfile.TemporaryDirectory() as root, mock.patch.dict(os.environ, {'AUTOTRIM_CODEX_BRIDGE_DIR': root}), \
                mock.patch.object(sys, 'argv', ['trial']), mock.patch.object(desktop_trial, 'running', return_value=True), \
                mock.patch.object(desktop_trial, 'quit_app') as quit_app:
            with self.assertRaises(SystemExit):
                desktop_trial.main()
            quit_app.assert_not_called()


class Desktop:
    def __init__(self, process):
        self.process = process
        self.lines = queue.Queue()
        self.events = []
        self.number = 0

        def read():
            for line in process.stdout:
                self.lines.put(json.loads(line))
            self.lines.put(None)
        threading.Thread(target=read, daemon=True).start()

    def send(self, value):
        self.process.stdin.write((json.dumps(value) + '\n').encode())
        self.process.stdin.flush()

    def request(self, method, params):
        self.number += 1
        request_id = 'desktop:' + str(self.number)
        self.send({'id': request_id, 'method': method, 'params': params})
        while True:
            result = self.lines.get(timeout=20)
            if result is None:
                raise RuntimeError('bridge exited unexpectedly')
            if result.get('id') == request_id:
                if 'error' in result:
                    raise RuntimeError(result['error'])
                return result['result']
            self.events.append(result)


@unittest.skipUnless(os.environ.get('AUTOTRIM_TEST_CODEX_CLI'), 'set AUTOTRIM_TEST_CODEX_CLI for isolated backend tests')
class BackendTest(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory(prefix='autotrim-bridge-test-', dir='/tmp')
        self.directory = Path(self.root.name)
        (self.directory / 'codex').mkdir()
        (self.directory / 'sqlite').mkdir()
        self.env = {**os.environ, 'CODEX_HOME': str(self.directory / 'codex'),
                    'CODEX_SQLITE_HOME': str(self.directory / 'sqlite'),
                    'AUTOTRIM_CODEX_BRIDGE_DIR': str(self.directory / 'registry'),
                    'AUTOTRIM_CODEX_REAL_CLI': os.environ['AUTOTRIM_TEST_CODEX_CLI']}
        self.log = (self.directory / 'stderr.log').open('wb')
        self.process = subprocess.Popen([sys.executable, str(HERE / 'bridge.py'),
                                         '-c', 'features.code_mode_host=true', 'app-server'],
                                        env=self.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.log)
        self.desktop = Desktop(self.process)
        try:
            self.desktop.request('initialize', {'clientInfo': {'name': 'autotrim-desktop-test', 'version': '0.1'},
                                               'capabilities': {'experimentalApi': True}})
        except Exception:
            if self.process.poll() is None:
                self.process.terminate()
            self.process.wait(timeout=15)
            self.log.close()
            print((self.directory / 'stderr.log').read_text(), file=sys.stderr)
            self.process.stdin.close()
            self.process.stdout.close()
            self.root.cleanup()
            raise
        self.desktop.send({'method': 'initialized'})
        self.manifest = json.loads(next((self.directory / 'registry').glob('*.json')).read_text())
        self.client = Client(self.manifest['socket'])

    def tearDown(self):
        self.client.wire.close()
        if self.process.poll() is None:
            self.process.terminate()
        self.process.wait(timeout=15)
        for stream in [self.process.stdin, self.process.stdout, self.log]:
            stream.close()
        self.root.cleanup()

    def synthetic_task(self, label):
        task_id = str(uuid.uuid4())
        stamp = datetime.datetime.now(datetime.timezone.utc).isoformat().replace('+00:00', 'Z')
        path = self.directory / 'codex' / 'sessions' / ('rollout-2026-09-14T12-00-00-' + task_id + '.jsonl')
        path.parent.mkdir(parents=True, exist_ok=True)
        rows = [{'timestamp': stamp, 'type': 'session_meta', 'payload': {
                    'id': task_id, 'timestamp': stamp, 'cwd': str(self.directory),
                    'originator': 'codex_cli_rs', 'cli_version': '0.154.0', 'source': 'cli', 'model_provider': 'openai'}},
                {'timestamp': stamp, 'type': 'response_item', 'payload': {
                    'type': 'message', 'role': 'user', 'content': [{'type': 'input_text',
                    'text': 'Synthetic ' + label + '. No action requested.'}]}}]
        path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
        self.desktop.request('thread/resume', {'threadId': task_id, 'path': str(path), 'cwd': str(self.directory)})
        return task_id

    def assert_shutdown(self):
        self.process.wait(timeout=15)
        self.assertFalse(Path(self.manifest['socket']).exists())
        self.assertEqual(list((self.directory / 'registry').glob('*.json')), [])
        with self.assertRaises(ProcessLookupError):
            os.kill(self.manifest['backend_pid'], 0)

    def test_individual_archive_and_read_only_status(self):
        first, second = self.synthetic_task('archive'), self.synthetic_task('keep')
        status = subprocess.run([sys.executable, str(HERE / 'status.py')], env=self.env,
                                capture_output=True, timeout=15, check=True)
        self.assertEqual(len(json.loads(status.stdout)['instances'][0]['tasks']), 2)
        self.assertEqual(self.client.request('thread/unsubscribe', {'threadId': first})['status'], 'notSubscribed')
        self.client.request('thread/archive', {'threadId': first})
        self.assertEqual(self.desktop.request('thread/loaded/list', {})['data'], [second])
        self.assertTrue(any(event.get('method') == 'thread/archived' and event['params']['threadId'] == first
                            for event in self.desktop.events))
        self.assertEqual(self.client.request('thread/unarchive', {'threadId': first})['thread']['id'], first)
        self.assertIsNone(self.process.poll())
        self.process.stdin.close()
        self.assert_shutdown()
        self.assertEqual(self.process.returncode, 0)

    def test_sigterm_removes_only_its_backend_and_registry(self):
        self.process.send_signal(signal.SIGTERM)
        self.assert_shutdown()
        self.assertEqual(self.process.returncode, 128 + signal.SIGTERM)

    def test_backend_exit_is_propagated(self):
        os.kill(self.manifest['backend_pid'], signal.SIGKILL)
        self.assert_shutdown()
        self.assertEqual(self.process.returncode, 128 + signal.SIGKILL)


if __name__ == '__main__':
    unittest.main()
