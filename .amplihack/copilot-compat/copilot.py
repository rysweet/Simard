#!/usr/bin/env python3
import importlib.util
import subprocess
import sys

REAL_BINARY = '/tmp/amplihack-copilot-compat-qraqndki/copilot'
MODULE_PATH = '/home/azureuser/src/amplihack/src/amplihack/recipes/rust_runner_copilot.py'

def normalize(args):
    spec = importlib.util.spec_from_file_location("amplihack_rust_runner_copilot", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Could not load compatibility module from {MODULE_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module._normalize_copilot_cli_args(args)


def main():
    cmd = [REAL_BINARY] + normalize(sys.argv[1:])
    completed = subprocess.run(cmd, check=False)
    raise SystemExit(completed.returncode)


if __name__ == "__main__":
    main()
