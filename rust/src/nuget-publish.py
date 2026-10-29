"""Publish one validated artifact with job-local OIDC, then a provided fallback.

Never use PackageVerify as a push gate. Never create or persist an API key.
HTTP uncertainty and non-authentication push failures stop the retry sequence.
"""
import glob
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import tempfile
from urllib.error import HTTPError
from urllib.request import urlopen
import xml.etree.ElementTree as ET
import zipfile


def normalized_version(version):
    """NuGet's flat-container URLs omit build metadata and a zero revision."""
    public = version.split("+", 1)[0].lower()
    release, separator, prerelease = public.partition("-")
    if not re.fullmatch(r"[0-9]+(?:\.[0-9]+){0,3}", release):
        raise ValueError("invalid NuGet release version")
    parts = [str(int(part)) for part in release.split(".")]
    while len(parts) < 3:
        parts.append("0")
    if len(parts) == 4 and parts[3] == "0":
        parts.pop()
    return ".".join(parts) + (separator + prerelease if separator else "")


def exact_version_exists(package_id, version):
    version = normalized_version(version)
    url = ("https://api.nuget.org/v3-flatcontainer/"
           f"{package_id.lower()}/{version.lower()}/"
           f"{package_id.lower()}.{version.lower()}.nupkg")
    try:
        with urlopen(url, timeout=30) as response:
            data = response.read()
        with tempfile.TemporaryDirectory() as directory:
            file = Path(directory) / "public.nupkg"
            file.write_bytes(data)
            return True if identity(file) == (package_id.lower(), version) else None
    except HTTPError as error:
        return False if error.code == 404 else None
    except Exception:
        return None


def identity(artifact):
    with zipfile.ZipFile(artifact) as archive:
        specs = [name for name in archive.namelist() if name.endswith(".nuspec")]
        if len(specs) != 1:
            raise ValueError("artifact must contain exactly one nuspec")
        root = ET.fromstring(archive.read(specs[0]))
        fields = {node.tag.rsplit("}", 1)[-1]: node.text for node in root.iter()}
    package_id, version = fields.get("id", ""), fields.get("version", "")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", package_id) or not re.fullmatch(r"[A-Za-z0-9.+-]+", version):
        raise ValueError("invalid artifact identity")
    return package_id.lower(), normalized_version(version)


def publish(artifact, oidc, fallback, push, probe=exact_version_exists):
    package_id, version = identity(artifact)
    fingerprint = hashlib.sha256(artifact.read_bytes()).digest()
    attempts = [{"method": "trusted-publishing", "status": "available" if oidc else "unavailable"}]
    existing = probe(package_id, version)
    if existing is not False:
        return {"selected_method": "none", "publication": "already-present" if existing else "unknown", "trust_repaired": False, "attempts": attempts}
    for method, key in [("trusted-publishing", oidc), ("token-fallback", fallback)]:
        if not key:
            continue
        if hashlib.sha256(artifact.read_bytes()).digest() != fingerprint:
            raise ValueError("validated artifact changed before retry")
        status = push(artifact, key)
        attempts.append({"method": method, "status": status})
        if status == "accepted":
            return {"selected_method": method, "publication": "accepted", "trust_repaired": method == "trusted-publishing", "attempts": attempts}
        if status not in ("authentication-failed", "authorization-failed"):
            return {"selected_method": method, "publication": "unverified", "trust_repaired": False, "attempts": attempts}
        exists = probe(package_id, version)
        if exists is not False:
            return {"selected_method": method, "publication": "already-present" if exists else "unknown", "trust_repaired": False, "attempts": attempts}
    return {"selected_method": "none", "publication": "unverified", "trust_repaired": False, "attempts": attempts}


def push_artifact(artifact, key):
    result = subprocess.run(["dotnet", "nuget", "push", str(artifact), "--source", "https://api.nuget.org/v3/index.json", "--api-key", key], capture_output=True, text=True, check=False)
    # Captured output can contain credentials. Report only a fixed classification.
    if result.returncode == 0:
        return "accepted"
    output = result.stdout + result.stderr
    if re.search(r"\b403\b.*Forbidden|Forbidden.*\b403\b", output, re.I):
        return "authorization-failed"
    if re.search(r"\b401\b.*Unauthorized|Unauthorized.*\b401\b|API key.*(?:invalid|expired)", output, re.I):
        return "authentication-failed"
    return "push-failed"


def main():
    expected = os.environ["PRM_PACKAGE_ID"].lower()
    if expected in ("mypackage", "packagename", "example"):
        raise ValueError("resolve template package identity before publication")
    candidates = [Path(file) for file in glob.glob(os.path.expandvars(os.environ.get("PRM_ARTIFACT_GLOB", ".prm-nuget/*.nupkg")))]
    if any(identity(file)[0] != expected for file in candidates):
        raise ValueError("artifact glob includes another package; choose an exact package-specific glob")
    artifacts = candidates
    if len(artifacts) != 1:
        raise ValueError("expected exactly one validated artifact for selected package ID")
    artifact = artifacts[0]
    result = publish(artifact, os.environ.get("PRM_NUGET_OIDC"), os.environ.get("PRM_NUGET_FALLBACK"), push_artifact)
    result["package_id"], result["version"] = identity(artifact)
    result["attempts"][0]["reason"] = ("temporary OIDC key available" if os.environ.get("PRM_NUGET_OIDC") else "NuGet username is missing" if not os.environ.get("PRM_NUGET_USER") else "OIDC exchange failed or returned no key")
    print(json.dumps(result))
    if result["publication"] not in ("accepted", "already-present"):
        raise SystemExit("NuGet publication remains unverified; see classified outcomes above")
    package_id, version = identity(artifact)
    verify_public(package_id, version)


def verify_public(package_id, version):
    for attempt in range(12):
        if exact_version_exists(package_id, version) is True:
            break
        if attempt == 11:
            raise SystemExit("exact public package download is not verified")
        time.sleep(5)
    with tempfile.TemporaryDirectory() as directory:
        for command in (["dotnet", "new", "classlib", "--output", directory], ["dotnet", "add", directory, "package", package_id, "--version", version, "--source", "https://api.nuget.org/v3/index.json"], ["dotnet", "build", directory, "--no-restore"]):
            subprocess.run(command, check=True)
    print(json.dumps({"package_id": package_id, "version": version, "public_download": "verified", "consumer": "passed"}))



if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--verify-public":
        verify_public(sys.argv[2], sys.argv[3])
    else:
        main()
