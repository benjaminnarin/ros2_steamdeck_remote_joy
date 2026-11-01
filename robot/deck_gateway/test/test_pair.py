"""Tests for scripts/pair: certificates, bundle, allow list, revocation."""

import hashlib
import importlib.machinery
import importlib.util
import ipaddress
import stat
import tarfile
from pathlib import Path

import pytest
from cryptography import x509
from cryptography.hazmat.primitives import serialization
from cryptography.x509.oid import ExtendedKeyUsageOID

HERE = Path(__file__).resolve().parent
PKG = HERE.parent
_loader = importlib.machinery.SourceFileLoader('pair', str(PKG / 'scripts' / 'pair'))
_spec = importlib.util.spec_from_loader('pair', _loader)
pair = importlib.util.module_from_spec(_spec)
_loader.exec_module(pair)

TEMPLATE = PKG / 'config' / 'router.json5'


def run(tmp, *args):
    return pair.main(['--dir', str(tmp / 'state'), '--template', str(TEMPLATE), '--no-restart',
                      *args])


def bundle(tmp, deck):
    out = tmp / f'{deck}.bundle.tar'
    with tarfile.open(out) as t:
        return {m.name: (t.extractfile(m).read(), m.mode) for m in t.getmembers()}


def load_cert(pem):
    return x509.load_pem_x509_certificate(pem)


def mode(p: Path):
    return stat.S_IMODE(p.stat().st_mode)


def pair_deck(tmp, deck, *extra):
    return run(tmp, f'--deck={deck}', '--out', str(tmp / f'{deck}.bundle.tar'), *extra)


def test_first_pairing_creates_ca_server_and_bundle(tmp_path):
    assert pair_deck(tmp_path, 'deck1', '--host', '192.168.1.50', '--robot-name', 'mybot') == 0
    tls = tmp_path / 'state' / 'tls'
    ca = load_cert((tls / 'ca.pem').read_bytes())
    server = load_cert((tls / 'robot.pem').read_bytes())
    assert ca.extensions.get_extension_for_class(x509.BasicConstraints).value.ca
    server.verify_directly_issued_by(ca)
    san = server.extensions.get_extension_for_class(x509.SubjectAlternativeName).value
    assert ipaddress.ip_address('192.168.1.50') in san.get_values_for_type(x509.IPAddress)
    assert ipaddress.ip_address('127.0.0.1') in san.get_values_for_type(x509.IPAddress)
    eku = server.extensions.get_extension_for_class(x509.ExtendedKeyUsage).value
    assert ExtendedKeyUsageOID.SERVER_AUTH in eku

    for key in ('ca.key', 'robot.key'):
        assert mode(tls / key) == 0o600, key
    assert mode(tmp_path / 'deck1.bundle.tar') == 0o600, 'the bundle holds a private key'

    files = bundle(tmp_path, 'deck1')
    assert set(files) == {'ca.pem', 'client.pem', 'client.key', 'robot.toml'}
    assert files['client.key'][1] == 0o600
    client = load_cert(files['client.pem'][0])
    client.verify_directly_issued_by(ca)
    cn = client.subject.get_attributes_for_oid(x509.NameOID.COMMON_NAME)[0].value
    assert pair.re.fullmatch(r'deck1-[0-9a-f]{6}', cn)
    eku = client.extensions.get_extension_for_class(x509.ExtendedKeyUsage).value
    assert ExtendedKeyUsageOID.CLIENT_AUTH in eku
    key = serialization.load_pem_private_key(files['client.key'][0], None)
    assert key.public_key().public_numbers() == client.public_key().public_numbers()

    toml = files['robot.toml'][0].decode()
    rid = hashlib.sha256(ca.public_bytes(serialization.Encoding.DER)).hexdigest()
    assert f'id = "{rid}"' in toml
    assert '"quic/192.168.1.50:7450?mixed_rel=1"' in toml
    assert 'name = "mybot"' in toml
    assert f'cn = "{cn}"' in toml


