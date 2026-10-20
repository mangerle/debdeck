#!/usr/bin/env bash
# DebDeck 原生 Debian (.deb) 软件包构建脚本（由 cargo-deb 驱动）
#
# 打包元数据集中在 Cargo.toml 的 [package.metadata.deb]：依赖、分类、装机路径、
# 维护脚本目录都在那里声明，本脚本只负责「检查 → 编译 → 出包 → 校验 → 交付」。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="${SCRIPT_DIR}"

PKG_NAME="debdeck"
PKG_VERSION="$(grep -m1 '^version' "${PROJECT_ROOT}/Cargo.toml" | tr -d ' "' | cut -d'=' -f2)"
PKG_ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
BUILD_DIR="${PROJECT_ROOT}/target/debian"

DO_INSTALL=false
usage() {
    echo "DebDeck Debian 软件包构建工具"
    echo "用法: $0 [选项]"
    echo ""
    echo "选项:"
    echo "  -i, --install   打包完成后自动调用 sudo dpkg -i 安装"
    echo "  -h, --help      显示帮助信息"
    echo ""
    echo "产物: ${BUILD_DIR}/${PKG_NAME}_<版本>_${PKG_ARCH}.deb"
}
for arg in "$@"; do
    case "${arg}" in
        -i | --install) DO_INSTALL=true ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "未知选项: ${arg}" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [ -z "${PKG_VERSION}" ]; then
    echo "[打包失败] 无法从 Cargo.toml 读取版本号" >&2
    exit 1
fi

echo "[DebDeck 打包] 步骤 1/3: 检查维护脚本与 cargo-deb"
# dpkg 要求维护脚本可执行；权限位不在版本库里可靠保存，每次构建前显式补齐。
chmod +x "${PROJECT_ROOT}"/debian/postinst "${PROJECT_ROOT}"/debian/postrm
if ! command -v cargo-deb >/dev/null 2>&1; then
    echo "未检测到 cargo-deb，正在通过 cargo 自动安装..."
    cargo install cargo-deb
fi

echo "[DebDeck 打包] 步骤 2/3: 编译 release 二进制"
cd "${PROJECT_ROOT}"
cargo build --release

echo "[DebDeck 打包] 步骤 3/3: 构建并校验 Debian 软件包"
rm -f "${BUILD_DIR}/${PKG_NAME}"_*.deb
cargo deb --no-build

LATEST_DEB="$(ls -t "${BUILD_DIR}"/*.deb 2>/dev/null | head -n1 || true)"
if [ -z "${LATEST_DEB}" ] || [ ! -f "${LATEST_DEB}" ]; then
    echo "[打包失败] 未找到生成的 Debian 软件包" >&2
    exit 1
fi

OUTPUT_DEB="${LATEST_DEB}"

# 自检：control 里该有的字段与归档里该有的路径都要在。
# 早先版本靠 sed 替换 .desktop 里的 @EXEC@ 占位符，替换漏了也能出包，
# 装出来才发现菜单项启动不了——现在直接断言成品内容。
CONTROL="$(dpkg-deb -f "${OUTPUT_DEB}" Depends)"
for required in apt pkexec xdg-utils; do
    if ! grep -q "${required}" <<<"${CONTROL}"; then
        echo "[打包失败] Depends 缺少 ${required}：${CONTROL}" >&2
        exit 1
    fi
done
CONTENTS="$(dpkg-deb -c "${OUTPUT_DEB}")"
for path in ./usr/bin/debdeck ./usr/share/applications/debdeck.desktop ./usr/share/icons/hicolor/256x256/apps/debdeck.png; do
    if ! grep -q "${path}" <<<"${CONTENTS}"; then
        echo "[打包失败] 包内缺少 ${path}" >&2
        exit 1
    fi
done

# .desktop 必须是可直接执行的成品，不允许残留模板占位符。
VERIFY_DIR="$(mktemp -d)"
trap 'rm -rf "${VERIFY_DIR}"' EXIT
dpkg-deb -x "${OUTPUT_DEB}" "${VERIFY_DIR}"
if grep -q "@EXEC@" "${VERIFY_DIR}/usr/share/applications/debdeck.desktop"; then
    echo "[打包失败] .desktop 里仍有未替换的 @EXEC@ 占位符" >&2
    exit 1
fi

DEB_SIZE="$(du -h "${OUTPUT_DEB}" | cut -f1)"
echo "============================================================"
echo "[打包完成] 成功构建 Debian 软件包:"
echo "  软件包路径: ${OUTPUT_DEB} (体积: ${DEB_SIZE})"
echo "  手动安装:   sudo dpkg -i ${OUTPUT_DEB}"
echo "  卸载:       sudo apt remove ${PKG_NAME}"
echo "============================================================"

if [ "${DO_INSTALL}" = true ]; then
    echo "[自动安装] 正在执行 sudo dpkg -i ${OUTPUT_DEB}..."
    sudo dpkg -i "${OUTPUT_DEB}"
fi
