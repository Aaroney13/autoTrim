# Embedded after codex-bridge/wire.py by codex.rs. No imports from user paths.
# All SQLite access is read-only. Only explicit archive/unarchive requests mutate Codex.
import hashlib
import json
import os
import stat
import pathlib
import sqlite3
import sys
import time
import signal

LIMIT = 256


def bounded_json(path, limit=8 * 1024 * 1024):
    with open(path, 'rb') as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise ValueError('metadata exceeds size limit')
    return json.loads(data)


def database(home, name):
    conn = sqlite3.connect((home / name).as_uri() + '?mode=ro', uri=True, timeout=1)
    conn.row_factory = sqlite3.Row
    conn.execute('PRAGMA query_only = ON')
    return conn


def registry(directory):
    directory = pathlib.Path(directory)
    if not directory.exists():
        return []
    info = directory.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise ValueError('bridge registry is not private')
    files = sorted(directory.glob('*.json'))
    if len(files) > 16:
        raise ValueError('too many bridge instances')
    instances = []
    for path in files:
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise ValueError('bridge manifest is not private')
        item = bounded_json(path, 16384)
        if item.get('schema_version') != 1:
            raise ValueError('unsupported bridge registry version')
        if not pathlib.Path(item['codex_home']).is_absolute():
            raise ValueError('invalid Codex home')
        if any(type(item.get(k)) is not int or item[k] <= 0 for k in ('backend_pid', 'bridge_pid', 'created_at')):
            raise ValueError('invalid bridge process identity')
        os.kill(item['backend_pid'], 0)
        os.kill(item['bridge_pid'], 0)
        instances.append(item)
    return instances


def loaded(client):
    found, cursor, cursors = [], None, set()
    while True:
        page = client.request('thread/loaded/list', {'cursor': cursor})
        found.extend(page['data'])
        if len(found) > LIMIT:
            raise ValueError('too many loaded tasks to verify')
        cursor = page.get('nextCursor')
        if not cursor:
            return set(found)
        if cursor in cursors:
            raise ValueError('repeated loaded-task cursor')
        cursors.add(cursor)


def protections(home):
    # Schema changes fail closed. Never infer absence of protection from an
    # unreadable/missing store in a desktop installation.
    with database(home, 'state_5.sqlite') as db:
        rows = {r['id']: dict(r) for r in db.execute(
            'SELECT id, is_pinned, archived, rollout_path, updated_at, cwd FROM threads')}
        edges = list(db.execute('SELECT parent_thread_id, child_thread_id FROM thread_spawn_edges'))
    with database(home, 'queue_1.sqlite') as db:
        queued = {r[0] for r in db.execute('SELECT DISTINCT thread_id FROM queued_items')}
    with database(home, 'goals_1.sqlite') as db:
        goals = {r[0] for r in db.execute("SELECT thread_id FROM thread_goals WHERE status != 'complete'")}
    with database(home, 'sqlite/codex-dev.db') as db:
        automated = {r[0] for r in db.execute('SELECT target_thread_id FROM automations WHERE target_thread_id IS NOT NULL')}
        automated.update(r[0] for r in db.execute('SELECT thread_id FROM automation_runs'))
    global_state = bounded_json(home / '.codex-global-state.json')
    followups = global_state.get('queued-follow-ups')
    if not isinstance(followups, dict):
        raise ValueError('queued follow-up metadata unavailable')
    queued.update(k for k, v in followups.items() if v)
    # The desktop database and automation.toml can briefly disagree. Any UUID
    # referenced in a schedule protects that task, including paused schedules.
    automation_dir = home / 'automations'
    if not automation_dir.is_dir():
        raise ValueError('automation metadata unavailable')
    import re
    for path in automation_dir.glob('*/automation.toml'):
        if path.stat().st_size > 1024 * 1024:
            raise ValueError('automation metadata exceeds size limit')
        automated.update(re.findall(r'[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}', path.read_text()))
    return rows, {r[0] for r in edges}, {r[1] for r in edges}, queued, goals, automated


