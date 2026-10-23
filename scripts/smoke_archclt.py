"""Check a packaged Briefcase binary through the released Archclt Python mapper."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import yaml


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--briefcase',type=Path,required=True)
    binary=parser.parse_args().briefcase.resolve()
    with tempfile.TemporaryDirectory(prefix='briefcase-archclt-release-') as temporary:
        root=Path(temporary).resolve();(root/'src').mkdir();(root/'architecture').mkdir()
        (root/'src/a.py').write_text('import b\n')
        (root/'src/b.py').write_text('value = 1\n')
        model={'schema_version':'archctl.authoring.v2','enforcement':'strict','system_id':'release-smoke',
               'name':'Native release smoke','domains':[{'id':'app','name':'Application','owners':['release-test']}],
               'components':[{'id':name,'name':name,'domain':'app','kind':'library','paths':[f'src/{name}.py']} for name in ['a','b']]}
        (root/'architecture/model.yaml').write_text(yaml.safe_dump(model,sort_keys=False))
        subprocess.run(['git','init','-q',str(root)],check=True)
        subprocess.run(['git','-C',str(root),'add','.'],check=True)
        subprocess.run(['git','-C',str(root),'-c','user.name=Release fixture','-c','user.email=fixture@example.invalid',
                        'commit','-qm','fixture source'],check=True)
        cli=[sys.executable,'-I','-m','archctl','--root',str(root)]
        export=subprocess.run([*cli,'briefcase','export-evidence','--briefcase',str(binary),'--source-root','src'],
                              check=True,capture_output=True,text=True)
        bundle=json.loads((root/'.archctl/evidence.json').read_text())
        if bundle['schema_version']!='archctl.evidence.v2':raise RuntimeError('Wrong exchange version')
        if bundle['coverage']['status']!='complete' or bundle['coverage']['gaps']:
            raise RuntimeError('Incomplete native Python import coverage')
        if {(edge['source'],edge['target']) for edge in bundle['edges']}!={('a','b')}:
            raise RuntimeError('Native import edge was not mapped')
        subprocess.run([*cli,'evidence','validate','--input','.archctl/evidence.json'],check=True,capture_output=True,text=True)
        print(json.dumps({'status':'passed','mapping':'Python imports','schema':'archctl.evidence.v2',
                          'components':2,'mapped_edges':1,'coverage':'complete'}))
    return 0

if __name__=='__main__':raise SystemExit(main())
