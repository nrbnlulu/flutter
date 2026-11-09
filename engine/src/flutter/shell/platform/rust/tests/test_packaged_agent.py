#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Verify a generated application using only extracted Flutter/Rust SDK archives."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for flag in ('rlutter','flutter-archive','rust-archive','artifacts'):parser.add_argument('--'+flag,type=Path,required=True)
    parser.add_argument('--modes',nargs='+',default=['debug','profile','release'],choices=['debug','profile','release'])
    parser.add_argument('--project',type=Path)
    parser.add_argument('--work-dir',type=Path)
    parser.add_argument('--cargo-target-dir',type=Path,help='Optional shared build cache; SDK sources remain physically extracted')
    args=parser.parse_args()
    output=args.artifacts.resolve();output.mkdir(parents=True,exist_ok=False)
    temporary=None if args.work_dir else tempfile.TemporaryDirectory(prefix='rlutter-packaged-agent-')
    work=args.work_dir.resolve() if args.work_dir else Path(temporary.name)
    work.mkdir(parents=True,exist_ok=True)
    flutter=work/'flutter-sdk';flutter.mkdir()
    with tarfile.open(args.flutter_archive.resolve()) as archive:archive.extractall(flutter,filter='data')
    (flutter/'.rlutter-complete').touch()
    if (flutter/'engine').exists():raise RuntimeError('Standalone Flutter SDK must not include a local engine checkout')
    project=args.project.resolve() if args.project else work/'app'
    project.mkdir(parents=True,exist_ok=False)
    sdk=project/'.dart_tool/flutter_rs/sdk';sdk.mkdir(parents=True)
    with tarfile.open(args.rust_archive.resolve()) as archive:archive.extractall(sdk,filter='data')
    if any(path.is_symlink() for path in sdk.rglob('*')):raise RuntimeError('Rust source/engine archive contains a link to an external checkout')
    metadata=json.loads((sdk/'sdk-manifest.json').read_text());assert metadata['shellAbi']==16,metadata
    cli=args.rlutter.resolve()
    env={k:v for k,v in os.environ.items() if k not in ('DISPLAY','WAYLAND_DISPLAY','FLUTTER_ROOT','FLUTTER_RUST_ENGINE_DIR')}
    env['RUSTUP_TOOLCHAIN']='1.93.1'
    def run(arguments,log,ok=True):
        with (output/log).open('w') as stream:
            result=subprocess.run([str(cli),'--flutter-root',str(flutter),*arguments],cwd=project,env=env,stdout=stream,stderr=stream,timeout=1800)
        if (result.returncode==0)!=ok:raise RuntimeError(f'Unexpected command result: {arguments}; see {output/log}')
    try:
        run(['create','.','--project-name','packaged_agent_fixture'],'create.log')
        if args.cargo_target_dir:
            cached=args.cargo_target_dir.resolve();cached.mkdir(parents=True,exist_ok=True)
            target=project/'runner-rs/target'
            if target.exists():target.rmdir()
            target.symlink_to(cached,target_is_directory=True)
        shutil.copy2(sdk/'tests/agent_fixture/lib/main.dart',project/'lib/main.dart')
        spec=project/'pubspec.yaml'
        spec.write_text(spec.read_text().replace('dependencies:\n','dependencies:\n  flutter_rust_agent:\n    path: .dart_tool/flutter_rs/sdk/dart/flutter_rust_agent\n',1))
        scenario=sdk/'tests/agent_fixture/test/agent/save_dialog.json'
        reports={}
        for mode in args.modes:
            report=output/mode/'result.json'
            run(['verify',str(scenario),'--headless',f'--{mode}','--pixel-ratio=1.25','--report',str(report)],f'{mode}.log')
            value=json.loads(report.read_text())
            assert value['status']=='passed' and value['session']['buildMode']==mode,value
            assert value['session']['shellAbi']==metadata['shellAbi']
            assert value['session']['capabilities']['frameworkErrors']
            # Paths may point into the extracted SDK/project, never the original
            # checkout's crates, bridge or engine out directories.
            for relative in ['crates','third_party','dart']:
                assert not (sdk/relative).is_symlink()
            reports[mode]=value['status']
        # Ordinary release omits the opt-in bridge define and opens no endpoint.
        if 'release' in args.modes:
            run(['build','linux','--release'],'ordinary-release.log')
            runner=project/'runner-rs/target/release/packaged_agent_fixture'
            # Launch without any agent variable: native API returns no control endpoint.
            clean={k:v for k,v in env.items() if not k.startswith('FLUTTER_RUST_AGENT')}
            clean['FLUTTER_RUST_HEADLESS']='1'
            with (output/'ordinary-release-clean.log').open('w') as log:
                ordinary=subprocess.Popen([str(runner)],cwd=project,env=clean,stdout=log,stderr=log)
                try:
                    import time
                    time.sleep(1)
                    assert ordinary.poll() is None
                    owned=set()
                    for fd in Path(f'/proc/{ordinary.pid}/fd').iterdir():
                        try:
                            link=os.readlink(fd)
                            if link.startswith('socket:['):owned.add(link[8:-1])
                        except FileNotFoundError:pass
                    for row in Path(f'/proc/{ordinary.pid}/net/unix').read_text().splitlines()[1:]:
                        fields=row.split()
                        assert not (fields[6] in owned and len(fields)>7 and ('control.sock' in fields[7] or 'flutter-rs-agent-' in fields[7])),row
                    assert 'fixture Dart bridge enabled: false' in (output/'ordinary-release-clean.log').read_text()
                finally:
                    ordinary.terminate();ordinary.wait(timeout=10)
        (output/'report.json').write_text(json.dumps({'status':'passed','modes':reports,'sdk':metadata,'project':str(project)},indent=2)+'\n')
        print(f'PASS: {output}')
    finally:
        if temporary:temporary.cleanup()


if __name__=='__main__':main()