def inspect_task(client, instance, task_id, guards, request, thorough=False):
    task = client.request('thread/read', {'threadId': task_id, 'includeTurns': thorough})['thread']
    if task['id'] != task_id:
        raise ValueError('task identity changed')
    cwd = os.path.realpath(task['cwd']) if task.get('cwd') else ''
    rows, parents, children, queued, goals, automated = guards
    row = rows.get(task_id)
    state = task.get('status', {}).get('type', 'unknown')
    reason = None
    if state != 'idle':
        reason = 'Active work or unknown activity'
    elif not row or row['archived'] or not task.get('path') or task.get('ephemeral'):
        reason = 'Persisted task identity unavailable'
    elif row['is_pinned']:
        reason = 'Pinned task'
    elif task_id in parents:
        reason = 'Has child tasks · archive would cascade'
    elif task_id in children or task.get('parentThreadId') or isinstance(task.get('source'), dict):
        reason = 'Helper task · managed by its parent'
    elif task_id in queued or any(task_id in k for k in queued):
        reason = 'Queued work'
    elif task_id in goals:
        reason = 'Unfinished goal'
    elif task_id in automated:
        reason = 'Linked to an automation'
    elif task_id in request.get('protected_ids', []):
        reason = 'Current task'
    elif any(x and (x in cwd or x in task.get('cwd', '')) for x in request.get('ignore_projects', [])):
        reason = 'Excluded project'
    elif any(x in ('Codex', 'ChatGPT', 'Codex app', 'ChatGPT app') for x in request.get('ignore_apps', [])):
        reason = 'Excluded app'
    path = pathlib.Path(task.get('path') or '/')
    if row and str(path) != row['rollout_path']:
        reason = 'Transcript identity changed'
    stamp = None
    if reason is None:
        meta = path.stat()
        if not path.is_file():
            raise ValueError('transcript unavailable')
        stamp = [meta.st_dev, meta.st_ino, meta.st_size, meta.st_mtime_ns]
    if thorough and reason is None:
        # Read full turn/item states only during a reviewed action. Unfinished
        # tool calls/approvals protect the task even if its summary says idle.
        for turn in task['turns']:
            if turn.get('status') not in ('completed', 'interrupted', 'failed'):
                reason = 'Unfinished turn'
            for item in turn.get('items', []):
                if item.get('status') in ('inProgress', 'in_progress', 'pending', 'running', 'waiting'):
                    reason = 'Unfinished tool or background work'
    identity = {'backend_pid': instance['backend_pid'], 'backend_created_at': instance['created_at'],
                'id': task_id, 'codex_home': instance['codex_home']}
    revision = hashlib.sha256(json.dumps([identity, task.get('updatedAt'), task.get('recencyAt'),
                                          row, stamp], sort_keys=True).encode()).hexdigest()
    return {**identity, 'revision': revision, 'name': task.get('name') or task_id,
            'cwd': cwd, 'transcript': str(path),
            'updated_at': max(task.get('updatedAt', 0), task.get('recencyAt') or 0,
                              stamp[3] // 1000000000 if stamp else 0), 'state': state, 'protection': reason}


def automatic_settings(automatic):
    meta = pathlib.Path(automatic['config_path']).stat()
    stamp = [meta.st_dev, meta.st_ino, meta.st_size, meta.st_mtime_ns, meta.st_ctime_ns]
    if stamp != automatic['config_stamp']:
        raise ValueError('automatic settings changed; a fresh warning is required')


def automatic_task(client, instance, task, guards, request):
    automatic = request.get('automatic')
    if not automatic:
        return
    automatic_settings(automatic)
    if not task['cwd'] or task['updated_at'] <= 0 or time.time() - task['updated_at'] < automatic['stale_after_secs']:
        raise ValueError('task has not been inactive long enough')
    # Recheck the newest-task safeguard on the live owning backend. If a newer
    # task disappeared since the daemon scan, retain this one even after grace.
    peers = [inspect_task(client, instance, other, guards, request)
             for other in loaded(client) if other != task['id']]
    if not any(peer['cwd'] == task['cwd'] and peer['updated_at'] > task['updated_at'] for peer in peers):
        raise ValueError('newest remaining task in its project; kept open')


def connect(instance):
    return Client(instance['socket'], name='autotrim-task-control')


def dispatch(request):
    instances = registry(request['registry'])
    operation = request['operation']
    if operation == 'inspect':
        output = []
        for instance in instances:
            client = None
            backend = {'pid': instance['backend_pid'], 'tasks': [], 'error': None}
            try:
                client = connect(instance)
                guards = protections(pathlib.Path(instance['codex_home']))
                for task_id in sorted(loaded(client)):
                    backend['tasks'].append(inspect_task(client, instance, task_id, guards, request))
            except Exception as error:
                backend['tasks'] = []
                backend['error'] = str(error)
            finally:
                if client:
                    client.wire.close()
            output.append(backend)
        return output
    expected = request['task']
    candidates = [i for i in instances if i['codex_home'] == expected['codex_home'] and
                  (operation == 'restore' or (i['backend_pid'] == expected['backend_pid'] and
                   i['created_at'] == expected['backend_created_at']))]
    if len(candidates) != 1:
        raise ValueError('owning backend changed or is unavailable; refresh and review again')
    instance = candidates[0]
    client = connect(instance)
    try:
        if operation == 'restore':
            with database(pathlib.Path(instance['codex_home']), 'state_5.sqlite') as db:
                row = db.execute('SELECT archived FROM threads WHERE id = ?', (expected['id'],)).fetchone()
            if not row:
                raise ValueError('archived task no longer exists')
            if not row['archived']:
                return {'result': 'already restored'}
            result = client.request('thread/unarchive', {'threadId': expected['id']})
            if result['thread']['id'] != expected['id']:
                raise ValueError('restored task identity mismatch')
            with database(pathlib.Path(instance['codex_home']), 'state_5.sqlite') as db:
                row = db.execute('SELECT archived FROM threads WHERE id = ?', (expected['id'],)).fetchone()
            if not row or row['archived']:
                raise ValueError('restoration could not be verified')
            return {'result': 'restored'}
        if operation not in ('prepare', 'archive'):
            raise ValueError('unknown operation')
        if expected['id'] not in loaded(client):
            raise ValueError('task is no longer loaded; refresh and review again')
        guards = protections(pathlib.Path(instance['codex_home']))
        task = inspect_task(client, instance, expected['id'], guards, request, thorough=True)
        if task['protection']:
            raise ValueError(task['protection'])
        if task['revision'] != expected['revision']:
            raise ValueError('task changed since review; refresh and review again')
        if operation == 'prepare':
            return task
        # Re-read protection stores and runtime immediately before the mutation.
        final = inspect_task(client, instance, expected['id'], protections(pathlib.Path(instance['codex_home'])), request)
        if final['protection'] or final['revision'] != task['revision']:
            raise ValueError('task changed during final checks; kept open')
        automatic_task(client, instance, final, guards, request)
        if request.get('automatic'):
            automatic_settings(request['automatic'])
            # Peer checks can take time; sample the target again at the boundary.
            latest = inspect_task(client, instance, expected['id'], protections(pathlib.Path(instance['codex_home'])), request)
            if latest['protection'] or latest['revision'] != task['revision']:
                raise ValueError('task changed during automatic checks; kept open')
            automatic_settings(request['automatic'])
        client.request('thread/archive', {'threadId': expected['id']})
        for _ in range(10):
            if expected['id'] not in loaded(client):
                return {'result': 'archived and verified unloaded'}
            time.sleep(.1)
        return {'result': 'archived; unloading not verified'}
    finally:
        client.wire.close()


def control_main():
    # Absolute wall-clock deadline also bounds unsolicited notifications.
    signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(TimeoutError('Codex operation timed out')))
    signal.alarm(20)
    try:
        raw = sys.stdin.buffer.read(1024 * 1024 + 1)
        if len(raw) > 1024 * 1024:
            raise ValueError('request exceeds size limit')
        result = dispatch(json.loads(raw))
        print(json.dumps({'ok': result}))
    except Exception as error:
        print(json.dumps({'error': str(error)}))
    finally:
        signal.alarm(0)


if __name__ == '__main__':
    control_main()
