"""Package a native Briefcase executable with public documentation and hashes."""
from pathlib import Path
import argparse
import hashlib
import platform
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

ROOT=Path(__file__).resolve().parents[1]

def native_target():
    machine=platform.machine().lower()
    arch={'x86_64':'x86_64','amd64':'x86_64','arm64':'aarch64','aarch64':'aarch64'}.get(machine)
    system={'Linux':'unknown-linux-gnu','Darwin':'apple-darwin','Windows':'pc-windows-msvc'}.get(platform.system())
    if not arch or not system: raise RuntimeError('Unsupported native release platform')
    return arch+'-'+system

def package(binary, target, destination):
    destination.mkdir(parents=True,exist_ok=True)
    windows='windows' in target
    name='briefcase-'+target+('.zip' if windows else '.tar.gz')
    archive=destination/name
    with tempfile.TemporaryDirectory(prefix='briefcase-package-') as staging:
        # The archive is flat, so repository paths in the README must resolve there.
        readme=Path(staging)/'README.md'
        readme.write_text((ROOT/'README.md').read_text().replace('](docs/DISTRIBUTION.md)','](DISTRIBUTION.md)'))
        entries={('briefcase.exe' if windows else 'briefcase'):binary,'LICENSE':ROOT/'LICENSE',
                 'README.md':readme,'DISTRIBUTION.md':ROOT/'docs'/'DISTRIBUTION.md'}
        if windows:
            with zipfile.ZipFile(archive,'w',compression=zipfile.ZIP_DEFLATED) as out:
                for name,path in entries.items():out.write(path,name)
        else:
            with tarfile.open(archive,'w:gz') as out:
                for name,path in entries.items():out.add(path,arcname=name,recursive=False)
    digest=hashlib.sha256(archive.read_bytes()).hexdigest()
    (destination/'SHA256SUMS').write_text(f'{digest}  {archive.name}\n')
    return archive

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target',default=native_target())
    parser.add_argument('--binary',type=Path)
    parser.add_argument('--out',type=Path,default=ROOT/'dist')
    args=parser.parse_args()
    binary=args.binary or ROOT/'target/release'/('briefcase.exe' if 'windows' in args.target else 'briefcase')
    if not binary.is_file():raise RuntimeError('Build the native release executable first')
    print(package(binary,args.target,args.out))

if __name__=='__main__':main()
