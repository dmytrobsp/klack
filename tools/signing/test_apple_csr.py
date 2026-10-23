"""Tests for tools/signing/apple-csr.sh: a temporary HOME, a fake gh, and a throwaway CA in place of Apple's."""
import base64
import hashlib
import os
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "apple-csr.sh")
REPO = "dmytrobsp/ttyak"
TEAM = "TEAMID1234"

# Records each call's arguments and the length and SHA-256 of stdin, never the value. Fails like gh does
# when FAKE_GH_FAIL is set, and when APPLE_P12 does not get non-empty base64.
FAKE_GH = r'''#!/usr/bin/env python3
import base64, binascii, hashlib, os, sys
data = sys.stdin.buffer.read()
with open(os.environ["FAKE_GH_LOG"], "a") as log:
    log.write("%s\t%d\t%s\n" % (" ".join(sys.argv[1:]), len(data), hashlib.sha256(data).hexdigest()))
if os.environ.get("FAKE_GH_FAIL"):
    sys.stderr.write("HTTP 404: Not Found\n")
    sys.exit(1)
if sys.argv[1:4] == ["secret", "set", "APPLE_P12"]:
    try:
        ok = len(base64.b64decode(data, validate=True)) > 0
    except binascii.Error:
        ok = False
    if not ok:
        sys.stderr.write("fake gh: APPLE_P12 is not base64\n")
        sys.exit(1)
'''


def openssl(*args, stdin=None):
    r = subprocess.run(["openssl", *args], input=stdin, capture_output=True, check=True)
    return r.stdout


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


