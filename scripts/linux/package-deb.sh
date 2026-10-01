#!/usr/bin/env bash
# 打 Ubuntu/Debian 安装包（.deb）：在 Ubuntu 22.04 容器里原生构建服务 + Fcitx5 addon + 设置程序，
# 产物在 22.04 及更新的 Ubuntu / Debian 系（如 Deepin 25）上都能装（glibc ≥ 2.35，fcitx5 ≥ 5.0.14）。
#
#   scripts/linux/package-deb.sh                 # 产物 dist/linux/windinput_<版本>_<架构>.deb（本机架构）
#   DATA_DIR=/path/to/data scripts/linux/package-deb.sh
#
# 环境变量：
#   DATA_DIR    随包分发的词库/方案目录（默认 <仓库>/build_dev/data，即 Windows 开发部署用的同一份）
#   OUT_DIR     产物目录（默认 <仓库>/dist/linux）
#   WORK_DIR    构建缓存（cargo registry / target / cmake），默认 ~/.cache/wi-deb；可反复复用，
#               首次全量构建约十几分钟（release 开了 LTO），之后增量
#   DOCKER_PLATFORM  交叉架构打包用，如 linux/arm64（需宿主装了 qemu-user-static，很慢；
#               正式 arm64 包由 CI 的原生 arm runner 出，见 .github/workflows/linux-build.yml）
#   RUST_VERSION  容器里的 Rust 版本（默认取本机 rustc 的版本）
#   SETTING_REPO  设置程序仓库（默认 <仓库>/../wind-setting）；它经 path 依赖引用
#               `../WindInput/...` 与 `../wind-ui-rust`，容器里按同样的兄弟布局挂到 /ws 下
#
# 需要 docker。包内布局：
#   /usr/lib/windinput/{wind_input,data/}                服务及其词库（服务按可执行文件同目录找 data/）
#   /usr/lib/windinput/wind_setting                      设置程序（addon 收到 settings.open 时启动它）
#   /usr/share/applications/windinput-{setting,import}.desktop  应用菜单入口 + windinput:// 协议 / 包文件关联
#   /usr/share/mime/packages/windinput.xml               .wpkg / .wtheme 的 MIME 类型
#   /usr/share/icons/hicolor/{64x64,256x256}/apps/windinput.png
#   /usr/lib/<multiarch>/fcitx5/libwindinput.so           Fcitx5 addon
#   /usr/share/fcitx5/{addon,inputmethod}/windinput.conf  addon / 输入法描述
#   /usr/bin/windinput-setup                              一次性配置当前用户的 Fcitx5
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="$(tr -d '[:space:]' <"$REPO/docs/VERSION")"
STAMP="$(date +%Y%m%d%H%M)"
DEB_VERSION="${VERSION/-/"~"}+linux.${STAMP}"   # 预发布后缀用 ~，排在同号正式版之前
DATA_DIR="${DATA_DIR:-$REPO/build_dev/data}"
OUT_DIR="${OUT_DIR:-$REPO/dist/linux}"
WORK_DIR="${WORK_DIR:-$HOME/.cache/wi-deb}"
RUST_VERSION="${RUST_VERSION:-$(rustc -V | awk '{print $2}')}"
PLATFORM_ARGS=()
IMAGE_TAG="${RUST_VERSION}"
if [[ -n "${DOCKER_PLATFORM:-}" ]]; then
    PLATFORM_ARGS=(--platform "$DOCKER_PLATFORM")
    IMAGE_TAG="${RUST_VERSION}-${DOCKER_PLATFORM//\//-}"
    # 不同架构的 cargo target / cmake 缓存不能混用
    WORK_DIR="$WORK_DIR-${DOCKER_PLATFORM//\//-}"
fi
IMAGE="windinput-build-jammy:rust-${IMAGE_TAG}"

SETTING_REPO="$(readlink -f "${SETTING_REPO:-$REPO/../wind-setting}")"

[[ -d "$DATA_DIR/schemas" ]] || { echo "DATA_DIR 不是词库目录（缺 schemas/）: $DATA_DIR" >&2; exit 1; }
[[ -f "$SETTING_REPO/Cargo.toml" ]] || { echo "找不到设置程序仓库: $SETTING_REPO（用 SETTING_REPO 指定）" >&2; exit 1; }
# wind-ui-rust 常是符号链接（worktree 里指向真仓库）：挂载要用解析后的真实路径，
# 链接本身挂进容器会指向容器里不存在的宿主路径。
UI_REPO="$(readlink -f "$SETTING_REPO/../wind-ui-rust")"
[[ -f "$UI_REPO/Cargo.toml" ]] || { echo "找不到 wind-ui-rust（应在设置程序仓库旁）: $UI_REPO" >&2; exit 1; }
command -v docker >/dev/null || { echo "需要 docker" >&2; exit 1; }
mkdir -p "$WORK_DIR"/{home,cargo-home,target,cmake,stage} "$OUT_DIR"

echo "[deb] 构建镜像 $IMAGE（首次较慢）"
docker build -q "${PLATFORM_ARGS[@]}" -t "$IMAGE" --build-arg "RUST_VERSION=$RUST_VERSION" \
    -f "$REPO/scripts/linux/docker/jammy.Dockerfile" "$REPO/scripts/linux/docker" >/dev/null

echo "[deb] 容器内构建 → $OUT_DIR"
# 容器内步骤放独立脚本（同目录的 build-deb.sh，CI 原生 runner 上也用它），避免多层引号/heredoc 嵌套。
docker run --rm "${PLATFORM_ARGS[@]}" -u "$(id -u):$(id -g)" \
    -e HOME=/work/home -e CARGO_HOME=/work/cargo-home -e CARGO_TARGET_DIR=/work/target \
    -e DEB_VERSION="$DEB_VERSION" -e APP_VERSION="$VERSION" \
    -e SRC_DIR=/src -e DATA_DIR=/data -e SETTING_DIR=/ws/wind-setting -e WORK_DIR=/work -e OUT_DIR=/out \
    -e SERVICE_TARGET=/work/target -e SETTING_TARGET=/work/target-setting \
    -v "$REPO:/src:ro" -v "$DATA_DIR:/data:ro" -v "$WORK_DIR:/work" -v "$OUT_DIR:/out" \
    -v "$REPO:/ws/WindInput:ro" -v "$SETTING_REPO:/ws/wind-setting:ro" -v "$UI_REPO:/ws/wind-ui-rust:ro" \
    "$IMAGE" bash /src/scripts/linux/build-deb.sh

echo "[deb] 完成："
ls -la "$OUT_DIR"/windinput_"${DEB_VERSION}"_*.deb
