#!/usr/bin/python3
"""Read-only status for live AutoTrim bridge instances. Never resumes tasks."""
import json
import sys

from bridge import registry_dir
from wire import Client


def inspect():
    instances = []
    for path in sorted(registry_dir().glob("*.json")):
        client = None
        try:
            manifest = json.loads(path.read_text())
            client = Client(manifest["socket"])
            loaded = []
            cursor = None
            while True:
                page = client.request("thread/loaded/list", {"cursor": cursor})
                loaded.extend(page["data"])
                cursor = page.get("nextCursor")
                if not cursor:
                    break
            threads = []
            for task_id in loaded:
                task = client.request("thread/read", {"threadId": task_id, "includeTurns": False})["thread"]
                threads.append({key: task.get(key) for key in
                                ("id", "name", "status", "cwd", "updatedAt")})
            instances.append({**manifest, "connected": True, "tasks": threads})
        except Exception as error:
            instances.append({"registry": str(path), "connected": False, "error": str(error)})
        finally:
            if client:
                client.wire.close()
    print(json.dumps({"instances": instances}, indent=2))
    return 0 if instances and all(item["connected"] for item in instances) else 1


if __name__ == "__main__":
    sys.exit(inspect())
