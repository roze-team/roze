#!/usr/bin/env python3
"""Statically verify the contract between Roze and an adjacent roze-dtm checkout."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any


ROZE_GIT_URL = "https://github.com/roze-team/roze.git"
FULL_REVISION = re.compile(r"^[0-9a-f]{40}$")


def git_revision(repository: Path) -> str:
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=repository,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip().lower()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def load_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def source_contains(path: Path, marker: str) -> bool:
    return path.is_file() and marker in path.read_text(encoding="utf-8")


def inspect(roze_root: Path, dtm_root: Path) -> dict[str, Any]:
    require((dtm_root / ".git").exists(), f"not a Git checkout: {dtm_root}")
    manifest = load_toml(dtm_root / "Cargo.toml")
    require(manifest.get("package", {}).get("name") == "roze-dtm", "unexpected DTM package name")
    require("service" in manifest.get("workspace", {}).get("members", []), "missing DTM service member")
    package_version = str(manifest.get("workspace", {}).get("package", {}).get("version", ""))
    require(package_version, "missing DTM workspace package version")

    dependencies = manifest.get("workspace", {}).get("dependencies", {})
    roze_dependencies = {
        name: value
        for name, value in dependencies.items()
        if isinstance(value, dict) and value.get("git") == ROZE_GIT_URL
    }
    require(roze_dependencies, "no pinned Roze Git dependencies found")
    revisions = {str(value.get("rev", "")).lower() for value in roze_dependencies.values()}
    require(len(revisions) == 1, "Roze dependencies do not share one revision")
    pinned_roze_revision = revisions.pop()
    require(
        FULL_REVISION.fullmatch(pinned_roze_revision) is not None
        and set(pinned_roze_revision) != {"0"},
        "Roze dependency revision is not a full, non-zero Git revision",
    )

    require(
        "pub struct DtmHttpClient" in (dtm_root / "src" / "client.rs").read_text(encoding="utf-8"),
        "missing DtmHttpClient",
    )
    require(
        "pub struct DtmGrpcClient"
        in (dtm_root / "src" / "grpc_client.rs").read_text(encoding="utf-8"),
        "missing DtmGrpcClient",
    )

    openapi = json.loads((dtm_root / "service" / "static" / "openapi.json").read_text(encoding="utf-8"))
    require("/api/dtmsvr/version" in openapi.get("paths", {}), "missing DTM revision endpoint")
    openapi_info_version = str(openapi.get("info", {}).get("version", ""))
    require(openapi_info_version, "missing DTM OpenAPI info.version")
    version_schema = openapi.get("components", {}).get("schemas", {}).get("CompatVersion", {})
    require(
        "release_revision" in version_schema.get("required", [])
        and "release_revision" in version_schema.get("properties", {}),
        "DTM version contract does not require release_revision",
    )

    capabilities = {
        "trusted_branch_tls": source_contains(
            dtm_root / "service" / "src" / "main.rs", "branch_tls_ca_file"
        )
        and source_contains(dtm_root / "src" / "lib.rs", "with_enabled_roots")
        and source_contains(dtm_root / "src" / "lib.rs", "ca_certificate"),
        "retention_compare_and_delete": source_contains(
            dtm_root / "src" / "lib.rs", "delete_transaction_if_unchanged"
        )
        and source_contains(
            dtm_root / "service" / "src" / "main.rs", "roze_dtm_retention_deleted_total"
        ),
        "cross_language_protocol_acceptance": (
            dtm_root / "interop" / "dtm-labs-go" / "main.go"
        ).is_file()
        and (dtm_root / "scripts" / "sdk-protocol-integration.mjs").is_file()
        and (dtm_root / "scripts" / "sdk-typescript-integration.ts").is_file(),
        "grpc_callback_recovery": (dtm_root / "examples" / "grpc_callback_smoke.rs").is_file()
        and (dtm_root / "proto" / "workflow_callback_test.proto").is_file(),
        "official_tcc_rollback_acceptance": source_contains(
            dtm_root / "interop" / "dtm-labs-go" / "main.go",
            "dtmcli.TccGlobalTransaction",
        )
        and source_contains(
            dtm_root / "interop" / "dtm-labs-go" / "main.go", '"tcc_cancel"'
        ),
        "redis_fault_injection": source_contains(
            dtm_root / ".github" / "workflows" / "ci.yml",
            "ROZE_TEST_REDIS_CLUSTER_FAULT_SLOT",
        )
        and source_contains(
            dtm_root / ".github" / "workflows" / "ci.yml", "after primary failover"
        )
        and (dtm_root / "scripts" / "production-soak.mjs").is_file()
        and (dtm_root / "scripts" / "validate-soak-evidence.py").is_file(),
        "delayed_message_restart_recovery": source_contains(
            dtm_root / ".github" / "workflows" / "ci.yml",
            "node scripts/message-restart-integration.mjs",
        )
        and source_contains(
            dtm_root / "scripts" / "message-restart-integration.mjs",
            'delay_millis: 4_000',
        )
        and source_contains(
            dtm_root / "scripts" / "message-restart-integration.mjs",
            'child.kill("SIGKILL")',
        )
        and source_contains(
            dtm_root / "scripts" / "message-restart-integration.mjs",
            "branchCalls === 1",
        )
        and source_contains(
            dtm_root / "scripts" / "message-restart-integration.mjs",
            'waitForStatus(gid, "succeeded"',
        ),
    }

    roze_revision = git_revision(roze_root)
    dtm_revision = git_revision(dtm_root)
    return {
        "status": "aligned" if pinned_roze_revision == roze_revision else "pinned_baseline_differs",
        "roze_revision": roze_revision,
        "dtm_revision": dtm_revision,
        "dtm_roze_revision": pinned_roze_revision,
        "roze_dependency_count": len(roze_dependencies),
        "package_version": package_version,
        "openapi_info_version": openapi_info_version,
        "package_openapi_versions_match": package_version == openapi_info_version,
        "capabilities": capabilities,
        "rust_clients": [
            "roze_dtm::client::DtmHttpClient",
            "roze_dtm::grpc_client::DtmGrpcClient",
        ],
        "revision_endpoint": "/api/dtmsvr/version",
        "revision_field": "release_revision",
    }


def validate_baseline(report: dict[str, Any], baseline: dict[str, Any]) -> None:
    require(baseline.get("schema_version") == 1, "unsupported compatibility baseline schema")
    for field in [
        "dtm_revision",
        "dtm_roze_revision",
        "package_version",
        "openapi_info_version",
    ]:
        require(
            report.get(field) == baseline.get(field),
            f"compatibility baseline mismatch for {field}: expected {baseline.get(field)!r}, got {report.get(field)!r}",
        )

    upstream_ci = baseline.get("upstream_ci")
    require(isinstance(upstream_ci, dict), "missing upstream_ci provenance")
    require(upstream_ci.get("workflow") == "ci", "unexpected upstream CI workflow")
    run_id = upstream_ci.get("run_id")
    require(isinstance(run_id, int) and run_id > 0, "invalid upstream CI run_id")
    require(
        upstream_ci.get("head_revision") == report.get("dtm_revision"),
        "upstream CI head revision does not match the DTM baseline",
    )
    require(
        upstream_ci.get("recorded_conclusion") == "success",
        "upstream CI baseline is not recorded as successful",
    )
    require(
        upstream_ci.get("url")
        == f"https://github.com/roze-team/roze-dtm/actions/runs/{run_id}",
        "upstream CI URL does not match run_id",
    )

    capabilities = report.get("capabilities", {})
    required_capabilities = baseline.get("required_capabilities")
    require(
        isinstance(required_capabilities, list) and required_capabilities,
        "required_capabilities must be a non-empty list",
    )
    require(
        all(isinstance(capability, str) and capability for capability in required_capabilities),
        "required_capabilities must contain non-empty strings",
    )
    require(
        len(required_capabilities) == len(set(required_capabilities)),
        "required_capabilities contains duplicates",
    )
    for capability in required_capabilities:
        require(capabilities.get(capability) is True, f"missing required DTM capability: {capability}")
    report["upstream_ci"] = upstream_ci
    report["baseline_status"] = "matched"


def main() -> int:
    roze_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dtm-dir",
        type=Path,
        default=roze_root.parent / "roze-dtm",
        help="path to the independent roze-dtm checkout",
    )
    parser.add_argument(
        "--require-roze-head",
        action="store_true",
        help="fail when roze-dtm does not pin the current Roze HEAD",
    )
    parser.add_argument(
        "--baseline",
        type=Path,
        default=roze_root / "docs" / "integrations" / "roze-dtm-compatibility.json",
        help="versioned compatibility baseline to enforce",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="optional path for the machine-readable compatibility report",
    )
    args = parser.parse_args()

    try:
        report = inspect(roze_root, args.dtm_dir.resolve())
        validate_baseline(report, load_json(args.baseline.resolve()))
    except (OSError, ValueError, KeyError, json.JSONDecodeError, subprocess.CalledProcessError) as error:
        print(json.dumps({"status": "invalid", "error": str(error)}, ensure_ascii=False, indent=2))
        return 2

    rendered = json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    print(rendered, end="")
    if args.output:
        output = args.output.resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(rendered, encoding="utf-8")
    if args.require_roze_head and report["status"] != "aligned":
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