def test_router_config_lists_active_cns_and_revoke_removes_them(tmp_path):
    pair_deck(tmp_path, 'deck1', '--host', '10.0.0.5')
    pair_deck(tmp_path, 'deck2')
    pair_deck(tmp_path, 'deck1')                     # re-pair: a new CN, the old one stays active
    state = pair.json.loads((tmp_path / 'state' / 'pairing.json').read_text())
    cns = [d['cn'] for d in state['decks']]
    assert len(cns) == 3 and len(set(cns)) == 3, 'CNs are never reused'
    router = (tmp_path / 'state' / 'router.json5').read_text()
    for cn in cns:
        assert f'"{cn}"' in router
    assert 'deck1-spike' not in router, 'the template placeholder is replaced'

    assert run(tmp_path, '--revoke', 'deck1') == 0
    router = (tmp_path / 'state' / 'router.json5').read_text()
    deck1 = [d['cn'] for d in state['decks'] if d['name'] == 'deck1']
    deck2 = [d['cn'] for d in state['decks'] if d['name'] == 'deck2']
    for cn in deck1:
        assert f'"{cn}"' not in router
    assert f'"{deck2[0]}"' in router
    assert run(tmp_path, '--revoke', 'deck1') == 1, 'nothing left to revoke'

    run(tmp_path, '--revoke', 'deck2')
    router = (tmp_path / 'state' / 'router.json5').read_text()
    assert f'"{pair.NO_DECKS_CN}"' in router, 'Zenoh rejects an empty allow list'


def test_first_pairing_needs_a_host(tmp_path, capsys):
    assert pair_deck(tmp_path, 'deck1') == 1
    assert '--host' in capsys.readouterr().err


def test_new_host_needs_reissue_and_keeps_the_ca(tmp_path, capsys):
    pair_deck(tmp_path, 'deck1', '--host', '10.0.0.5')
    ca_before = (tmp_path / 'state' / 'tls' / 'ca.pem').read_bytes()
    assert pair_deck(tmp_path, 'deck2', '--host', '10.0.0.6') == 1
    assert '--reissue-server' in capsys.readouterr().err
    assert pair_deck(tmp_path, 'deck2', '--host', '10.0.0.6', '--reissue-server') == 0
    assert (tmp_path / 'state' / 'tls' / 'ca.pem').read_bytes() == ca_before
    server = load_cert((tmp_path / 'state' / 'tls' / 'robot.pem').read_bytes())
    ips = server.extensions.get_extension_for_class(x509.SubjectAlternativeName).value \
        .get_values_for_type(x509.IPAddress)
    assert {ipaddress.ip_address('10.0.0.5'), ipaddress.ip_address('10.0.0.6')} <= set(ips)


@pytest.mark.parametrize('bad', ['Deck1', 'deck_1', '-deck', 'a' * 33, ''])
def test_rejects_bad_deck_names(tmp_path, bad):
    assert pair_deck(tmp_path, bad, '--host', '10.0.0.5') == 1


def test_robot_name_is_fixed_after_first_run(tmp_path):
    pair_deck(tmp_path, 'deck1', '--host', '10.0.0.5', '--robot-name', 'mybot')
    assert pair_deck(tmp_path, 'deck2', '--robot-name', 'other') == 1
    assert pair_deck(tmp_path, 'deck2', '--robot-name', 'mybot') == 0


def test_render_router_requires_the_decks_subject():
    with pytest.raises(pair.PairError):
        pair.render_router('{ access_control: { subjects: [] } }', ['x'])
    out = pair.render_router(TEMPLATE.read_text(), ['deck1-abcdef'])
    assert 'cert_common_names: ["deck1-abcdef"]' in out
    assert out.count('cert_common_names') == TEMPLATE.read_text().count('cert_common_names')


def test_list(tmp_path, capsys):
    run(tmp_path, '--list')
    assert 'not set up yet' in capsys.readouterr().out
    pair_deck(tmp_path, 'deck1', '--host', '10.0.0.5', '--robot-name', 'mybot')
    run(tmp_path, '--revoke', 'deck1')
    capsys.readouterr()
    run(tmp_path, '--list')
    out = capsys.readouterr().out
    assert 'robot mybot' in out and 'deck1' in out and 'revoked' in out
