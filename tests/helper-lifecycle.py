"""Check crash cleanup without connecting to Discord; run after a release build."""
from pathlib import Path
import json
import os
import signal
import subprocess
import sys
import time

root=Path(__file__).resolve().parents[1]
helpers=list((root/'src-tauri/target/release/build').glob('mimic-linux-*/out/dummy'))
assert helpers, 'Build the application first'
helper=max(helpers,key=lambda p:p.stat().st_mtime)
runner='''import os,subprocess,sys,time
p=subprocess.Popen([sys.argv[1],str(os.getpid())],env={})
print(p.pid,flush=True)
time.sleep(30)
'''
parent=subprocess.Popen([sys.executable,'-c',runner,str(helper)],stdout=subprocess.PIPE,text=True)
child=int(parent.stdout.readline())
try:
 time.sleep(.15)
 assert os.path.exists(f'/proc/{child}'), 'Helper exited unexpectedly'
 assert os.readlink(f'/proc/{child}/exe'), 'Executable must be visible to Discord'
 assert 'NoNewPrivs:\t1' in Path(f'/proc/{child}/status').read_text()
 parent.kill();parent.wait(timeout=5)
 end=time.monotonic()+3
 while time.monotonic()<end:
  try: status=Path(f'/proc/{child}/status').read_text()
  except FileNotFoundError: break
  if 'State:\tZ' in status: break  # terminated, awaiting system reaper
  time.sleep(.03)
 else: raise AssertionError('Helper survived parent death')
 print('Helper lifecycle: visible executable, no_new_privs and SIGKILL parent-death cleanup — passed.')
finally:
 if parent.poll() is None: parent.kill();parent.wait()
 try: os.kill(child,signal.SIGKILL)
 except ProcessLookupError: pass
