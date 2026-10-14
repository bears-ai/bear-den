"""Opt-in production-image acceptance against an explicitly isolated database.

Set CABINET_TEST_IMAGE, CABINET_TEST_NETWORK and CABINET_TEST_DATABASE_URL.
The test uses a disposable container and trusted loopback TLS certificate, not
production ingress, and never restarts the running Den or attaches memory volumes.
"""

import html
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import shutil
import ssl
import subprocess
import tempfile
import threading
import time
import unittest
from urllib.parse import urlparse
import uuid

import requests

from test_cabinet_compose import ROOT, compose


class CabinetProxy(BaseHTTPRequestHandler):
    def forward(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        headers = {key: value for key, value in self.headers.items()
                   if key.lower() not in ("host", "connection", "content-length")}
        response = requests.request(
            self.command, self.server.upstream + self.path, headers=headers,
            data=body, timeout=10, allow_redirects=False,
        )
        self.send_response(response.status_code)
        for key, value in response.headers.items():
            if key.lower() not in (
                "transfer-encoding", "content-encoding", "content-length", "connection",
            ):
                self.send_header(key, value)
        self.send_header("Content-Length", str(len(response.content)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(response.content)

    do_GET = forward
    do_POST = forward
    do_HEAD = forward

    def log_message(self, *_args):
        pass


def docker(*args, timeout=30):
    return subprocess.run(
        ["docker", *args], check=True, capture_output=True, text=True, timeout=timeout,
    ).stdout.strip()


@unittest.skipUnless(
    os.environ.get("CABINET_TEST_IMAGE") and shutil.which("docker") and shutil.which("openssl"),
    "opt-in image, Docker and OpenSSL are required",
)
class CabinetContainerTests(unittest.TestCase):
    url_suffix = ""

    @classmethod
    def setUpClass(cls):
        cls.image = os.environ["CABINET_TEST_IMAGE"]
        cls.network = os.environ["CABINET_TEST_NETWORK"]
        database_url = os.environ["CABINET_TEST_DATABASE_URL"]
        database = urlparse(database_url).path.removeprefix("/")
        if not re.fullmatch(r"den_cabinet_[a-z0-9_]+_validation(?:_[0-9]+)?", database):
            raise ValueError("Acceptance requires a named isolated Cabinet validation database")
        cls.name = "cabinet-image-test-" + uuid.uuid4().hex[:12]
        temporary = tempfile.TemporaryDirectory(prefix="cabinet_image_", dir=ROOT / ".pytest_cache")
        cls.addClassCleanup(temporary.cleanup)
        certificate = Path(temporary.name) / "certificate.pem"
        key = Path(temporary.name) / "key.pem"
        subprocess.run(
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
             "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1",
             "-keyout", str(key), "-out", str(certificate)],
            check=True, capture_output=True, timeout=15,
        )
        cls.certificate = str(certificate)
        cls.proxy = ThreadingHTTPServer(("127.0.0.1", 0), CabinetProxy)
        cls.addClassCleanup(cls.proxy.server_close)
        cls.origin = f"https://127.0.0.1:{cls.proxy.server_port}"
        cabinet = compose(
            DEN_IMAGE=cls.image, DATABASE_URL=database_url, CABINET_URL=cls.origin + cls.url_suffix,
            S3_ENDPOINT="", S3_BUCKET="", S3_ACCESS_KEY_ID="", S3_SECRET_ACCESS_KEY="",
        )["bears-cabinet"]
        environment_args = []
        for name, value in cabinet["environment"].items():
            environment_args += ["--env", f"{name}={value}"]
        # The same release image's migration job owns SQLx and browser-session DDL.
        docker("run", "--rm", "--network", cls.network,
               *environment_args, cls.image, "migrate", timeout=120)
        cls.addClassCleanup(
            subprocess.run, ["docker", "rm", "--force", cls.name],
            capture_output=True, timeout=30, check=False,
        )
        docker("run", "--detach", "--name", cls.name, "--network", cls.network,
               "--read-only", "--tmpfs", cabinet["tmpfs"][0],
               *environment_args, cls.image, *cabinet["command"])
        cls.inspection = json.loads(docker("inspect", cls.name))[0]
        ip = cls.inspection["NetworkSettings"]["Networks"][cls.network]["IPAddress"]
        cls.upstream = f"http://{ip}:{cabinet['environment']['PORT']}"
        for _ in range(30):
            try:
                response = requests.get(cls.upstream + "/health/ready", timeout=2)
                if response.status_code == 200:
                    break
            except requests.RequestException:
                pass
            time.sleep(1)
        else:
            raise AssertionError("Cabinet failed readiness: " + docker("logs", cls.name))
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, key)
        cls.proxy.socket = context.wrap_socket(cls.proxy.socket, server_side=True)
        cls.proxy.upstream = cls.upstream
        thread = threading.Thread(target=cls.proxy.serve_forever, daemon=True)
        thread.start()
        cls.addClassCleanup(cls.proxy.shutdown)

    def setUp(self):
        self.browser = requests.Session()
        self.browser.verify = self.certificate
        self.addCleanup(self.browser.close)

    def request(self, method, path, **kwargs):
        return self.browser.request(
            method, self.origin + path, timeout=10, allow_redirects=False, **kwargs,
        )

    def login(self):
        response = self.request(
            "POST", "/login/password", headers={"Origin": self.origin},
            data={"username": "admin", "password": "Never deploy with default passwords.",
                  "next": "/cabinet"},
        )
        self.assertEqual(response.status_code, 303, response.text)
        cookie = response.headers["Set-Cookie"]
        self.assertIn("cabinet_session=", cookie)
        self.assertIn("Secure", cookie)
        self.assertIn("HttpOnly", cookie)
        self.assertNotIn("Domain=", cookie)
        self.assertEqual(response.headers["Location"], "/cabinet")

    def test_production_health_assets_and_native_runtime_isolation(self):
        self.assertEqual(self.request("GET", "/health/ready").status_code, 200)
        self.assertEqual(self.request("GET", "/login").status_code, 200)
        self.assertEqual(self.request("GET", "/assets/css/style.css").status_code, 200)
        for path in ("/admin", "/bear/test-bear", "/v1/chat", "/work", "/bearwire", "/connections"):
            self.assertEqual(self.request("GET", path).status_code, 404, path)
        self.assertTrue(self.inspection["HostConfig"]["ReadonlyRootfs"])
        self.assertFalse(self.inspection["Mounts"])
        docker("exec", self.name, "sh", "-c", "test ! -e /var/lib/den/bear-sqlite")
        status = docker("exec", self.name, "cat", "/proc/1/status")
        self.assertRegex(status, r"Uid:\s+10001\s+10001")

    def test_https_cookie_page_edit_discussion_and_history(self):
        self.login()
        response = self.request("GET", "/cabinet")
        self.assertEqual(response.status_code, 200, response.text)
        self.assertIn("Cabinet navigation", response.text)
        self.assertNotIn("Den management", response.text)
        response = self.request(
            "POST", "/cabinet/new", headers={"Origin": self.origin},
            data={"title": "Image acceptance " + uuid.uuid4().hex[:8],
                  "content": "Original image publication"},
        )
        self.assertEqual(response.status_code, 303, response.text)
        page = response.headers["Location"]
        self.assertRegex(page, r"^/cabinet/cabinet_item_[0-9a-f]{32}$")
        response = self.request("GET", page)
        self.assertIn("Original image publication", response.text)
        self.assertIn("File uploads are unavailable", response.text)
        response = self.request("GET", page + "/edit")
        self.assertEqual(response.status_code, 200)
        base = re.search(r'name="base_version" value="([^"]+)"', response.text).group(1)
        self.assertEqual(self.request(
            "POST", page + "/discussion", data={"content": "Missing origin must be refused"},
        ).status_code, 403)
        response = self.request(
            "POST", page + "/discussion", headers={"Origin": self.origin},
            data={"content": "Production image discussion"},
        )
        self.assertEqual(response.status_code, 303, response.text)
        response = self.request("GET", page + "/discussion")
        self.assertIn("Production image discussion", response.text)
        response = self.request(
            "POST", page + "/edit", headers={"Origin": self.origin},
            data={"title": "Updated image acceptance", "content": "Updated image publication",
                  "base_version": html.unescape(base)},
        )
        self.assertEqual(response.status_code, 303, response.text)
        response = self.request("GET", page + "/history")
        self.assertEqual(response.status_code, 200, response.text)
        self.assertIn("Revision 1", response.text)
        self.assertIn("Revision 2", response.text)
        response = self.request("GET", page + "/history", params={"before": html.unescape(base)})
        self.assertEqual(response.status_code, 200, response.text)
        self.assertIn("Original image publication", response.text)
        self.assertIn("Updated image publication", response.text)
        response = self.request("GET", page, params={"version": html.unescape(base)})
        self.assertEqual(response.status_code, 200)
        self.assertIn("Original image publication", response.text)
        self.assertNotIn("Updated image publication", response.text)

    def test_mcp_has_no_browser_cookie_or_origin_authority(self):
        self.login()
        response = self.request("POST", "/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "ping"})
        self.assertEqual(response.status_code, 401)
        self.assertIn("Bearer", response.headers["WWW-Authenticate"])
        self.assertEqual(response.headers["Cache-Control"], "no-store")
        response = self.request("POST", "/mcp", headers={"Origin": self.origin}, json={})
        self.assertEqual(response.status_code, 403)


class CabinetContainerTrailingSlashTests(CabinetContainerTests):
    url_suffix = "/"


if __name__ == "__main__":
    unittest.main()
