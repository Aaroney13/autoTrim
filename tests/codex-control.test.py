"""Read-only metadata fixtures and mocked RPCs; never mutate real tasks."""
import copy
import importlib.util
import json
import pathlib
import sqlite3
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('control', ROOT / 'src/codex_control.py')
control = importlib.util.module_from_spec(spec)
spec.loader.exec_module(control)
# The production launcher embeds these names from wire.py.
import hashlib, os, stat
control.hashlib, control.os, control.stat = hashlib, os, stat

class Client:
    def __init__(self, task):
        self.task = task
        self.peers = {}
        self.calls = []
        self.ids = {task['id'], 'keep'}
        self.wire = mock.Mock()
    def request(self, method, params):
        self.calls.append(method)
        if method == 'thread/read': return {'thread': copy.deepcopy(self.peers.get(params['threadId'], self.task))}
        if method == 'thread/loaded/list': return {'data': list(self.ids)}
        if method == 'thread/archive': self.ids.remove(params['threadId']); return {}
        raise AssertionError(method)

class TaskControl(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = pathlib.Path(self.temp.name)
        path = self.home / 'task.jsonl'; path.write_text('{}\n')
        self.instance = {'backend_pid': 123, 'created_at': 10, 'codex_home': str(self.home)}
        self.task = {'id': 'task', 'path': str(path), 'status': {'type': 'idle'}, 'name': 'Task',
                     'cwd': '/work', 'updatedAt': 10, 'source': 'cli', 'turns': []}
        self.client = Client(self.task)
        self.row = {'id': 'task', 'is_pinned': 0, 'archived': 0, 'rollout_path': str(path), 'updated_at': 10, 'cwd': '/work'}
        self.guards = ({'task': self.row}, set(), set(), set(), set(), set())
        self.request = {'registry': '/unused', 'operation': 'inspect'}
    def inspect(self, request=None):
        return control.inspect_task(self.client, self.instance, 'task', self.guards, request or self.request, True)
    def test_protections(self):
        cases = [
            (lambda: self.task.update(status={'type': 'active'}), 'Active'),
            (lambda: self.row.update(is_pinned=1), 'Pinned'),
            (lambda: self.guards[1].add('task'), 'child'),
            (lambda: self.guards[2].add('task'), 'Helper'),
            (lambda: self.guards[3].add('local:task'), 'Queued'),
            (lambda: self.guards[4].add('task'), 'goal'),
            (lambda: self.guards[5].add('task'), 'automation'),
            (lambda: self.task.update(turns=[{'status': 'inProgress'}]), 'Unfinished'),
            (lambda: self.task.update(turns=[{'status': 'completed', 'items': [{'status':'inProgress'}]}]), 'background'),
        ]
        for change, reason in cases:
            with self.subTest(reason=reason):
                old_task, old_row = copy.deepcopy(self.task), copy.deepcopy(self.row)
                change(); self.assertIn(reason, self.inspect()['protection'])
                self.task.clear(); self.task.update(old_task)
                self.row.clear(); self.row.update(old_row)
                for group in self.guards[1:]: group.clear()
        self.assertEqual(self.inspect({'protected_ids':['task']})['protection'], 'Current task')
        self.assertEqual(self.inspect({'ignore_projects':['/work']})['protection'], 'Excluded project')
    def test_archive_preserves_backend_and_other_task(self):
        reviewed = self.inspect()
        with mock.patch.object(control, 'registry', return_value=[self.instance]), mock.patch.object(control, 'connect', return_value=self.client), mock.patch.object(control, 'protections', return_value=self.guards):
            result = control.dispatch({**self.request, 'operation':'archive', 'task':reviewed})
        self.assertEqual(result['result'], 'archived and verified unloaded')
        self.assertEqual(self.client.ids, {'keep'})
        self.assertEqual(self.client.calls.count('thread/archive'), 1)
    def test_changed_revision_or_protection_never_archives(self):
        for change in ['activity', 'pin', 'descendant', 'queue', 'goal', 'automation', 'transcript']:
            with self.subTest(change=change):
                reviewed = self.inspect()
                if change == 'activity': self.task['updatedAt'] += 1
                elif change == 'pin': self.row['is_pinned'] = 1
                elif change == 'transcript': pathlib.Path(self.task['path']).write_text('changed')
                else: self.guards[{'descendant':1,'queue':3,'goal':4,'automation':5}[change]].add('task')
                with mock.patch.object(control, 'registry', return_value=[self.instance]), mock.patch.object(control, 'connect', return_value=self.client), mock.patch.object(control, 'protections', return_value=self.guards):
                    with self.assertRaises(ValueError): control.dispatch({**self.request,'operation':'archive','task':reviewed})
                self.assertNotIn('thread/archive', self.client.calls)
                self.row['is_pinned'] = 0
                for group in self.guards[1:]: group.clear()
    def test_final_recheck_stops_new_protection(self):
        reviewed = self.inspect()
        second = copy.deepcopy(self.guards); second[1].add('task')
        with mock.patch.object(control, 'registry', return_value=[self.instance]), mock.patch.object(control, 'connect', return_value=self.client), mock.patch.object(control, 'protections', side_effect=[self.guards, second]):
            with self.assertRaises(ValueError): control.dispatch({**self.request,'operation':'archive','task':reviewed})
        self.assertNotIn('thread/archive', self.client.calls)
    def automatic(self):
        config = self.home / 'config.toml'
        config.write_text('auto_archive_codex = true\n')
        meta = config.stat()
        return {'config_path': str(config), 'config_stamp': [meta.st_dev, meta.st_ino, meta.st_size, meta.st_mtime_ns, meta.st_ctime_ns], 'stale_after_secs': 0}
    def test_automatic_guards_and_archive(self):
        reviewed = self.inspect()
        self.client.peers['keep'] = {**self.task, 'id': 'keep', 'updatedAt': reviewed['updated_at'] + 1}
        self.guards[0]['keep'] = {**self.row, 'id': 'keep'}
        automatic = self.automatic()
        request = {**self.request, 'operation': 'archive', 'task': reviewed, 'automatic': automatic}
        with mock.patch.object(control, 'registry', return_value=[self.instance]), mock.patch.object(control, 'connect', return_value=self.client), mock.patch.object(control, 'protections', return_value=self.guards):
            automatic['stale_after_secs'] = 3600
            with self.assertRaisesRegex(ValueError, 'inactive long enough'): control.dispatch(request)
            automatic['stale_after_secs'] = 0
            self.client.ids.remove('keep')
            with self.assertRaisesRegex(ValueError, 'newest remaining'): control.dispatch(request)
            self.client.ids.add('keep')
            self.assertNotIn('thread/archive', self.client.calls)
            self.assertEqual(control.dispatch(request)['result'], 'archived and verified unloaded')
    def test_automatic_settings_change_requires_fresh_warning(self):
        automatic = self.automatic()
        control.automatic_settings(automatic)
        config = pathlib.Path(automatic['config_path'])
        config.write_text('auto_archive_codex = false\n')
        config.write_text('auto_archive_codex = true\n')
        os.utime(config, ns=(config.stat().st_atime_ns, automatic['config_stamp'][3]))
        with self.assertRaisesRegex(ValueError, 'settings changed'): control.automatic_settings(automatic)
    def test_automatic_final_activity_recheck(self):
        reviewed = self.inspect()
        automatic = self.automatic()
        def new_activity(*args): self.task['updatedAt'] += 1
        with mock.patch.object(control, 'registry', return_value=[self.instance]), mock.patch.object(control, 'connect', return_value=self.client), mock.patch.object(control, 'protections', return_value=self.guards), mock.patch.object(control, 'automatic_task', side_effect=new_activity):
            with self.assertRaisesRegex(ValueError, 'changed during automatic'): control.dispatch({**self.request, 'operation': 'archive', 'task': reviewed, 'automatic': automatic})
        self.assertNotIn('thread/archive', self.client.calls)
    def test_backend_identity_change_never_connects(self):
        reviewed = self.inspect(); reviewed['backend_created_at'] += 1
        with mock.patch.object(control, 'registry', return_value=[self.instance]), mock.patch.object(control, 'connect') as connect:
            with self.assertRaises(ValueError): control.dispatch({**self.request,'operation':'archive','task':reviewed})
            connect.assert_not_called()
    def test_missing_metadata_fails_closed(self):
        with self.assertRaises(sqlite3.OperationalError): control.protections(self.home)
    def test_partial_unload_is_not_success(self):
        reviewed = self.inspect()
        original = self.client.request
        def request(method, params):
            if method == 'thread/archive': return {}
            return original(method, params)
        with mock.patch.object(control, 'registry', return_value=[self.instance]), mock.patch.object(control, 'connect', return_value=self.client), mock.patch.object(control, 'protections', return_value=self.guards), mock.patch.object(self.client, 'request', side_effect=request), mock.patch.object(control.time, 'sleep'):
            self.assertEqual(control.dispatch({**self.request,'operation':'archive','task':reviewed})['result'], 'archived; unloading not verified')

if __name__ == '__main__': unittest.main()
