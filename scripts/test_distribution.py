"""Release installers verify downloaded bytes before replacing an executable."""
from pathlib import Path
import hashlib
import importlib.util
import os
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('package_release',ROOT/'scripts/package_release.py')
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)


@unittest.skipUnless(os.name=='posix' and shutil.which('sh'),'POSIX installer test needs sh')
class DistributionTests(unittest.TestCase):
    def test_archive_is_small_and_installer_verifies_before_replacing(self):
        with tempfile.TemporaryDirectory(prefix='briefcase-dist-') as temporary:
            root=Path(temporary); fake=root/'briefcase'
            fake.write_text('#!/bin/sh\necho "briefcase 0.1.0"\n');fake.chmod(0o755)
            target=module.native_target()
            archive=module.package(fake,target,root/'assets')
            with tarfile.open(archive) as bundle:
                self.assertEqual(set(bundle.getnames()),{'briefcase','LICENSE','README.md','DISTRIBUTION.md'})
            tools=root/'tools';tools.mkdir()
            curl=tools/'curl'
            curl.write_text('#!/bin/sh\nset -eu\ncase "$4" in */SHA256SUMS) cp "$FIXTURE_ASSETS/SHA256SUMS" "$3";; *) cp "$FIXTURE_ASSETS/'+archive.name+'" "$3";; esac\n')
            curl.chmod(0o755)
            env={**os.environ,'PATH':str(tools)+os.pathsep+os.environ['PATH'],'FIXTURE_ASSETS':str(root/'assets')}
            command=['sh',str(ROOT/'scripts/install.sh'),'--repo','example/briefcase','--version','v0.1.0','--dir',str(root/'installed')]
            valid=subprocess.run(command,env=env,capture_output=True,text=True)
            self.assertEqual(valid.returncode,0,valid.stdout+valid.stderr)
            installed=root/'installed/briefcase';self.assertEqual(installed.read_bytes(),fake.read_bytes())
            installed.write_text('preserve local installation')
            archive.write_bytes(archive.read_bytes()+b'tampered')
            invalid=subprocess.run(command,env=env,capture_output=True,text=True)
            self.assertNotEqual(invalid.returncode,0)
            self.assertIn('checksum mismatch',invalid.stderr)
            self.assertEqual(installed.read_text(),'preserve local installation')

if __name__=='__main__':unittest.main()
