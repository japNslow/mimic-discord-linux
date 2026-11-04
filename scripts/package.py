"""Package the built native binary and source without build caches or credentials."""
from pathlib import Path
import hashlib
import json
import shutil
import subprocess
import tarfile

root=Path(__file__).resolve().parents[1]
version=json.loads((root/'package.json').read_text())['version']
output=root/'dist-linux'
output.mkdir(exist_ok=True)
binary=root/'src-tauri/target/release/mimic-linux'
if not binary.is_file(): raise SystemExit('Run npm run linux:portable first')
shutil.copy2(binary,output/'mimic-linux')
release=f'mimic-linux-{version}-linux-x86_64'
with tarfile.open(output/f'{release}.tar.gz','w:gz')as archive:
 for source,name in [(binary,'mimic-linux')]+[(root/name,name)for name in ['README.md','SECURITY.md','AUDIT.md','LICENSE','run.sh','docs/preview.png']]:
  archive.add(source,arcname=f'{release}/{name}')
files=subprocess.check_output(['git','ls-files','--cached','--others','--exclude-standard','-z'],cwd=root).decode().split('\0')
with tarfile.open(output/f'mimic-linux-{version}-source.tar.gz','w:gz')as archive:
 for name in sorted(set(files)):
  if name and (root/name).is_file():archive.add(root/name,arcname=f'mimic-linux-source/{name}',recursive=False)
with (output/'SHA256SUMS').open('w')as checksums:
 for f in [output/'mimic-linux',*sorted(output.glob('*.tar.gz'))]:
  checksums.write(f'{hashlib.file_digest(f.open("rb"),"sha256").hexdigest()}  {f.name}\n')
print('Packaged:',*(p.name for p in output.iterdir()),sep='\n')
