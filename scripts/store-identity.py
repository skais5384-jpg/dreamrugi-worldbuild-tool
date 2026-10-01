"""Validate operator-confirmed Partner Center identity; no account or network writes."""
import argparse
import json
import re
from pathlib import Path


def package_version(value):
    if not isinstance(value, str) or not re.fullmatch(r'[1-9]\d*\.(0|[1-9]\d*)\.(0|[1-9]\d*)\.0', value):
        raise ValueError('Store package version must be four numeric parts, major > 0, revision 0')
    parts = tuple(map(int, value.split('.')))
    if any(p > 65535 for p in parts):
        raise ValueError('Store package version exceeds MSIX range')
    return parts


def validate(identity, app_version):
    if identity.get('source') != 'PartnerCenter' or identity.get('verified') is not True or identity.get('historyVerified') is not True:
        raise ValueError('Verified Partner Center identity and package-family history are required')
    for field in ['name', 'publisher', 'publisherDisplayName', 'storeId', 'checkedUtc']:
        value = identity.get(field)
        if not isinstance(value, str) or not value.strip() or value != value.strip() or any(c in value for c in '\r\n\0'):
            raise ValueError('Missing or invalid Store identity field: ' + field)
        if any(s in value.lower() for s in ['localtest', 'local test', 'placeholder', '{{', 'example']):
            raise ValueError('Local test/placeholder identity cannot be submitted')
    if not re.fullmatch(r'[A-Za-z0-9.-]{3,50}', identity['name']) or not identity['publisher'].startswith('CN='):
        raise ValueError('Invalid MSIX Name/Publisher')
    if not re.fullmatch(r'[A-Z0-9]{12}', identity['storeId']):
        raise ValueError('Invalid Store product ID')
    if not re.fullmatch(r'[1-9]\d*\.(0|[1-9]\d*)\.(0|[1-9]\d*)', app_version):
        raise ValueError('Store release requires a formal app version')
    version = identity.get('packageVersion', app_version + '.0')
    current = package_version(version)
    latest = identity.get('latestPackageVersion')
    if identity.get('noPublishedPackages') is True:
        if latest is not None or version != app_version + '.0':
            raise ValueError('New Store family starts at the app version; test-family history is excluded')
    elif identity.get('noPublishedPackages') is False and latest is not None:
        # Store may assign the fourth component. Compare the full existing version.
        if not isinstance(latest, str) or not re.fullmatch(r'[1-9]\d*\.(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', latest):
            raise ValueError('Existing Store-family version is invalid')
        prior = tuple(map(int, latest.split('.')))
        if any(p > 65535 for p in prior) or current <= prior:
            raise ValueError('Candidate must exceed the latest version in the same Store family')
    else:
        raise ValueError('Explicit Store-family submission history is required')
    return dict(identity, packageVersion=version)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--identity', required=True, type=Path)
    parser.add_argument('--version', required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(validate(json.loads(args.identity.read_text(encoding='utf-8-sig')), args.version)))
    except (ValueError, OSError) as error:
        raise SystemExit('Store identity rejected: ' + str(error))
