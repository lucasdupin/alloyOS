#!/usr/bin/env python3
"""Interactive local setup for run.sh. Never prints or passes keys in argv."""
import argparse
import getpass
import os
from pathlib import Path
import shutil
import subprocess
import sys
import termios

ROOT = Path(__file__).resolve().parent.parent
PROVIDERS = {
    '1': ('Claude', 'ANTHROPIC_API_KEY', 'https://platform.claude.com/settings/keys',
          'Sign in to the Claude Console, create a key in Settings → API keys, and configure API billing.'),
    '2': ('Gemini', 'GEMINI_API_KEY', 'https://aistudio.google.com/apikey',
          'Sign in to Google AI Studio, select or create a project, and create an API key. Review its quota and billing.'),
    '3': ('OpenAI (models behind ChatGPT)', 'OPENAI_API_KEY', 'https://platform.openai.com/api-keys',
          'Create an OpenAI project API key and configure API billing. A ChatGPT subscription does not include API billing.'),
}

class SetupError(ValueError):
    """Safe, actionable setup diagnostics authored here."""

def normalize_terminal_input():
    """Repair input modes left behind by QEMU or an interrupted raw-mode app."""
    if not sys.stdin.isatty():
        return
    fd = sys.stdin.fileno()
    modes = termios.tcgetattr(fd)
    modes[0] &= ~(termios.IGNCR | termios.INLCR)
    modes[0] |= termios.ICRNL
    modes[1] |= termios.OPOST | termios.ONLCR
    modes[3] |= termios.ICANON | termios.ECHO | termios.ISIG | termios.IEXTEN
    modes[6][termios.VMIN] = 1
    modes[6][termios.VTIME] = 0
    # Do not flush anything the user has already typed. Keep the repaired modes
    # on exit, so cancellation also returns the shell to normal line input.
    termios.tcsetattr(fd, termios.TCSANOW, modes)

def choose(prompt, options, default):
    while True:
        answer = input(f'{prompt} [{default}]: ').strip().lower() or default
        if answer in options:
            return answer
        print('Choose one of: ' + ', '.join(options))

def save_secret(name, value):
    directory = ROOT / '.secrets'
    directory.mkdir(mode=0o700, exist_ok=True)
    if directory.is_symlink():
        raise SetupError('The .secrets directory must not be a symlink. Use a regular local directory, then rerun.')
    directory.chmod(0o700)
    path = directory / name
    # Exclusive creation avoids silently overwriting a key or following a symlink.
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as output:
        output.write(value + '\n')
    return path

def credential(name):
    path = ROOT / '.secrets' / name
    if os.environ.get(name, '').strip():
        print(f'Using {name} from your environment; its value stays hidden.')
        return ('--auth-env', name)
    if path.parent.is_symlink() or path.is_symlink():
        raise SetupError('Saved credentials must use regular files under a regular .secrets directory; remove the symlink and rerun.')
    if path.exists():
        if path.is_symlink() or path.stat().st_mode & 0o077:
            raise SetupError(f'Make .secrets/{name} a regular private file (chmod 600), then rerun.')
        if not path.read_text().strip():
            raise SetupError(f'.secrets/{name} is empty. Remove it and rerun to enter a key.')
        print(f'Reusing your private .secrets/{name} file.')
        return ('--auth-file', str(path))
    print('Paste into the hidden prompt below. Input is not echoed or added to shell history.')
    while True:
        value = getpass.getpass(f'{name} (Enter to cancel): ').strip()
        if not value:
            raise KeyboardInterrupt
        if any(c in value for c in '\n\r\0') or len(value.encode()) > 16000:
            print('Use a single token, under 16 KiB.')
            continue
        break
    if choose('Save privately for future launches? y/n', ('y', 'n'), 'y') == 'y':
        path = save_secret(name, value)
        print(f'Saved .secrets/{name} with owner-only permissions; Git ignores this directory.')
        return ('--auth-file', str(path))
    os.environ[name] = value
    print('Using this credential for this setup session only.')
    return ('--auth-env', name)