class AppleCsr(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="apple-csr-test-")
        self.home = os.path.join(self.tmp, "home")
        self.work = os.path.join(self.tmp, "work")
        bin_dir = os.path.join(self.tmp, "bin")
        for d in (self.home, self.work, bin_dir):
            os.mkdir(d)
        gh = os.path.join(bin_dir, "gh")
        with open(gh, "w") as f:
            f.write(FAKE_GH)
        os.chmod(gh, 0o755)
        self.gh_log = os.path.join(self.tmp, "gh.log")
        self.folder = os.path.join(self.home, ".config", "ttyak-signing")
        self.key = os.path.join(self.folder, "developer-id.key")
        self.csr = os.path.join(self.folder, "developer-id.certSigningRequest")
        self.env = {k: v for k, v in os.environ.items() if k not in ("GH_TOKEN", "GITHUB_TOKEN", "GH_HOST")}
        self.env.update(HOME=self.home, PATH=bin_dir + os.pathsep + os.environ["PATH"], FAKE_GH_LOG=self.gh_log)
        self.ca_key, self.ca = self.make_ca("Test Developer ID CA")
        self.env["TTYAK_SIGNING_CA"] = self.ca
        self.outputs = []

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def run_script(self, *args, env=None):
        r = subprocess.run(["bash", SCRIPT, *args], env=env or self.env, capture_output=True, text=True)
        self.outputs.append(r.stdout + r.stderr)
        return r

    def path(self, name):
        return os.path.join(self.work, name)

    def make_ca(self, cn):
        key, cert = self.path(cn + ".key"), self.path(cn + ".pem")
        openssl("genpkey", "-quiet", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", key)
        openssl("req", "-x509", "-new", "-key", key, "-subj", "/CN=%s/O=Test" % cn, "-days", "30", "-out", cert,
                "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign")
        return key, cert

    def issue(self, csr, name, kind="Developer ID Application", der=True, ca=None, eku="codeSigning", serial=7):
        """Issues a certificate for the CSR's key with an Apple-like subject; returns its path."""
        ca_key, ca_cert = ca or (self.ca_key, self.ca)
        ext = self.path(name + ".ext")
        with open(ext, "w") as f:
            f.write("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\n")
            f.write("extendedKeyUsage=critical,%s\n" % eku)
        out = self.path(name)
        subject = "/UID=%s/CN=%s: Test, Person (%s)/OU=%s/O=Test, Person/C=US" % (TEAM, kind, TEAM, TEAM)
        openssl("x509", "-req", "-in", csr, "-CA", ca_cert, "-CAkey", ca_key, "-set_serial", str(serial), "-days", "365",
                "-subj", subject, "-extfile", ext, "-outform", "DER" if der else "PEM", "-out", out)
        return out

    def other_csr(self):
        key, csr = self.path("other.key"), self.path("other.csr")
        openssl("genpkey", "-quiet", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", key)
        openssl("req", "-new", "-key", key, "-subj", "/CN=other", "-out", csr)
        return csr

    def gh_calls(self):
        if not os.path.exists(self.gh_log):
            return []
        with open(self.gh_log) as f:
            return [line.rstrip("\n").split("\t") for line in f]

    def secrets(self):
        """The secret byte strings no output may contain: the key's PEM body lines and the p12 password."""
        found = []
        with open(self.key) as f:
            found += [line.strip() for line in f if len(line.strip()) >= 16 and "PRIVATE KEY" not in line]
        pw = os.path.join(self.folder, "developer-id.p12-password")
        if os.path.exists(pw):
            with open(pw) as f:
                found.append(f.read().strip())
        return found

    def assert_nothing_secret_printed(self):
        secrets = self.secrets()
        self.assertTrue(secrets)
        for out in self.outputs:
            for s in secrets:
                self.assertNotIn(s, out)

    def mode(self, path):
        return os.stat(path).st_mode & 0o777

    def request_and_install(self, der=True):
        self.assertEqual(self.run_script("request").returncode, 0)
        cer = self.issue(self.csr, "apple.cer", der=der)
        r = self.run_script("install", cer)
        self.assertEqual(r.returncode, 0, r.stderr)
        return r

    # request

    def test_request_makes_private_folder_key_and_csr(self):
        r = self.run_script("request")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.mode(self.folder), 0o700)
        self.assertEqual(self.mode(self.key), 0o600)
        self.assertEqual(self.mode(self.csr), 0o600)
        self.assertIn(self.csr, r.stdout)
        self.assertIn("Developer ID Application", r.stdout)
        self.assertIn("G2 Sub-CA (Xcode 11.4.1 or later)", r.stdout)
        text = openssl("req", "-in", self.csr, "-noout", "-text").decode()
        self.assertIn("Public-Key: (2048 bit)", text)
        self.assertIn("rsaEncryption", text)
        self.assertIn("sha256WithRSAEncryption", text)
        self.assertIn("CN = ttyak Developer ID", text)
        openssl("req", "-in", self.csr, "-noout", "-verify")
        csr_pub = openssl("req", "-in", self.csr, "-noout", "-pubkey")
        key_pub = openssl("pkey", "-in", self.key, "-pubout")
        self.assertEqual(csr_pub, key_pub)
        self.assert_nothing_secret_printed()

    def test_request_refuses_to_overwrite_a_key(self):
        self.assertEqual(self.run_script("request").returncode, 0)
        before = (sha256(self.key), sha256(self.csr))
        r = self.run_script("request")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("--renew", r.stderr)
        self.assertEqual((sha256(self.key), sha256(self.csr)), before)
        self.assertEqual(sorted(os.listdir(self.folder)), ["developer-id.certSigningRequest", "developer-id.key"])
        self.assert_nothing_secret_printed()

    def test_renew_keeps_the_old_files_with_a_date(self):
        self.request_and_install()
        old = {n: sha256(os.path.join(self.folder, n)) for n in os.listdir(self.folder)}
        self.assertEqual(len(old), 5)
        r = self.run_script("request", "--renew")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotEqual(sha256(self.key), old["developer-id.key"])
        names = sorted(os.listdir(self.folder))
        self.assertIn("developer-id.key", names)
        self.assertIn("developer-id.certSigningRequest", names)
        for name, digest in old.items():
            kept = [n for n in names if n.startswith(name + ".") and n != name and n[len(name) + 1:][:1].isdigit()]
            self.assertEqual(len(kept), 1, (name, names))
            self.assertEqual(sha256(os.path.join(self.folder, kept[0])), digest)
            self.assertEqual(self.mode(os.path.join(self.folder, kept[0])), 0o600)
        self.assertNotIn("developer-id.cer", names)
        # A second renewal on the same day keeps both older sets.
        self.assertEqual(self.run_script("request", "--renew").returncode, 0)
        self.assertEqual(len([n for n in os.listdir(self.folder) if n.startswith("developer-id.key.")]), 2)
        self.assert_nothing_secret_printed()

    # install

    def test_install_sets_both_secrets_and_prints_only_public_facts(self):
        r = self.request_and_install()
        p12 = os.path.join(self.folder, "developer-id.p12")
        pw = os.path.join(self.folder, "developer-id.p12-password")
        cer = os.path.join(self.folder, "developer-id.cer")
        for p in (p12, pw, cer):
            self.assertEqual(self.mode(p), 0o600, p)
        self.assertEqual(self.mode(self.folder), 0o700)
        calls = self.gh_calls()
        self.assertEqual([c[0] for c in sorted(calls)], [
            "secret set APPLE_P12 -R " + REPO,
            "secret set APPLE_P12_PASSWORD -R " + REPO,
        ])
        by_name = {c[0].split()[2]: c for c in calls}
        with open(p12, "rb") as f:
            b64 = base64.b64encode(f.read())
        self.assertEqual(by_name["APPLE_P12"][1:], [str(len(b64)), hashlib.sha256(b64).hexdigest()])
        with open(pw, "rb") as f:
            password = f.read()
        self.assertGreaterEqual(len(password), 32)
        self.assertEqual(by_name["APPLE_P12_PASSWORD"][1:], [str(len(password)), hashlib.sha256(password).hexdigest()])
        lines = r.stdout.splitlines()
        self.assertTrue(any(l.startswith("APPLE_P12 and APPLE_P12_PASSWORD are set (expires ") for l in lines), lines)
        self.assertIn("Developer ID Application: Test, Person (%s)" % TEAM, r.stdout)
        self.assert_nothing_secret_printed()

    def test_p12_opens_with_its_password_in_the_algorithms_macos_imports(self):
        self.request_and_install()
        p12 = os.path.join(self.folder, "developer-id.p12")
        pw = os.path.join(self.folder, "developer-id.p12-password")
        info = subprocess.run(["openssl", "pkcs12", "-in", p12, "-info", "-noout", "-passin", "file:" + pw],
                              capture_output=True, text=True)
        self.assertEqual(info.returncode, 0, info.stderr)
        self.assertIn("MAC: sha1", info.stderr)
        self.assertIn("Shrouded Keybag: pbeWithSHA1And3-KeyTripleDES-CBC", info.stderr)
        self.assertIn("PKCS7 Encrypted data: pbeWithSHA1And3-KeyTripleDES-CBC", info.stderr)
        legacy = subprocess.run(["openssl", "pkcs12", "-legacy", "-in", p12, "-info", "-noout", "-passin", "file:" + pw],
                                capture_output=True)
        self.assertEqual(legacy.returncode, 0)
        certs = openssl("pkcs12", "-in", p12, "-nokeys", "-passin", "file:" + pw).decode()
        self.assertEqual(certs.count("BEGIN CERTIFICATE"), 2)  # the certificate and the intermediate
        self.assertIn("Developer ID Application: Test, Person", certs)
        self.assertIn("Test Developer ID CA", certs)
        wrong = self.path("wrong")
        with open(wrong, "w") as f:
            f.write("not-the-password")
        bad = subprocess.run(["openssl", "pkcs12", "-in", p12, "-info", "-noout", "-passin", "file:" + wrong],
                             capture_output=True)
        self.assertNotEqual(bad.returncode, 0)

    def test_install_accepts_pem(self):
        self.request_and_install(der=False)
        self.assertEqual(len(self.gh_calls()), 2)

    def test_install_refuses_a_certificate_for_another_key(self):
        self.assertEqual(self.run_script("request").returncode, 0)
        r = self.run_script("install", self.issue(self.other_csr(), "other.cer"))
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("does not match", r.stderr)
        self.assertEqual(self.gh_calls(), [])
        self.assertEqual(sorted(os.listdir(self.folder)), ["developer-id.certSigningRequest", "developer-id.key"])
        self.assert_nothing_secret_printed()

    def test_install_refuses_other_certificate_types(self):
        self.assertEqual(self.run_script("request").returncode, 0)
        for kind in ("Apple Development", "Developer ID Installer"):
            r = self.run_script("install", self.issue(self.csr, kind + ".cer", kind=kind))
            self.assertNotEqual(r.returncode, 0, kind)
            self.assertIn("not a Developer ID Application certificate", r.stderr)
        r = self.run_script("install", self.issue(self.csr, "no-eku.cer", eku="emailProtection"))
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("Code Signing", r.stderr)
        self.assertEqual(self.gh_calls(), [])
        self.assert_nothing_secret_printed()

    def test_install_refuses_a_certificate_from_another_authority(self):
        self.assertEqual(self.run_script("request").returncode, 0)
        r = self.run_script("install", self.issue(self.csr, "g1.cer", ca=self.make_ca("Old CA")))
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("G2", r.stderr)
        self.assertEqual(self.gh_calls(), [])

    def test_install_refuses_a_file_that_is_no_certificate(self):
        self.assertEqual(self.run_script("request").returncode, 0)
        junk = self.path("junk.cer")
        with open(junk, "w") as f:
            f.write("hello\n")
        r = self.run_script("install", junk)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("not a certificate", r.stderr)
        r = self.run_script("install", self.path("missing.cer"))
        self.assertNotEqual(r.returncode, 0)

    def test_install_before_request_says_to_run_request(self):
        r = self.run_script("install", self.issue(self.other_csr(), "other.cer"))
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("request", r.stderr)

    def test_install_reports_a_gh_failure(self):
        self.assertEqual(self.run_script("request").returncode, 0)
        env = dict(self.env, FAKE_GH_FAIL="1")
        r = self.run_script("install", self.issue(self.csr, "apple.cer"), env=env)
        self.assertNotEqual(r.returncode, 0)
        self.assertNotIn("are set", r.stdout)
        self.assertIn("gh secret set", r.stderr)
        self.assert_nothing_secret_printed()

    def test_install_keeps_a_different_older_certificate(self):
        self.request_and_install()
        first = sha256(os.path.join(self.folder, "developer-id.cer"))
        cer = self.issue(self.csr, "again.cer", serial=8)  # the same key, a new certificate
        self.assertEqual(self.run_script("install", cer).returncode, 0)
        kept = [n for n in os.listdir(self.folder) if n.startswith("developer-id.cer.")]
        self.assertEqual(len(kept), 1)
        self.assertEqual(sha256(os.path.join(self.folder, kept[0])), first)
        self.assert_nothing_secret_printed()

    # status

    def test_status_shows_names_subject_and_days_left(self):
        r = self.run_script("status")
        self.assertEqual(r.returncode, 0)
        self.assertIn("request", r.stdout)
        self.request_and_install()
        r = self.run_script("status")
        self.assertEqual(r.returncode, 0, r.stderr)
        for name in ("developer-id.key", "developer-id.certSigningRequest", "developer-id.cer",
                     "developer-id.p12", "developer-id.p12-password"):
            self.assertIn(name, r.stdout)
        self.assertIn("Developer ID Application: Test, Person (%s)" % TEAM, r.stdout)
        self.assertRegex(r.stdout, r"expires \d{4}-\d\d-\d\d \((364|365) days left\)")
        self.assertIn("matches the key", r.stdout)
        self.assert_nothing_secret_printed()

    def test_usage(self):
        for args in ((), ("bogus",), ("install",), ("request", "--force")):
            r = self.run_script(*args)
            self.assertEqual(r.returncode, 2, args)
            self.assertIn("usage", r.stderr)


if __name__ == "__main__":
    unittest.main()
