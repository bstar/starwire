#!/usr/bin/env python3
"""Exercise WIRE's KIT-hosted reader against an isolated replay database."""
import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary', required=True, type=Path)
p.add_argument('--kit', required=True, type=Path, help='KIT checkout containing scripts/terminal_fixture.py')
p.add_argument('--kitty', required=True)
p.add_argument('--kitten', required=True)
p.add_argument('--output', required=True, type=Path)
p.add_argument('--display-server', choices=['wayland', 'x11'], default='wayland')
a = p.parse_args()
root = Path(__file__).resolve().parents[1]
binary = a.binary.resolve()
output = a.output.resolve()
output.mkdir(parents=True, exist_ok=True)
sys.path.insert(0, str(a.kit.resolve() / 'scripts'))
from terminal_fixture import TerminalStartup

with tempfile.TemporaryDirectory(prefix='starwire-ui-proof-') as directory:
    private = Path(directory)
    runtime = private / 'runtime'
    runtime.mkdir(mode=0o700)
    env = {**os.environ, 'STARWIRE_DIR': str(private / 'data'),
           'STARWIRE_CONFIG_DIR': str(private / 'config'), 'HOME': str(private),
           'XDG_CONFIG_HOME': str(private / 'config'), 'XDG_DATA_HOME': str(private / 'share'),
           'XDG_CACHE_HOME': str(private / 'cache'), 'XDG_RUNTIME_DIR': str(runtime),
           'STARWIRE_LOG': 'starwire=debug,starkit=debug', 'STAR_GRAPHICS_RENDERER': 'cpu'}
    for key in ('TMUX', 'TMUX_PANE', 'STY', 'SSH_CONNECTION', 'SSH_TTY', 'NO_COLOR', 'STAR_KIT_ASCII'):
        env.pop(key, None)
    if a.display_server == 'wayland' and os.environ.get('WAYLAND_DISPLAY'):
        env['WAYLAND_DISPLAY'] = str(Path(os.environ['XDG_RUNTIME_DIR']) / os.environ['WAYLAND_DISPLAY'])
    else:
        env.pop('WAYLAND_DISPLAY', None)
    # Replay publication dates are fixed. Production retention must not delete
    # these synthetic articles while a slow CI terminal is starting.
    configuration = private / 'config'
    configuration.mkdir()
    (configuration / 'config.toml').write_text(
        '[articles]\nkeep_days = 36500\n[fetch]\nrefresh_on_start = false\n')
    replay = root / 'testdata/replay'
    subprocess.run([str(binary), '--replay', str(replay), 'fetch'], env=env,
                   capture_output=True, check=True, timeout=30)
    entries = [json.loads(line) for line in subprocess.check_output(
        [str(binary), 'list', '--json'], env=env, timeout=10).decode().splitlines()]
    assert entries, 'Replay database has no articles'
    title = entries[0]['title'][:16]
    address = 'unix:' + str(private / 'kitty.sock')
    startup = TerminalStartup(private)
    log = (output / 'kitty.log').open('w')
    child = subprocess.Popen([a.kitty, '--config', '/dev/null', '--listen-on', address,
        '-o', 'allow_remote_control=yes', '-o', f'linux_display_server={a.display_server}',
        '-o', 'remember_window_size=no', '-o', 'initial_window_width=120c', '-o', 'initial_window_height=50c',
        '--title', 'STAR-WIRE-PRIVATE-UI-PROOF',
        *startup.command(str(binary), '--graphical', '--verbose', '--replay', str(replay))],
        env=env, stdout=log, stderr=log, start_new_session=True)

    def rc(*args):
        return subprocess.check_output([a.kitten, '@', '--to', address, *args], stderr=subprocess.PIPE, timeout=10)

    def wait(check, reason, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            assert child.poll() is None, 'Reader exited: inspect frontend.log and kitty.log'
            try:
                value = check()
                if value:
                    return value
            except (OSError, subprocess.SubprocessError):
                pass
            time.sleep(.1)
        raise AssertionError(reason)

    def text():
        return rc('get-text', '--match', 'id:1').decode()

    def pixels(name):
        path = output / f'{name}.png'
        rc('screenshot', '--match', 'id:1', str(path))
        return subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(path),
            '-vf', 'scale=480:-1:flags=neighbor', '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1'], stderr=subprocess.PIPE, timeout=10)

    def changed(before, name):
        after = pixels(name)
        return after if len(after) != len(before) or sum(abs(x-y)>8 for x,y in zip(before,after)) > len(after)*.0001 else None

    report = {}
    app_log = private / 'data/cache/starwire.log'
    try:
        startup.release(child, rc, output / 'terminal-ready.png')
        wait(lambda: app_log.exists() and 'Graphical frame presented' in app_log.read_text(), 'No native reader frame')
        native = pixels('sources-native')
        rc('send-text', '--match', 'id:1', '\x1b[<35;0;1M\x1b[<35;1;0M\x1b[0;0R')
        rc('send-key', '--match', 'id:1', 'f1')
        wait(lambda: changed(native, 'help-native'), 'Shared Help did not open')
        rc('send-key', '--match', 'id:1', 'escape')
        rc('send-key', '--match', 'id:1', 'f9')
        wait(lambda: 'Source' in text() and 'All' in text(), 'Cell source table did not appear')
        pixels('sources-cells')
        rc('send-key', '--match', 'id:1', 'enter')
        wait(lambda: title in text(), 'Opening All did not show real replay entries')
        pixels('entries-cells')
        rc('send-key', '--match', 'id:1', 'enter')
        wait(lambda: title in text() and 'Age' not in text(), 'Opening an entry did not show the reader')
        pixels('reader-cells')
        rc('send-key', '--match', 'id:1', 'f9')
        wait(lambda: title not in text(), 'Graphical reader did not return')
        reader = pixels('reader-native')
        rc('send-key', '--match', 'id:1', 'alt+f')
        wait(lambda: changed(reader, 'search-native'), 'Search field did not open')
        rc('send-text', '--match', 'id:1', 'fixture query')
        rc('send-key', '--match', 'id:1', 'f9')
        wait(lambda: 'fixture query' in text(), 'Shared field editing did not survive presentation switching')
        pixels('search-cells')
        rc('send-key', '--match', 'id:1', 'escape')
        rc('send-key', '--match', 'id:1', 'f1')
        wait(lambda: 'Keys' in text() and 'Action' in text(), 'Cell Help did not open')
        rc('send-key', '--match', 'id:1', 'q')
        time.sleep(.2)
        assert child.poll() is None and 'Keys' in text(), 'Quit escaped modal isolation'
        rc('send-key', '--match', 'id:1', 'escape')
        rc('send-key', '--match', 'id:1', 'ctrl+q')
        child.wait(timeout=10)
        assert child.returncode == 0
        report.update(native_shell=True, source_entry_article=True, shared_fields=True,
                      cell_graphics_switch=True, modal_isolation=True, clean_exit=True)
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, sort_keys=True))
    finally:
        if app_log.exists():
            (output / 'frontend.log').write_text(app_log.read_text())
        if child.poll() is None:
            try:
                (output / 'last-screen.txt').write_text(text())
                rc('screenshot', '--match', 'id:1', str(output / 'last-screen.png'))
            except (OSError, subprocess.SubprocessError):
                pass
        # A failed proof must not leave its persistent private controller alive.
        for path in runtime.glob('starkit-starwire/*.sock'):
            try:
                with socket.socket(socket.AF_UNIX) as connection:
                    connection.settimeout(1)
                    connection.connect(str(path))
                    viewport = {'columns':120,'rows':50,'width':840,'height':800,'generation':1}
                    messages = [{'type':'hello','version':1,'viewport':viewport,'client':'wire-proof-cleanup'},
                                {'type':'input','id':1,'revision':0,'generation':1,'input':{'kind':'key','code':'char:c','modifiers':2}}]
                    for message in messages:
                        connection.sendall((json.dumps(message)+'\n').encode())
            except OSError:
                pass
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
        log.close()
