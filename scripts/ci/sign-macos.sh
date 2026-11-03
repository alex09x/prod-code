#!/usr/bin/env bash
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail
set +x
umask 077
: "${APPLE_CERTIFICATE_P12_BASE64:?Missing protected signing certificate}"
: "${APPLE_CERTIFICATE_PASSWORD:?Missing certificate password}"
: "${APPLE_NOTARY_KEY_P8:?Missing notary API key}"
: "${APPLE_NOTARY_KEY_ID:?Missing notary key ID}" "${APPLE_NOTARY_ISSUER_ID:?Missing issuer}"
: "${APPLE_SIGNING_IDENTITY:?Missing signing identity}" "${RUNNER_TEMP:?}"

work=$(mktemp -d "$RUNNER_TEMP/prod-code-sign.XXXXXX")
keychain="$work/release.keychain-db"
cleanup() {
    security delete-keychain "$keychain" >/dev/null 2>&1 || true
    rm -rf "$work"
}
trap cleanup EXIT
export SIGNING_WORK="$work"
python3 - <<'PY'
import base64, os
from pathlib import Path
p = Path(os.environ['SIGNING_WORK'])
(p / 'identity.p12').write_bytes(base64.b64decode(os.environ['APPLE_CERTIFICATE_P12_BASE64'], validate=True))
(p / 'AuthKey.p8').write_text(os.environ['APPLE_NOTARY_KEY_P8'])
PY
keychain_password=$(openssl rand -hex 32)
security create-keychain -p "$keychain_password" "$keychain" >/dev/null
security set-keychain-settings -lut 7200 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$work/identity.p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" \
    -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
    -k "$keychain_password" "$keychain" >/dev/null
security list-keychains -d user -s "$keychain"
unset APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD keychain_password

assets="$RUNNER_TEMP/signed-assets"
mkdir -p "$assets"
python3 - <<'PY'
import os, tarfile
from pathlib import Path
root = Path(os.environ['RUNNER_TEMP'])
expected = {'prod-code-aarch64-apple-darwin', 'prod-code-server-aarch64-apple-darwin'}
with tarfile.open(root / 'incoming/native-assets.tar') as archive:
    files = [m for m in archive.getmembers() if m.isfile()]
    assert {m.name.removeprefix('./') for m in files} == expected and len(files) == 2
    for member in files:
        name = member.name.removeprefix('./')
        assert member.size <= 256 * 1024 * 1024
        dest = root / 'signed-assets' / name
        with archive.extractfile(member) as source, dest.open('wb') as target:
            import shutil
            shutil.copyfileobj(source, target)
        dest.chmod(0o755)
PY
for binary in prod-code prod-code-server; do
    identifier=prod-code
    if test "$binary" = prod-code-server; then identifier=com.prod-code.gateway; fi
    codesign --force --sign "$APPLE_SIGNING_IDENTITY" --keychain "$keychain" \
        --identifier "$identifier" --options runtime --timestamp \
        "$assets/$binary-aarch64-apple-darwin"
    codesign --verify --deep --strict "$assets/$binary-aarch64-apple-darwin"
done
ditto -c -k --keepParent "$assets" "$work/submission.zip"
xcrun notarytool submit "$work/submission.zip" --key "$work/AuthKey.p8" \
    --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER_ID" \
    --wait --timeout 20m --output-format json > "$work/notary.json"
python3 - <<'PY'
import hashlib, json, os
from pathlib import Path
work = Path(os.environ['SIGNING_WORK'])
result = json.loads((work / 'notary.json').read_text())
assert result.get('status') == 'Accepted', 'Apple notarization was not accepted'
root = Path(os.environ['RUNNER_TEMP']) / 'signed-assets'
receipt = {'status': result['status'], 'id': result['id'],
           'sha256': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir()}}
(root / 'notarization.json').write_text(json.dumps(receipt, indent=2) + '\n')
print('Apple notarization: Accepted; both strict signatures verified')
PY
submission_id=$(python3 -c 'import json,os;print(json.load(open(os.environ["SIGNING_WORK"]+"/notary.json"))["id"])')
xcrun notarytool log "$submission_id" --key "$work/AuthKey.p8" \
    --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER_ID" "$work/notary-log.json"
python3 - <<'PY'
import json, os
from pathlib import Path
r = json.loads((Path(os.environ['SIGNING_WORK']) / 'notary-log.json').read_text())
assert r.get('status') == 'Accepted', 'Notarization log is not Accepted'
print('Notarization log issues:', len(r.get('issues') or []))
PY
tar -cf "$RUNNER_TEMP/signed-assets.tar" -C "$assets" .
