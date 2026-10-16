#!/usr/bin/env python3
"""Provision a private local CA and browser-server identity; never install trust."""
import argparse
import ipaddress
import os
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import sys


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='New private directory; existing directories are refused')
    parser.add_argument('--host', action='append', required=True, help='Server DNS name or IP; repeat for each address clients use')
    args = parser.parse_args()
    names = []
    for name in args.host:
        try:
            names.append('IP:' + str(ipaddress.ip_address(name)))
        except ValueError:
            if not re.fullmatch(r'[A-Za-z0-9](?:[A-Za-z0-9.-]{0,251}[A-Za-z0-9])?', name) or '..' in name:
                parser.error('Hosts must be DNS names or IP addresses without paths or shell syntax')
            names.append('DNS:' + name.lower())
    os.umask(0o077)
    root = args.output.expanduser().absolute()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    password = root / 'identity.password'
    password.write_text(secrets.token_urlsafe(48) + '\n')
    extensions = root / 'server.ext'
    extensions.write_text('basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=' + ','.join(dict.fromkeys(names)) + '\n')

    def run(*arguments: str) -> None:
        subprocess.run(['openssl', *arguments], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

    run('genrsa', '-out', str(root / 'ca.key'), '3072')
    run('req', '-x509', '-new', '-sha256', '-key', str(root / 'ca.key'), '-days', '3650',
        '-subj', '/CN=Soundroom local CA', '-out', str(root / 'ca.crt'),
        '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,keyCertSign,cRLSign')
    run('genrsa', '-out', str(root / 'server.key'), '3072')
    run('req', '-new', '-key', str(root / 'server.key'), '-subj', '/CN=Soundroom hardware server', '-out', str(root / 'server.csr'))
    run('x509', '-req', '-in', str(root / 'server.csr'), '-CA', str(root / 'ca.crt'), '-CAkey', str(root / 'ca.key'),
        '-CAcreateserial', '-days', '365', '-sha256', '-extfile', str(extensions), '-out', str(root / 'server.crt'))
    run('pkcs12', '-export', '-inkey', str(root / 'server.key'), '-in', str(root / 'server.crt'),
        '-certfile', str(root / 'ca.crt'), '-name', 'Soundroom web server', '-out', str(root / 'server.p12'), '-passout', 'file:' + str(password))
    run('verify', '-CAfile', str(root / 'ca.crt'), str(root / 'server.crt'))
    fingerprint = subprocess.check_output(['openssl', 'x509', '-in', str(root / 'ca.crt'), '-noout', '-fingerprint', '-sha256'], text=True).strip()
    print(f'Created {root}\n{fingerprint}')
    print('No trust settings were changed. Transfer only ca.crt to clients and verify this fingerprint before trusting it.')
    print('Keep ca.key, server.key, server.p12 and identity.password on the hardware host. Move ca.key to offline storage after provisioning.')
    print('Server arguments: --web-p12 ' + shlex.quote(str(root / 'server.p12')) + ' --web-password-file ' + shlex.quote(str(password)))
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (OSError, subprocess.CalledProcessError) as error:
        print(f'TLS provisioning failed: {error}', file=sys.stderr)
        raise SystemExit(1)
