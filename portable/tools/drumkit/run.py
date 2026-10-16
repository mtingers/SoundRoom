# Fetch one DRSKit instrument at a time, convert it, then delete the raw audio.
import json, os, shutil, subprocess, sys
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
index = os.path.join(root, 'work/drskit-index.json'); raw = os.path.join(root, 'work/raw')
out = sys.argv[1]; builder = sys.argv[2]; jobs = sys.argv[3:]
entries = json.load(open(index))['entries']
for job in jobs:
    instrument, voice, articulation, seconds, *flags = job.split(':')
    names = [e['name'] for e in entries if e['name'].startswith(f'DRSKit/{instrument}/') and not e['name'].endswith('/')]
    subprocess.run([sys.executable, '-I', os.path.join(root, 'tools/zipget.py'), index, raw, *names], check=True, stdout=subprocess.DEVNULL)
    subprocess.run([builder, os.path.join(raw, f'DRSKit/{instrument}/{instrument}.xml'), out, voice, articulation, seconds, *flags], check=True)
    shutil.rmtree(os.path.join(raw, f'DRSKit/{instrument}'))