def main():
    normalize_terminal_input()
    parser = argparse.ArgumentParser(description=__doc__)
    auth = parser.add_mutually_exclusive_group()
    auth.add_argument('--auth-env')
    auth.add_argument('--auth-file')
    parser.add_argument('--setup-cloud', action='store_true', help='Show cloud setup even when a provider is already configured')
    args, launch = parser.parse_known_args()
    print('\nWelcome to metalOS. We’ll prepare a local model, explain cloud access, then launch.\n')
    print('Step 1 of 4 — Build tools')
    missing = [name for name in ('cargo', 'qemu-system-aarch64', 'curl') if not shutil.which(name)]
    if missing:
        print('Missing: ' + ', '.join(missing))
        print('Install Rust from https://rustup.rs and QEMU (on macOS: brew install qemu), then rerun.')
        return 1
    print('Cargo, QEMU, and curl are available.')
    print('\nStep 2 of 4 — Optional cloud API access')
    print('Forge supports Claude, Gemini, and OpenAI through verified HTTPS.')
    print('Gemma remains the default. Use /model claude, /model gemini, or /model openai in chat.')
    configured = []
    for title, name, _, _ in PROVIDERS.values():
        path = ROOT / '.secrets' / name
        if os.environ.get(name, '').strip() or path.exists() or path.is_symlink():
            credential(name)  # Validate private files without displaying their contents.
            configured.append(title)
    if configured and not args.setup_cloud:
        print('Already configured: ' + ', '.join(configured) + '. Skipping cloud setup.')
        print('To add another provider later, run ./run.sh --setup-cloud.')
        selected = '0'
    else:
        print('1 Claude   2 Gemini   3 OpenAI / ChatGPT models   4 All three   0 Skip')
        selected = choose('Which setup instructions would you like?', ('0', '1', '2', '3', '4'), '0')
    for key in (('1', '2', '3') if selected == '4' else (() if selected == '0' else (selected,))):
        title, name, url, instructions = PROVIDERS[key]
        print(f'\n{title}\n  1. Open {url}\n  2. {instructions}\n  3. Return here with the API key.')
        if choose('Configure a key now? y/n', ('y', 'n'), 'n') == 'y':
            credential(name)
            print('Credential configured. Forge will enable this provider at launch.')
    print('\nStep 3 of 4 — Resident Gemma')
    image = ROOT / '.local/models/gemma-2-2b-it/model.img'
    if image.is_file():
        print('Local Gemma image found; no download needed.')
    else:
        print('This downloads about 10.5 GB, uses about 21 GB disk space, and needs 16 GiB guest RAM.')
        print('1. Sign in at https://huggingface.co/google/gemma-2-2b-it-GGUF and accept the Gemma license.')
        print('2. Create a read token at https://huggingface.co/settings/tokens with access to this gated model.')
        print('3. Return here. Use the same account for the license and token.')
        if choose('Ready to download? y/n', ('y', 'n'), 'y') == 'n':
            print('Setup stopped. Rerun ./run.sh when ready; saved credentials will be reused.')
            return 0
        reference = ('--auth-file', args.auth_file) if args.auth_file else (('--auth-env', args.auth_env) if args.auth_env else credential('HF_TOKEN'))
        while True:
            result = subprocess.run([sys.executable, str(ROOT / 'scripts/download_gemma.py'), *reference], cwd=ROOT)
            if result.returncode == 0:
                break
            if choose('Fix the issue shown above, then retry? y/n', ('y', 'n'), 'n') == 'n':
                return 1
    print('\nStep 4 of 4 — Build and launch')
    print('Building release mode and starting the UI with Gemma attached. The first build may take a while.')
    if choose('Launch now? y/n', ('y', 'n'), 'y') == 'n':
        print('Ready for next time. Run ./run.sh to launch.')
        return 0
    # Downloader credentials are unnecessary for the already-prepared model.
    env = dict(os.environ)
    for name in ('HF_TOKEN', args.auth_env):
        if name:
            env.pop(name, None)
    os.execve(str(ROOT / 'run.sh'), ['run.sh', '--non-interactive', *launch], env)

if __name__ == '__main__':
    try:
        sys.exit(main())
    except (KeyboardInterrupt, EOFError):
        print('\nSetup cancelled. Rerun ./run.sh to continue.')
        sys.exit(130)
    except SetupError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except (OSError, ValueError):
        print('Setup could not read or save a private credential. Check .secrets permissions and token-file format, then rerun.', file=sys.stderr)
        sys.exit(1)
