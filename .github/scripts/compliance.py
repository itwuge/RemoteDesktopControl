# ============================================================
# 脚本用途：生成 NOTICE.txt 与 CycloneDX 1.5 SBOM
#
# 层级：CI 合规 job
# 调用方：.github/workflows/ci.yml 的 compliance 步骤
#
# 实现选择：不引入 cargo-cyclonedx（安装耗时长），直接用 cargo metadata 的 JSON
# 生成最小可用 SBOM；许可证字段来自各 crate manifest，缺失时写 UNKNOWN 并告警。
# ============================================================
import json
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DIST = REPO_ROOT / "dist"
DIST.mkdir(exist_ok=True)


def cargo_metadata() -> dict:
    """读取 workspace 依赖元数据。"""
    out = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=REPO_ROOT,
        text=True,
    )
    return json.loads(out)


def license_of(pkg: dict) -> str:
    """取许可证表达式；缺失时回落到 UNKNOWN 并告警。"""
    lic = pkg.get("license")
    if not lic:
        lic = pkg.get("license_file") or "UNKNOWN"
        print(f"::warning::包 {pkg['name']} 缺少许可证字段，记为 {lic}")
    return lic


def main() -> int:
    meta = cargo_metadata()
    packages = sorted(meta["packages"], key=lambda p: (p["name"], p["version"]))

    # =====** NOTICE =====**
    lines = [
        "RDC NOTICE",
        "=" * 40,
        "本文件列出构建产物包含的第三方 crate 及其许可证声明。",
        "由 .github/scripts/compliance.py 自动生成，请勿手工编辑。",
        "",
    ]
    for pkg in packages:
        lines.append(f"- {pkg['name']} {pkg['version']} — {license_of(pkg)}")
        if pkg.get("repository"):
            lines.append(f"  {pkg['repository']}")
    (DIST / "NOTICE.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")

    # =====** CycloneDX 最小 SBOM =====**
    components = [
        {
            "type": "library",
            "name": pkg["name"],
            "version": pkg["version"],
            "licenses": [{"license": {"name": license_of(pkg)}}],
            **({"externalReferences": [{"type": "vcs", "url": pkg["repository"]}]} if pkg.get("repository") else {}),
        }
        for pkg in packages
    ]
    sbom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "rdc",
                "version": packages[0]["version"] if packages else "0.1.0",
            }
        },
        "components": components,
    }
    (DIST / "sbom.cdx.json").write_text(
        json.dumps(sbom, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    print(f"已生成 {DIST / 'NOTICE.txt'} 与 {DIST / 'sbom.cdx.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
